// wgpu_2d.rs
//
// S3-1：wGPU 2D 后端 + 离屏纹理 + MSAA（4x）。
//
// S4.1 后本文件是**唯一**的 2D 渲染路径（旧 CPU 回放 `egui_2d.rs` 已随其
// 编译期 feature 一并删除）——所有 `Shape2D` 都经此渲染。
// 本后端把 `Shape2D` 三角化（S3-0 的 lyon 适配层）后渲染到**离屏纹理**，
// 再交给 [`crate::Compositor`] 按声明顺序合成进 eframe 主 pass
// —— 为 S3-2（合成器）/ S3-3（2D/3D 互为纹理）铺路。
//
// S3-2/C2 变化：本文件**不再自带合成管线**（原先的 `pipeline_composite` /
// `composite_bgl` / `sampler` / `composite()` / `COMPOSITE_QUAD` 已移除），
// 合成统一由 [`crate::Compositor`] 承担；离屏清屏改为透明（背景由 `host` 提供）。
//
// 关键口径：
// - **顶点布局来自契约**：`to_wgpu(&VertexLayout::standard(AaModel::None, PerFrame))`，
//   无任何硬编码 `Float32x2` 字面量；
// - **MSAA 4x**：`MSAA_SAMPLES = 4`，离屏 pass = MSAA 纹理 → resolve 到单采样离屏纹理；
// - **2D 不用深度缓冲**：z 序由 `ZIndex` 稳定排序保证（画家算法）；
// - **uniform 复用 `UniformArena`**（256B 对齐 dynamic offset），不另造一套；
// - 几何缓冲按**内容哈希**缓存（`ResourceCache`），非"每帧必建"；
//   动画内容每帧哈希不同 → 帧末 `evict_unused(0)` + deferred destroy 保证有界。
//   注意：本后端的缓存决策**只认内容哈希**，`VertexLayout.cache`（`CacheStrategy`）
//   字段这里**仅作声明**、不参与决策（见 `tessellate::tessellation_layout` 的说明）。
//
// 已知限制（与计划 §6 一致）：本阶段 `AaModel::None`（硬边 + MSAA），**尚未启用**解析 AA。
// 但描边几何已**参数化**：顶点存路径点（不含宽度），宽度由 `shape_2d.wgsl` 现场按
// `position + normal * (stroke_width * 0.5)` 补偿（见 `tessellate.rs` 头部说明）。

use std::collections::HashMap;
use std::mem;

use ze_notion::Mat4;
use wgpu::util::DeviceExt as _;

use ze_notion::{Color32, MeasureSpace, Rect2D, Transform2D, Vec2};
use zela_render::resource_cache::{ResourceCache, ResourceKey};
use zela_render::{
    AaModel, CacheStrategy, DrawQueue, GpuHandle, MorphItem, Painter2DTarget, REVEAL_STEP_PX,
    RevealItem, Shape2D, Style2D, TessKind, VertexInputDesc, VertexLayout, hash_bytes,
    tolerance_for_scale,
};
use zela_render::vertex_record;
use ze_render_ir::LayerId;
use ze_vertex::VertexBufferData;

use crate::gpu_util::{UniformArena, bytes_of};
use crate::vertex_layout::{to_wgpu, to_wgpu_buffers};

/// MSAA 采样数（验收项：`multisample.count == 4`）。
pub const MSAA_SAMPLES: u32 = 4;

/// 离屏纹理格式：**非 sRGB**（存 gamma 值），与 egui-wgpu 对主目标的口径一致
/// （`egui.wgsl` 的 `fs_main_gamma_framebuffer` 分支 + 其 "egui prefers Rgba8Unorm" 提示）。
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// uniform 块大小：mat4x4（64）+ color vec4（16）+ params vec4（16）。
///
/// `params.x` = 描边宽度（**与几何同一空间**的屏幕像素，由 `draw.rs::resolve_style`
/// 唯一换算点解析）；填充 draw 传 0（`normal=[0,0]` 时偏移恒 0，与传 0 等价）。
const UNIFORM_SIZE: u64 = 96;
/// wgpu dynamic uniform offset 要求的最小对齐。
const DYNAMIC_OFFSET_ALIGNMENT: u64 = 256;

/// 离屏清屏色：**透明**（S3-2/C2）。
///
/// 背景不属于任何图层——它由合成器清屏（`SceneView::background`）提供。
/// 2D 离屏只保留实际绘制内容，图层外是 alpha=0，这样 2D 才能叠到 3D 之上
/// 而不盖死下层（若清成不透明，合成的就是一张不透明卡片）。
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

/// 2D 几何着色器（几何 pass）。
pub const SHAPE_2D_SHADER: &str = include_str!("../shaders/shape_2d.wgsl");

/// 2D **形变**几何着色器（S4.3）：双 vertex buffer + `t` uniform（见 shader 头注）。
pub const SHAPE_2D_MORPH_SHADER: &str = include_str!("../shaders/shape_2d_morph.wgsl");

// ─── 帧几何 ───────────────────────────────────────────────────────────────

/// 本帧的视口 / 屏幕几何（由 egui 回调在 `prepare` 阶段提供）。
#[derive(Debug, Clone, Copy)]
pub struct Frame2DGeometry {
    /// 视口矩形（egui points，屏幕坐标，原点在左上）。
    pub viewport_points: Rect2D,
    /// 每 point 的物理像素数。
    pub pixels_per_point: f32,
    /// 整个渲染目标尺寸（物理像素）。只用于把视口矩形**钳制**到屏幕内
    /// （与 `epaint::ViewportInPixels::from_points` 同口径），使离屏纹理尺寸
    /// 恒等于 egui-wgpu 为回调设置的 render pass viewport 尺寸；
    /// 合成四边形本身不依赖它（见 [`crate::compositor::COMPOSITE_QUAD`]）。
    pub screen_size_px: [u32; 2],
}

impl Frame2DGeometry {
    /// 视口的物理像素矩形（与 `epaint::ViewportInPixels::from_points` 同一取整口径，
    /// 保证离屏纹理尺寸 == egui-wgpu 为该回调设置的 render pass viewport 尺寸）。
    pub fn viewport_px(&self) -> (i32, i32, u32, u32) {
        let ppp = self.pixels_per_point;
        let screen_w = self.screen_size_px[0] as i32;
        let screen_h = self.screen_size_px[1] as i32;
        let left = (ppp * self.viewport_points.min.x).round() as i32;
        let top = (ppp * self.viewport_points.min.y).round() as i32;
        let right = (ppp * self.viewport_points.max.x).round() as i32;
        let bottom = (ppp * self.viewport_points.max.y).round() as i32;
        let left = left.clamp(0, screen_w);
        let right = right.clamp(left, screen_w);
        let top = top.clamp(0, screen_h);
        let bottom = bottom.clamp(top, screen_h);
        (left, top, (right - left) as u32, (bottom - top) as u32)
    }

    /// 离屏纹理尺寸（物理像素，至少 1×1）。
    fn offscreen_size(&self) -> (u32, u32) {
        let (_, _, w, h) = self.viewport_px();
        (w.max(1), h.max(1))
    }

    /// 图元点（points，屏幕坐标系）→ 离屏本地像素。
    fn local_transform(&self) -> Transform2D {
        Transform2D::translate(-self.viewport_points.min)
            .compose(Transform2D::scale(Vec2::splat(self.pixels_per_point)))
    }

    /// 离屏本地像素 → 裁剪空间（Y 翻转）。
    fn clip_matrix(&self) -> Mat4 {
        let (w, h) = self.offscreen_size();
        let (w, h) = (w as f32, h as f32);
        Mat4::from_cols_array(&[
            2.0 / w,
            0.0,
            0.0,
            0.0,
            0.0,
            -2.0 / h,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            -1.0,
            1.0,
            0.0,
            1.0,
        ])
    }
}

// ─── 录制指令 ─────────────────────────────────────────────────────────────

/// 一条被记录的 2D 指令。
#[derive(Debug, Clone)]
pub struct Recorded2D {
    pub shape: Shape2D,
    pub transform: Transform2D,
    pub style: Style2D,
    pub z: i32,
}

/// 从 [`DrawQueue`] 抓取 2D 快照（按 `ZOrder` 稳定排序，同 z 保录制顺序）。
///
/// B2-1 起 `DrawQueue` 是**统一队列**（2D 形状 + 3D 网格/线段共存）：
/// 本函数只取 `Shape` 载荷，3D 项由 3D 后端消费。
pub fn snapshot_queue(queue: &DrawQueue) -> Vec<Recorded2D> {
    let mut cmds: Vec<Recorded2D> = queue
        .commands()
        .into_iter()
        .map(|c| Recorded2D {
            shape: c.shape.clone(),
            transform: Transform2D::identity(),
            style: c.style,
            z: c.z.0,
        })
        .collect();
    // `sort_by_key` 是稳定排序：同 z 保录制顺序（与 DrawQueue 语义一致）。
    cmds.sort_by_key(|c| c.z);
    cmds
}

// ─── GPU 资源 ─────────────────────────────────────────────────────────────

/// 缓存条目：一条几何的**生产结果（CPU 侧）**。
///
/// B3-0c：条目形态从 `{vertices: Vec<Vertex2D>, indices}` 变为
/// `VertexBufferData`（已按布局交错的字节 + 索引 + 顶点数）——**打包在生产端完成**，
/// 后端只保存与拼接字节。两级缓存结构保持不变（B4）。
struct CachedGeometry {
    data: VertexBufferData,
}

/// 合批缓存条目：同 `(color, stroke_width)` 的若干几何合并后的**一套** GPU 缓冲。
///
/// 成员的顶点按序拼接；成员索引已按各自顶点基址**重排**（rebase），故整批
/// 只需**一次** `draw_indexed(0..index_count, 0, ..)`——这是 draw call 数
/// 从"几何条数"降到"批数"的关键。
struct BatchBuffers {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

/// `ResourceKey` 类型标签：2D 三角化几何（CPU 侧，免重复三角化）。
struct Geometry2DTag;

/// `ResourceKey` 类型标签：合批后的 GPU 缓冲。
struct Batch2DTag;

/// 形变缓存条目（S4.3）：`from` / `to` 两侧几何的 GPU 顶点缓冲 + 共享索引缓冲。
///
/// 两侧顶点数/索引数**一一对应**（等点重采样保证），故共用一份索引表；
/// 逐帧变的只有 `t`（uniform），本条目内容帧间不变 ⇒ 稳态零上传。
struct MorphBuffers {
    from_vertex_buffer: wgpu::Buffer,
    to_vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

/// `ResourceKey` 类型标签：形变（from/to 双侧）GPU 缓冲。
struct Morph2DTag;

/// 书写动画（R0 reveal）：一条路径的 GPU 顶点缓冲 + 索引缓冲。
///
/// 与 [`MorphBuffers`] 同一设计：内容**不含** reveal 区间 `[a,b]` ⇒ 缓存键不含区间
/// ⇒ 区间逐帧变化时零装配零上传（逐帧只多写一个 uniform）。
struct RevealBuffers {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

/// `ResourceKey` 类型标签：书写动画（reveal）GPU 缓冲。
struct Reveal2DTag;

/// 离屏渲染目标（随视口尺寸重建）。
struct OffscreenTargets {
    size: (u32, u32),
    /// 单采样离屏纹理（MSAA resolve 目标，也是被采样的那张）。
    _color: wgpu::Texture,
    /// 单采样离屏纹理视图。
    view: wgpu::TextureView,
    /// MSAA 颜色纹理。
    _msaa: wgpu::Texture,
    /// MSAA 颜色纹理视图（离屏 pass 的渲染目标）。
    msaa_view: wgpu::TextureView,
}

/// 已准备的一次**合批**绘制（`draw_indexed` 一次）。
struct Prepared2D {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_offset: u32,
}

/// 已准备的一次**形变**绘制（双 vertex buffer + `t` uniform，`draw_indexed` 一次）。
struct Prepared2DMorph {
    from_vertex_buffer: wgpu::Buffer,
    to_vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_offset: u32,
}

/// 已准备的一次**书写动画**绘制（单 vertex buffer + 区间 uniform，`draw_indexed` 一次）。
struct Prepared2DReveal {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_offset: u32,
}

/// 本帧缓存命中统计（S4.2 / S4.3：让"合批 + 缓存"可观测、可证伪）。
///
/// 稳态（内容不变）下 `geom_misses == 0`：几何条目只要本帧被再次用到就会
/// 刷新 `last_used_frame`，因此不会被帧末 `evict_unused(0)` 驱逐。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameCacheStats {
    /// 本帧请求的几何数（三角化产出条数）。
    pub geom_requests: usize,
    /// 其中缓存未命中（需重新三角化并建条目）的数。
    pub geom_misses: usize,
    /// 本帧请求的形变几何数（S4.3）。
    pub morph_requests: usize,
    /// 其中缓存未命中的数。⚠️ `t` 变化**不**计入 − 形变几何与 `t` 无关，
    /// 稳态（只有 `t` 在变）下本值恒为 0。
    pub morph_misses: usize,
    /// 本帧请求的书写动画几何数（R0）。
    pub reveal_requests: usize,
    /// 其中缓存未命中的数。⚠️ reveal 区间 `[a,b]` 变化**不**计入
    /// − 几何与区间无关，稳态（只有区间在变）下本值恒为 0。
    pub reveal_misses: usize,
}

/// 一帧的离屏结果（`prepare` 阶段产出、由回调交给合成器）。
pub struct Frame2DData {
    /// 离屏纹理（单采样）视图（合成器的采样源）。
    pub offscreen: wgpu::TextureView,
    /// 本帧的**几何条数**（三角化产出条数；语义自 S3-1 起未变）。
    pub draw_count: usize,
    /// 本帧实际 `draw_indexed` 次数 = 合批后的**批数**（≤ `draw_count`）。
    pub batch_count: usize,
    /// 本帧缓存命中统计。
    pub cache: FrameCacheStats,
    /// 保活：各批的 bind group 引用的 uniform arena 缓冲。
    pub _arena_buffer: wgpu::Buffer,
}

// ─── WgpuBackend2D ────────────────────────────────────────────────────────

/// wGPU 2D 后端：2D 几何管线 + 离屏/MSAA 目标 + uniform arena +
/// 两级缓存（三角化结果 / 合批 GPU 缓冲）。
///
/// **不含合成管线**（S3-2/C2 起统一由 [`crate::Compositor`] 承担）。
pub struct WgpuBackend2D {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// 2D 几何管线（4x MSAA，目标 = 离屏 MSAA 纹理）。
    pipeline_2d: wgpu::RenderPipeline,
    /// 2D **形变**管线（S4.3）：双 vertex buffer + `t` uniform，其余状态同 `pipeline_2d`。
    pipeline_2d_morph: wgpu::RenderPipeline,
    /// 动态 uniform 绑定布局（与 [`UniformArena`] 配套）。
    bind_group_layout: wgpu::BindGroupLayout,
    arena: UniformArena,
    /// uniform arena 的 bind group（缓存：arena generation + bind group），
    /// 仅在 arena 扩容重建 buffer 时失效重建。
    arena_bind_group: Option<(u64, wgpu::BindGroup)>,
    /// 三角化结果缓存（CPU 侧，按内容哈希）：命中即免三角化。
    geom_cache: ResourceCache<()>,
    /// 合批后的 GPU 缓冲缓存（按"批内容"哈希）：命中即免装配 + 免上传。
    batch_cache: ResourceCache<()>,
    /// 形变（from/to 双侧）GPU 缓冲缓存（S4.3）：`t` 不参与键 ⇒ 稳态零上传。
    morph_cache: ResourceCache<()>,
    /// 书写动画 GPU 缓冲缓存（R0）：区间 `[a,b]` 不参与键 ⇒ 稳态零上传。
    reveal_cache: ResourceCache<()>,
    /// 按 [`LayerId`] 分别持有的离屏目标（尺寸变化时重建）。
    targets: HashMap<LayerId, OffscreenTargets>,
    /// 本帧几何（`begin_frame` 设置，`finish_frame` 消费）。
    geometry: Frame2DGeometry,
    recorder: Vec<Recorded2D>,
    /// 本帧形变项（S4.3，`begin_frame_at` 清空）。
    morph_recorder: Vec<MorphItem>,
    /// 本帧书写动画项（R0，`begin_frame_at` 清空）。
    reveal_recorder: Vec<RevealItem>,
}

// S1 徽章：egui 的 `CallbackResources` 在 native 上是
// `type_map::concurrent::TypeMap`，其 `insert` 要求 `Send + Sync`。
// 本项目渲染全程单线程、主线程唯一 —— 这个边界不是真实的跨线程传递，
// 只是 egui 的类型要求。native 的 wgpu 资源本身是 `Send + Sync`，
// 本结构内的 `GpuHandle` 只承载 wgpu 资源（全部由本 crate 的 `GpuHandle::new` 创建），
// 因此这里显式声明徽章，而不把 `Send + Sync` 写回 `GpuHandle`：
// wasm 上 wgpu 资源含 `Rc<RefCell<…>>`，全局约束会让 wasm 直接编译失败。
#[cfg(not(target_arch = "wasm32"))]
unsafe impl Send for WgpuBackend2D {}
#[cfg(not(target_arch = "wasm32"))]
unsafe impl Sync for WgpuBackend2D {}

impl WgpuBackend2D {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let device = device.clone();
        let queue = queue.clone();

        // ── uniform：dynamic offset，与 UniformArena 配套 ──
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("zela-2d-bind-group-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: std::num::NonZeroU64::new(UNIFORM_SIZE),
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("zela-2d-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // ── 顶点布局：来自契约（无硬编码字面量） ──
        let vertex_layout = VertexLayout::standard(AaModel::None, CacheStrategy::PerFrame);
        let mut attrs = Vec::new();
        let wgpu_vertex_layout = to_wgpu(&vertex_layout, &mut attrs);

        let shape_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("zela-2d-shape-shader"),
            source: wgpu::ShaderSource::Wgsl(SHAPE_2D_SHADER.into()),
        });

        let pipeline_2d = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("zela-2d-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shape_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu_vertex_layout)],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shape_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OFFSCREEN_FORMAT,
                    // 预乘 alpha 混合（shader 已预乘）；不透明色下与 egui 一致。
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            // 2D 按 ZIndex 画家算法，不用深度缓冲。
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });

        // ── 形变管线（S4.3）：双 vertex buffer（各用同一 `standard` 布局）+ `t` uniform ──
        // ⚠️ 不动 `VertexLayout::standard`：只是把**同一个契约布局**绑到两个 slot，
        // shader location 由 `to_wgpu_buffers` 跨 buffer 全局连续编号（0..2 / 3..5）。
        let morph_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("zela-2d-morph-shader"),
            source: wgpu::ShaderSource::Wgsl(SHAPE_2D_MORPH_SHADER.into()),
        });
        let morph_input = VertexInputDesc {
            buffers: vec![vertex_layout.clone(), vertex_layout],
        };
        let mut morph_attrs = Vec::new();
        let morph_buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> =
            to_wgpu_buffers(&morph_input, &mut morph_attrs)
                .into_iter()
                .map(Some)
                .collect();
        let pipeline_2d_morph = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("zela-2d-morph-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &morph_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &morph_buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &morph_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OFFSCREEN_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: MSAA_SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });

        // ── 合成管线已外移 ──
        // S3-2/C2：本后端只负责画离屏几何纹理；主 pass 的合成由
        // [`crate::Compositor`] 统一承担（含 sRGB / 非 sRGB 双入口 + opacity）。

        Self {
            device,
            queue,
            pipeline_2d,
            pipeline_2d_morph,
            bind_group_layout,
            arena: UniformArena::new(UNIFORM_SIZE, DYNAMIC_OFFSET_ALIGNMENT),
            arena_bind_group: None,
            geom_cache: ResourceCache::new(),
            batch_cache: ResourceCache::new(),
            morph_cache: ResourceCache::new(),
            reveal_cache: ResourceCache::new(),
            targets: HashMap::new(),
            geometry: Frame2DGeometry {
                viewport_points: Rect2D::default(),
                pixels_per_point: 1.0,
                screen_size_px: [1, 1],
            },
            recorder: Vec::new(),
            morph_recorder: Vec::new(),
            reveal_recorder: Vec::new(),
        }
    }

    /// 本帧视口几何 + 清空录制器。
    pub fn begin_frame_at(&mut self, geometry: Frame2DGeometry) {
        self.geometry = geometry;
        self.recorder.clear();
        self.morph_recorder.clear();
        self.reveal_recorder.clear();
    }

    /// 追加一个**形变**绘制项（S4.3）：`from` / `to` 常量化，逐帧只变 `t`。
    pub fn push_morph(&mut self, item: MorphItem) {
        self.morph_recorder.push(item);
    }

    /// 追加一个**书写动画**绘制项（R0）：路径常量化，逐帧只变区间 `[start, end]`。
    pub fn push_reveal(&mut self, item: RevealItem) {
        self.reveal_recorder.push(item);
    }

    /// 录制器中的形变项数（S4.3 诊断）。
    pub fn recorded_morph_len(&self) -> usize {
        self.morph_recorder.len()
    }

    /// 录制器中的书写动画项数（R0 诊断）。
    pub fn recorded_reveal_len(&self) -> usize {
        self.reveal_recorder.len()
    }

    /// 追加一条绘制指令（`z` 用于稳定排序；同 z 保调用顺序）。
    pub fn push(&mut self, shape: &Shape2D, transform: &Transform2D, style: &Style2D, z: i32) {
        self.recorder.push(Recorded2D {
            shape: shape.clone(),
            transform: *transform,
            style: *style,
            z,
        });
    }

    /// 指定图层的离屏纹理视图（`Painter2DTarget` 路径的"输出纹理"）。
    pub fn offscreen_view(&self, layer_id: LayerId) -> Option<&wgpu::TextureView> {
        self.targets.get(&layer_id).map(|t| &t.view)
    }

    /// 指定图层的离屏纹理本体（S3-2/S3-3 把它当纹理共享/拷贝时的入口）。
    pub fn offscreen_texture(&self, layer_id: LayerId) -> Option<&wgpu::Texture> {
        self.targets.get(&layer_id).map(|t| &t._color)
    }

    /// 录制器中的指令数。
    pub fn recorded_len(&self) -> usize {
        self.recorder.len()
    }

    /// G7 诊断（R5-3）：uniform arena 的本帧占用 / 容量块数（`used` / `capacity`）。
    ///
    /// `used` 每帧被 `begin_frame` 归零，故恒等于"本帧实际 push 的 uniform 块数"。
    pub fn arena_usage(&self) -> (u32, u32) {
        (self.arena.used_blocks(), self.arena.capacity_blocks())
    }

    /// G7 诊断（R5-2）：几何（三角化结果）缓存的存活条目数（帧末 `evict_unused(0)` 之后）。
    pub fn geometry_cache_len(&self) -> usize {
        self.geom_cache.len()
    }

    /// S4.2 诊断：合批（GPU 缓冲）缓存的存活条目数。
    pub fn batch_cache_len(&self) -> usize {
        self.batch_cache.len()
    }

    /// S4.3 诊断：形变（from/to 双侧 GPU 缓冲）缓存的存活条目数。
    pub fn morph_cache_len(&self) -> usize {
        self.morph_cache.len()
    }

    /// R0 诊断：书写动画 GPU 缓冲缓存的存活条目数。
    pub fn reveal_cache_len(&self) -> usize {
        self.reveal_cache.len()
    }

    /// G7 诊断（R5-2）：graveyard 中等待延迟销毁的 GPU 条目数。
    ///
    /// S4.2：GPU 缓冲现由合批缓存持有，故这里报**合批缓存**的 graveyard ——
    /// 它才是"显存有界性"的观测点（几何条目只剩 CPU 数据）。
    pub fn pending_geometry_destroy(&self) -> usize {
        self.batch_cache.pending_destroy_count()
    }

    fn ensure_targets(&mut self, layer_id: LayerId, size: (u32, u32)) {
        if self.targets.get(&layer_id).map(|t| t.size) == Some(size) {
            return;
        }
        let extent = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let color = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("zela-2d-offscreen"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            // COPY_SRC：S3-2/S3-3（互为纹理/回读）需要它，也让本层能在测试里
            // 直接回读像素验证"图元确实落在预期位置、颜色正确"。
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let msaa = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("zela-2d-msaa"),
            size: extent,
            mip_level_count: 1,
            sample_count: MSAA_SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let msaa_view = msaa.create_view(&wgpu::TextureViewDescriptor::default());
        // 旧目标随替换释放（其上帧的 TextureView 由 Frame2DData 持有，下帧被替换时释放）。
        self.targets.insert(
            layer_id,
            OffscreenTargets {
                size,
                _color: color,
                view,
                _msaa: msaa,
                msaa_view,
            },
        );
    }

    /// 一帧落地：三角化 → 上传几何 → uniform → 离屏 MSAA pass。
    ///
    /// `encoder`：由调用方提供（egui 回调复用 egui 的 encoder ⇒ 离屏 pass 在主 pass 之前执行）。
    pub fn finish_frame(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        layer_id: LayerId,
        geometry: &Frame2DGeometry,
    ) -> Frame2DData {
        let size = geometry.offscreen_size();
        self.ensure_targets(layer_id, size);

        let mut draws = mem::take(&mut self.recorder);
        // 稳定排序：z 升序，同 z 保录制顺序（画家算法）。
        draws.sort_by_key(|d| d.z);

        self.arena.begin_frame();
        self.geom_cache.begin_frame();
        self.batch_cache.begin_frame();

        let local = geometry.local_transform();
        // 展平容差按"≤0.25 物理像素"推导（几何在 points 空间，故除以 ppp）。
        let tolerance = tolerance_for_scale(geometry.pixels_per_point);
        let clip = geometry.clip_matrix();

        // ── 趟 1：三角化（带缓存）→ 收集本帧几何请求 ──
        // 请求按绘制顺序记录：`(几何内容哈希, 颜色, 描边宽度)`。
        let mut stats = FrameCacheStats::default();
        let geoms = self.tessellate_pass(&draws, &local, tolerance, &mut stats);

        // ── 趟 2：按 `(color, stroke_width)` 合批 → 每批**一次** draw call ──
        let prepared = self.batch_pass(&geoms, clip);

        // ── 趟 3（S4.3）：形变项 ──
        let prepared_morph = self.morph_pass(&local, tolerance, clip, &mut stats);

        // ── 趟 4（R0）：书写动画项 ──
        let prepared_reveal = self.reveal_pass(&local, tolerance, clip, &mut stats);

        // 帧末驱逐：未被本帧用到的条目投入 deferred destroyer
        // （graveyard 3 次 submit 后才真正 drop），保证动画内容下缓存与显存有界。
        self.geom_cache.evict_unused(0);
        self.geom_cache.notify_submit();
        self.batch_cache.evict_unused(0);
        self.batch_cache.notify_submit();
        self.morph_cache.evict_unused(0);
        self.morph_cache.notify_submit();
        self.reveal_cache.evict_unused(0);
        self.reveal_cache.notify_submit();

        // 一次性上传全部 uniform。bind group 绑定 arena buffer，因此只在 arena
        // 扩容重建 buffer（generation 变化）时重建，避免每帧创建。
        let arena_buffer = {
            let (buf, _stride) = self.arena.flush(&self.device, &self.queue);
            buf.clone()
        };
        let arena_generation = self.arena.generation();
        if self.arena_bind_group.as_ref().map(|(g, _)| *g) != Some(arena_generation) {
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("zela-2d-uniform-bind-group"),
                layout: &self.bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &arena_buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    }),
                }],
            });
            self.arena_bind_group = Some((arena_generation, bg));
        }
        let arena_bind_group = self
            .arena_bind_group
            .as_ref()
            .expect("刚插入的 bind group")
            .1
            .clone();

        let targets = self
            .targets
            .get(&layer_id)
            .expect("ensure_targets 之后必然存在");

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("zela-2d-offscreen-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&targets.view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if !prepared.is_empty() {
                pass.set_pipeline(&self.pipeline_2d);
                // 每个 `prepared` 元素 = 一个**批**（同色/同宽度的若干几何合并），
                // 整批一次 `draw_indexed`（成员索引已 rebase 到同一顶点缓冲基址）。
                for d in &prepared {
                    pass.set_bind_group(0, &arena_bind_group, &[d.uniform_offset]);
                    pass.set_vertex_buffer(0, d.vertex_buffer.slice(..));
                    pass.set_index_buffer(d.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..d.index_count, 0, 0..1);
                }
            }
            if !prepared_morph.is_empty() {
                pass.set_pipeline(&self.pipeline_2d_morph);
                // 每个形变项一次 `draw_indexed`：双顶点缓冲（from/to）+ `t` uniform。
                for m in &prepared_morph {
                    pass.set_bind_group(0, &arena_bind_group, &[m.uniform_offset]);
                    pass.set_vertex_buffer(0, m.from_vertex_buffer.slice(..));
                    pass.set_vertex_buffer(1, m.to_vertex_buffer.slice(..));
                    pass.set_index_buffer(m.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..m.index_count, 0, 0..1);
                }
            }
            // R0：书写动画复用 `pipeline_2d`（`shape_2d.wgsl` 按 `advance` 与区间
            // `params.z/w` 分支），不需要新管线；区间逐项不同 ⇒ 每项一次 `draw_indexed`。
            if !prepared_reveal.is_empty() {
                pass.set_pipeline(&self.pipeline_2d);
                for r in &prepared_reveal {
                    pass.set_bind_group(0, &arena_bind_group, &[r.uniform_offset]);
                    pass.set_vertex_buffer(0, r.vertex_buffer.slice(..));
                    pass.set_index_buffer(r.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..r.index_count, 0, 0..1);
                }
            }
        }

        // ── 产出离屏结果（合成由调用方 / 回调经合成器完成） ──
        Frame2DData {
            offscreen: targets.view.clone(),
            // 「几何条数」= 合批前的三角化几何数 + 形变项数 + 书写动画项数。
            draw_count: geoms.len() + prepared_morph.len() + prepared_reveal.len(),
            // 实际 `draw_indexed` 次数 = 批数 + 形变项数 + 书写动画项数。
            batch_count: prepared.len() + prepared_morph.len() + prepared_reveal.len(),
            cache: stats,
            _arena_buffer: arena_buffer,
        }
    }

    /// 趟 1：三角化（带缓存）→ 收集本帧几何请求。
    ///
    /// 请求按绘制顺序记录：`(几何内容哈希, 颜色, 描边宽度)`。
    /// 本趟不做任何 GPU 上传：几何条目只存 CPU 侧三角化结果（免重复三角化），
    /// 顶点/索引的 GPU 缓冲由趟 2 的合批统一持有。
    fn tessellate_pass(
        &mut self,
        draws: &[Recorded2D],
        local: &Transform2D,
        tolerance: f32,
        stats: &mut FrameCacheStats,
    ) -> Vec<(u64, Color32, f32)> {
        let mut geoms: Vec<(u64, Color32, f32)> = Vec::new();
        for draw in draws {
            // GPU 2D 后端只接受屏幕空间样式（`record_scene` 的输出恒为此）；
            // 世界度量需要相机 scale，不在本后端的职责内。
            debug_assert!(
                draw.style.space == MeasureSpace::Screen,
                "GPU 2D 后端只接受 MeasureSpace::Screen 样式，收到 {:?}",
                draw.style.space
            );
            let style = Style2D {
                space: MeasureSpace::Screen,
                ..draw.style
            };
            // B3-0c：三角化 + 打包在渲染无关层（`vertex_record`）；后端只接字节。
            // B3-0d（不静默）：失败 ⇒ **显式 panic**，不再 `eprintln + continue`。
            let buffers = vertex_record::tessellate_to_buffers(&draw.shape, local, &style, tolerance)
                .unwrap_or_else(|error| {
                    panic!("不静默：2D 顶点生产失败（三角化/打包）：{error}")
                });
            for buffer in buffers {
                if buffer.data.is_empty() || buffer.data.indices.is_empty() {
                    continue;
                }
                let hash = hash_tessellated(buffer.kind, &buffer.data);
                let key = ResourceKey::content::<Geometry2DTag>(hash);
                stats.geom_requests += 1;
                // ⚠️ 命中必须 `mark_used`：`ResourceCache::get` 不刷新 `last_used_frame`，
                // 只用 `get` 的话帧末 `evict_unused(0)` 会把稳态内容一并驱逐
                // （缓存跨帧永不生效，`geom_misses` 永远等于请求数）。
                if !self.geom_cache.mark_used(&key) {
                    stats.geom_misses += 1;
                    self.geom_cache.insert_handle(
                        key,
                        (),
                        GpuHandle::new(CachedGeometry {
                            data: buffer.data,
                        }),
                    );
                }
                let color = match buffer.kind {
                    TessKind::Fill => style.fill.unwrap_or(Color32::WHITE),
                    TessKind::Stroke => style.stroke.unwrap_or(Color32::BLACK),
                };
                geoms.push((hash, color, style.stroke_width));
            }
        }
        geoms
    }

    /// 趟 2：按 `(color, stroke_width)` 合批 → 每批**一次** draw call。
    ///
    /// 分组保序（首次出现的组按首个成员在绘制序中的位置排列），使同 z 下的
    /// 覆盖关系与合批前逐条绘制一致。
    fn batch_pass(&mut self, geoms: &[(u64, Color32, f32)], clip: Mat4) -> Vec<Prepared2D> {
        let mut groups: Vec<(Color32, f32, Vec<u64>)> = Vec::new();
        for (hash, color, width) in geoms {
            match groups.iter_mut().find(|(c, w, _)| c == color && w == width) {
                Some((_, _, keys)) => keys.push(*hash),
                None => groups.push((*color, *width, vec![*hash])),
            }
        }

        let mut prepared: Vec<Prepared2D> = Vec::with_capacity(groups.len());
        for (color, width, keys) in &groups {
            let bkey = ResourceKey::content::<Batch2DTag>(hash_batch(*color, *width, keys));
            if !self.batch_cache.mark_used(&bkey) {
                // 装配：成员顶点字节按序拼接，成员索引按各自顶点基址**重排**（rebase）。
                // B3-0c：顶点已是打包字节 ⇒ 只拼接，不再理解 `Vertex2D` 结构。
                let mut vertices: Vec<u8> = Vec::new();
                let mut indices: Vec<u32> = Vec::new();
                for &h in keys {
                    let Some(handle) = self
                        .geom_cache
                        .get(ResourceKey::content::<Geometry2DTag>(h))
                    else {
                        continue;
                    };
                    let geom = handle
                        .downcast_ref::<CachedGeometry>()
                        .expect("2D 几何缓存条目必须是 CachedGeometry");
                    let stride = geom.data.layout.stride as usize;
                    let base = (vertices.len() / stride) as u32;
                    vertices.extend_from_slice(&geom.data.vertices);
                    indices.extend(geom.data.indices.iter().map(|i| i + base));
                }
                if indices.is_empty() {
                    continue;
                }
                let vb = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("zela-2d-batch-vertices"),
                        contents: &vertices,
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                let ib = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("zela-2d-batch-indices"),
                        contents: bytes_of(&indices),
                        usage: wgpu::BufferUsages::INDEX,
                    });
                self.batch_cache.insert_handle(
                    bkey,
                    (),
                    GpuHandle::new(BatchBuffers {
                        vertex_buffer: vb,
                        index_buffer: ib,
                        index_count: indices.len() as u32,
                    }),
                );
            }
            let batch = self
                .batch_cache
                .get(bkey)
                .and_then(|h| h.downcast_ref::<BatchBuffers>())
                .expect("合批条目必须存在（刚插入或已命中）");
            // 同批共享一个 uniform：颜色与描边宽度是分组键，clip 是帧级常量。
            let block = self.arena.push(&uniform_bytes(clip, *color, *width));
            prepared.push(Prepared2D {
                vertex_buffer: batch.vertex_buffer.clone(),
                index_buffer: batch.index_buffer.clone(),
                index_count: batch.index_count,
                uniform_offset: block * DYNAMIC_OFFSET_ALIGNMENT as u32,
            });
        }
        prepared
    }

    /// 趟 3（S4.3）：形变项。
    ///
    /// 两端几何（等点重采样后）与 `t` 无关 ⇒ 缓存键**不含 `t`**，稳态零上传；
    /// 逐帧只多写一个 `t` uniform。每个形变项一次 `draw_indexed`（`t` 逐项不同，
    /// 不做跨项合批）。
    ///
    /// 顺序：形变项排在全部 `DrawQueue` 几何**之后**。二者同为 z = 0 的内容，
    /// 网格 / 轴线（z < 0）不受影响；形变与其它 z = 0 内容的重叠次序因此
    /// 变为"形变在上"（详见 `MorphQueue` 的 doc）。
    fn morph_pass(
        &mut self,
        local: &Transform2D,
        tolerance: f32,
        clip: Mat4,
        stats: &mut FrameCacheStats,
    ) -> Vec<Prepared2DMorph> {
        let mut prepared_morph: Vec<Prepared2DMorph> =
            Vec::with_capacity(self.morph_recorder.len());
        for item in mem::take(&mut self.morph_recorder) {
            stats.morph_requests += 1;
            let (from_path, to_path) = item.sampled_endpoints();
            if from_path.commands.is_empty() || to_path.commands.is_empty() {
                continue;
            }
            let style = Style2D {
                space: MeasureSpace::Screen,
                ..item.style
            };
            // B3-0c：形变端点生产（含"退化一侧合成"与拓扑校验）在渲染无关层。
            // 失败 / 拓扑不一致 ⇒ 显式错误（B3-0d：不再 eprintln + continue）。
            let Some(endpoints) = vertex_record::morph_endpoint_buffers(
                &from_path,
                &to_path,
                local,
                &style,
                tolerance,
            )
            .unwrap_or_else(|error| panic!("不静默：形变顶点生产失败：{error}"))
            else {
                // 两侧都没有几何（空路径已在上方挡掉）⇒ 无从合成。
                continue;
            };
            let from_data = &endpoints.from.data;
            let to_data = &endpoints.to.data;
            let mkey = ResourceKey::content::<Morph2DTag>(hash_morph(&endpoints.from, &endpoints.to));
            if !self.morph_cache.mark_used(&mkey) {
                stats.morph_misses += 1;
                let fvb = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("zela-2d-morph-from-vertices"),
                        contents: &from_data.vertices,
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                let tvb = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("zela-2d-morph-to-vertices"),
                        contents: &to_data.vertices,
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                // 两侧拓扑一致 ⇒ 共用 from 的索引表。
                let ib = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("zela-2d-morph-indices"),
                        contents: bytes_of(&from_data.indices),
                        usage: wgpu::BufferUsages::INDEX,
                    });
                self.morph_cache.insert_handle(
                    mkey,
                    (),
                    GpuHandle::new(MorphBuffers {
                        from_vertex_buffer: fvb,
                        to_vertex_buffer: tvb,
                        index_buffer: ib,
                        index_count: from_data.indices.len() as u32,
                    }),
                );
            }
            let morph = self
                .morph_cache
                .get(mkey)
                .and_then(|h| h.downcast_ref::<MorphBuffers>())
                .expect("形变条目必须存在（刚插入或已命中）");
            let color = style.stroke.or(style.fill).unwrap_or(Color32::BLACK);
            let block = self.arena.push(&uniform_bytes_morph(
                clip,
                color,
                style.stroke_width,
                item.t,
            ));
            prepared_morph.push(Prepared2DMorph {
                from_vertex_buffer: morph.from_vertex_buffer.clone(),
                to_vertex_buffer: morph.to_vertex_buffer.clone(),
                index_buffer: morph.index_buffer.clone(),
                index_count: morph.index_count,
                uniform_offset: block * DYNAMIC_OFFSET_ALIGNMENT as u32,
            });
        }
        prepared_morph
    }

    /// 趟 4（R0）：书写动画项。
    ///
    /// 几何 = 路径按 `REVEAL_STEP_PX` 预细分 + 每顶点归一化弧长 `s`；逐帧变的只有
    /// 区间 `[a,b]`（走 uniform）⇒ **缓存键不含区间**，稳态零上传。
    /// 区间逐项不同 ⇒ 同批只能共享一个区间 ⇒ **不进合批**，每项一次 `draw_indexed`
    /// （与形变同一口径）。用的是**同一个** `pipeline_2d`（`shape_2d.wgsl` 内分支），
    /// 即"新增书写动画不改 shader 之外的渲染结构"。
    fn reveal_pass(
        &mut self,
        local: &Transform2D,
        tolerance: f32,
        clip: Mat4,
        stats: &mut FrameCacheStats,
    ) -> Vec<Prepared2DReveal> {
        let mut prepared_reveal: Vec<Prepared2DReveal> =
            Vec::with_capacity(self.reveal_recorder.len());
        for item in mem::take(&mut self.reveal_recorder) {
            stats.reveal_requests += 1;
            let style = Style2D {
                space: MeasureSpace::Screen,
                ..item.style
            };
            // B3-0c：书写动画顶点生产也在渲染无关层。
            let Some(buffer) = vertex_record::tessellate_reveal_to_buffer(
                &item.path,
                local,
                &style,
                tolerance,
                REVEAL_STEP_PX,
            )
            .unwrap_or_else(|error| panic!("不静默：书写动画顶点生产失败：{error}"))
            else {
                continue;
            };
            if buffer.data.is_empty() || buffer.data.indices.is_empty() {
                continue;
            }
            let rkey = ResourceKey::content::<Reveal2DTag>(hash_tessellated(buffer.kind, &buffer.data));
            if !self.reveal_cache.mark_used(&rkey) {
                stats.reveal_misses += 1;
                let vb = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("zela-2d-reveal-vertices"),
                        contents: &buffer.data.vertices,
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                let ib = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("zela-2d-reveal-indices"),
                        contents: bytes_of(&buffer.data.indices),
                        usage: wgpu::BufferUsages::INDEX,
                    });
                self.reveal_cache.insert_handle(
                    rkey,
                    (),
                    GpuHandle::new(RevealBuffers {
                        vertex_buffer: vb,
                        index_buffer: ib,
                        index_count: buffer.data.indices.len() as u32,
                    }),
                );
            }
            let reveal = self
                .reveal_cache
                .get(rkey)
                .and_then(|h| h.downcast_ref::<RevealBuffers>())
                .expect("书写动画条目必须存在（刚插入或已命中）");
            let color = style.stroke.or(style.fill).unwrap_or(Color32::BLACK);
            let block = self.arena.push(&uniform_bytes_reveal(
                clip,
                color,
                style.stroke_width,
                item.start,
                item.end,
            ));
            prepared_reveal.push(Prepared2DReveal {
                vertex_buffer: reveal.vertex_buffer.clone(),
                index_buffer: reveal.index_buffer.clone(),
                index_count: reveal.index_count,
                uniform_offset: block * DYNAMIC_OFFSET_ALIGNMENT as u32,
            });
        }
        prepared_reveal
    }
}

// ─── Painter2DTarget：录制 + 输出离屏纹理 ─────────────────────────────────

impl Painter2DTarget for WgpuBackend2D {
    /// 开始一帧：`logical_size` 视为视口尺寸（points），ppp = 1。
    ///
    /// egui 回调路径用 [`WgpuBackend2D::begin_frame_at`]（带 pixels_per_point / 屏幕尺寸）。
    fn begin_frame(&mut self, logical_size: Vec2) {
        self.begin_frame_at(Frame2DGeometry {
            viewport_points: Rect2D::from_min_size(Vec2::ZERO, logical_size),
            pixels_per_point: 1.0,
            screen_size_px: [
                logical_size.x.max(1.0) as u32,
                logical_size.y.max(1.0) as u32,
            ],
        });
    }

    /// 绘制一个图元（z = 0，即按调用顺序绘制；需要 z 序请用 [`WgpuBackend2D::push`]）。
    fn draw_shape(&mut self, shape: &Shape2D, transform: &Transform2D, style: &Style2D) {
        self.push(shape, transform, style, 0);
    }

    /// 结束一帧：自持 encoder 录制离屏 MSAA pass 并提交（结果留在离屏纹理，
    /// 由 [`WgpuBackend2D::offscreen_view`] 取用）。
    fn end_frame(&mut self) {
        let geometry = self.geometry;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("zela-2d-standalone-encoder"),
            });
        let _frame = self.finish_frame(&mut encoder, LayerId(0), &geometry);
        self.queue.submit([encoder.finish()]);
    }
}

// ─── helpers ──────────────────────────────────────────────────────────────

/// 三角化结果的内容哈希（顶点 + 索引 + 种类），用于几何缓存复用。
fn hash_tessellated(kind: TessKind, data: &VertexBufferData) -> u64 {
    let mut h = hash_bytes(&data.vertices);
    for &i in &data.indices {
        h ^= i as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h ^= match kind {
        TessKind::Fill => 0,
        TessKind::Stroke => 0x9E37_79B9_7F4A_7C15,
    };
    h
}

/// **合批键**：完全决定一批 GPU 缓冲的内容。
///
/// 组成 = 颜色 + 描边宽度（同批共享一个 uniform）+ **有序**成员几何哈希
/// （成员顺序决定缓冲布局，故顺序必须参与哈希）。任一成员内容变化 ⇒ 键变化
/// ⇒ 重新装配；内容不变 ⇒ 命中，零装配零上传。
fn hash_batch(color: Color32, stroke_width: f32, keys: &[u64]) -> u64 {
    let mut buf = Vec::with_capacity(8 + keys.len() * 8);
    buf.extend_from_slice(&[color.r, color.g, color.b, color.a]);
    buf.extend_from_slice(&stroke_width.to_bits().to_le_bytes());
    for &k in keys {
        buf.extend_from_slice(&k.to_le_bytes());
    }
    hash_bytes(&buf)
}

/// **形变缓存键**（S4.3）：只由两侧几何决定，**不含 `t`**。
///
/// 这正是"逐帧只写 t"能成立的原因：`t` 变 ⇒ 键不变 ⇒ 命中 ⇒ 零装配零上传。
/// 用**可三角化的一侧**（`template`）的拓扑，合成**退化侧**的几何：
/// 形变缓存键：两端**已打包缓冲**的内容哈希（与 `t` 无关 ⇒ 稳态命中）。
///
/// B3-0c 起"退化一侧合成"在生产端（`vertex_record::morph_endpoint_buffers`），
/// 后端不再持有 `Tessellation2D` / `Vertex2D` 知识。
fn hash_morph(
    from: &zela_render::vertex_record::TessellatedBuffer,
    to: &zela_render::vertex_record::TessellatedBuffer,
) -> u64 {
    hash_tessellated(from.kind, &from.data).rotate_left(13)
        ^ hash_tessellated(to.kind, &to.data).wrapping_mul(0x100_0000_01b3)
}

/// uniform 字节：mat4 列主序（64）+ 预乘颜色 vec4（16）+ params vec4（16）。
///
/// `params.x` 写 `stroke_width`（偏移 80）——shader 用它沿 `normal` 外扩半个线宽，
/// 把顶点存着的**路径点**还原成轮廓点（R1 几何参数化）。
///
/// `params.w` 写 **1.0**（偏移 92，reveal 区间终点）—— 普通绘制必须写 `[0,1]`
/// （起点 `params.z` 默认 0），否则 `shape_2d.wgsl` 会把整条几何当成"区间外"裁掉。
fn uniform_bytes(
    transform: Mat4,
    color: Color32,
    stroke_width: f32,
) -> [u8; UNIFORM_SIZE as usize] {
    let mut data = [0u8; UNIFORM_SIZE as usize];
    data[..64].copy_from_slice(bytes_of(&transform.to_cols_array()));
    let a = color.a() as f32 / 255.0;
    let c = [
        color.r() as f32 / 255.0 * a,
        color.g() as f32 / 255.0 * a,
        color.b() as f32 / 255.0 * a,
        a,
    ];
    data[64..80].copy_from_slice(bytes_of(&c));
    data[80..84].copy_from_slice(bytes_of(&[stroke_width]));
    // reveal 区间默认全开：z = 0.0（已由零初始化），w = 1.0。
    data[92..96].copy_from_slice(bytes_of(&[1.0f32]));
    data
}

/// 形变 uniform 字节（S4.3）：与 [`uniform_bytes`] 同块，额外写
/// **`params.y = t`**（偏移 84）——`shape_2d_morph.wgsl` 用它做 `mix(from, to, t)`。
fn uniform_bytes_morph(
    transform: Mat4,
    color: Color32,
    stroke_width: f32,
    t: f32,
) -> [u8; UNIFORM_SIZE as usize] {
    let mut data = uniform_bytes(transform, color, stroke_width);
    data[84..88].copy_from_slice(bytes_of(&[t]));
    data
}

/// 书写动画 uniform 字节（R0）：与 [`uniform_bytes`] 同块，额外写
/// **`params.z = a`（偏移 88）/ `params.w = b`（偏移 92）** ——
/// `shape_2d.wgsl` 用它们决定"写"到哪（`advance` 落在区间外的顶点塌缩）。
fn uniform_bytes_reveal(
    transform: Mat4,
    color: Color32,
    stroke_width: f32,
    start: f32,
    end: f32,
) -> [u8; UNIFORM_SIZE as usize] {
    let mut data = uniform_bytes(transform, color, stroke_width);
    data[88..92].copy_from_slice(bytes_of(&[start]));
    data[92..96].copy_from_slice(bytes_of(&[end]));
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    // B3-0c：生产函数移到 `zela_render`；测试仍直接调用三角化做几何断言。
    use zela_render::tessellate;
    use zela_render::{DrawCommand, ZIndex};
    use zela_scene::domain::LayerBlend;

    use crate::compositor::{CompositeOp, Compositor};

    fn geometry(w: f32, h: f32, ppp: f32, screen: [u32; 2]) -> Frame2DGeometry {
        Frame2DGeometry {
            viewport_points: Rect2D::from_min_size(Vec2::new(100.0, 50.0), Vec2::new(w, h)),
            pixels_per_point: ppp,
            screen_size_px: screen,
        }
    }

    /// 视口像素矩形与 `epaint::ViewportInPixels::from_points` 同口径（取整 + 钳制）。
    #[test]
    fn viewport_px_matches_epaint_rounding() {
        let g = geometry(300.0, 200.0, 1.5, [1600, 1200]);
        let (left, top, w, h) = g.viewport_px();
        assert_eq!(left, (1.5f32 * 100.0).round() as i32);
        assert_eq!(top, (1.5f32 * 50.0).round() as i32);
        assert_eq!(
            w,
            ((1.5f32 * 400.0).round() - (1.5f32 * 100.0).round()) as u32
        );
        assert_eq!(
            h,
            ((1.5f32 * 250.0).round() - (1.5f32 * 50.0).round()) as u32
        );
    }

    /// 视口超出屏幕时被钳制（不产生越界尺寸）。
    #[test]
    fn viewport_px_clamps_to_screen() {
        let g = geometry(1000.0, 1000.0, 1.0, [640, 480]);
        let (left, top, w, h) = g.viewport_px();
        assert_eq!((left, top), (100, 50));
        assert_eq!((w, h), (540, 430));
    }

    /// 离屏尺寸 = 视口像素尺寸，且至少 1×1。
    #[test]
    fn offscreen_size_is_at_least_one() {
        let g = Frame2DGeometry {
            viewport_points: Rect2D::from_min_size(Vec2::ZERO, Vec2::new(0.0, 0.0)),
            pixels_per_point: 1.0,
            screen_size_px: [100, 100],
        };
        assert_eq!(g.offscreen_size(), (1, 1));
    }

    /// 本地变换：先减视口原点（points），再乘 ppp → 离屏像素。
    #[test]
    fn local_transform_maps_viewport_origin_to_zero() {
        let g = geometry(300.0, 200.0, 2.0, [1600, 1200]);
        let t = g.local_transform();
        let p = t.apply_point(Vec2::new(100.0, 50.0));
        assert!((p.x).abs() < 1e-4 && (p.y).abs() < 1e-4);
        let q = t.apply_point(Vec2::new(200.0, 150.0));
        assert!((q.x - 200.0).abs() < 1e-4 && (q.y - 200.0).abs() < 1e-4);
    }

    /// 裁剪矩阵把离屏左下/右上映射到 NDC（含 Y 翻转）。
    #[test]
    fn clip_matrix_maps_offscreen_corners() {
        let g = geometry(300.0, 200.0, 1.0, [1600, 1200]);
        let m = g.clip_matrix();
        let (w, h) = g.offscreen_size();
        let nw = m * ze_notion::Vec4::new(0.0, 0.0, 0.0, 1.0);
        let se = m * ze_notion::Vec4::new(w as f32, h as f32, 0.0, 1.0);
        assert!((nw.x + 1.0).abs() < 1e-5 && (nw.y - 1.0).abs() < 1e-5);
        assert!((se.x - 1.0).abs() < 1e-5 && (se.y + 1.0).abs() < 1e-5);
    }

    /// uniform 字节：mat4 + 预乘颜色（alpha=255 时颜色不变）+ params.x = 描边宽度。
    #[test]
    fn uniform_bytes_layout() {
        let b = uniform_bytes(Mat4::IDENTITY, Color32::new(10, 20, 30, 255), 3.0);
        assert_eq!(b.len(), UNIFORM_SIZE as usize);
        let f32_at = |off: usize| -> f32 {
            f32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
        };
        // 单位矩阵：对角（字节 0/20/40/60）为 1，非对角为 0。
        assert_eq!(f32_at(0), 1.0);
        assert_eq!(f32_at(20), 1.0);
        assert_eq!(f32_at(40), 1.0);
        assert_eq!(f32_at(60), 1.0);
        assert_eq!(f32_at(16), 0.0);
        // 颜色（alpha = 1 → 不改变 RGB）。
        assert!((f32_at(64) - 10.0 / 255.0).abs() < 1e-6);
        assert!((f32_at(68) - 20.0 / 255.0).abs() < 1e-6);
        assert!((f32_at(72) - 30.0 / 255.0).abs() < 1e-6);
        assert!((f32_at(76) - 1.0).abs() < 1e-6);
        // alpha = 0.5 → 预乘。
        let b2 = uniform_bytes(Mat4::IDENTITY, Color32::new(200, 0, 0, 128), 0.0);
        let a = f32_at_of(&b2, 76);
        assert!((f32_at_of(&b2, 64) - 200.0 / 255.0 * a).abs() < 1e-6);
    }

    /// 🔴 R3：描边宽度写在 **偏移 80**（`params.x`），且宽度 0 也不越界。
    ///
    /// 半宽/全宽或偏移算错的话，实机表现为线条偏细/偏粗一倍——这里把它钉死在字节层。
    /// R0 后 `params.w`（偏移 92）是 **reveal 区间终点**，普通绘制必须为 1.0
    /// （否则 `shape_2d.wgsl` 会把整条几何裁掉）。
    #[test]
    fn uniform_stroke_width_lands_at_offset_80() {
        for w in [0.0f32, 1.0, 2.0, 8.0] {
            let b = uniform_bytes(Mat4::IDENTITY, Color32::WHITE, w);
            // params.x（偏移 80）必须是原样写入的宽度。
            assert_eq!(f32_at_of(&b, 80), w, "params.x 应为 stroke_width={w}");
            // y = 形变参数（本路径不写）、z = reveal 起点，均为 0。
            assert_eq!(f32_at_of(&b, 84), 0.0);
            assert_eq!(f32_at_of(&b, 88), 0.0);
            // w = reveal 终点 → 必须为 1.0（全可见）。
            assert_eq!(f32_at_of(&b, 92), 1.0, "params.w 必须是 1.0（区间全开）");
        }
    }

    /// R0：reveal 区间写在 **params.z（88）/ params.w（92）**，且与形变的 `t`（84）不冲突。
    #[test]
    fn uniform_reveal_range_lands_at_offsets_88_and_92() {
        let b = uniform_bytes_reveal(Mat4::IDENTITY, Color32::WHITE, 2.0, 0.25, 0.75);
        assert_eq!(f32_at_of(&b, 80), 2.0, "params.x 仍是描边宽度");
        assert_eq!(f32_at_of(&b, 84), 0.0, "params.y（形变 t）不受影响");
        assert_eq!(f32_at_of(&b, 88), 0.25, "params.z = reveal 起点");
        assert_eq!(f32_at_of(&b, 92), 0.75, "params.w = reveal 终点");
        // 与形变共用同一块：`t` 与区间互不覆盖。
        let m = uniform_bytes_morph(Mat4::IDENTITY, Color32::WHITE, 2.0, 0.5);
        assert_eq!(f32_at_of(&m, 84), 0.5, "params.y = t");
        assert_eq!(f32_at_of(&m, 92), 1.0, "形变不带 reveal ⇒ 区间全开");
    }

    fn f32_at_of(bytes: &[u8], off: usize) -> f32 {
        f32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
    }

    /// 队列快照：按 z 稳定排序（同 z 保录制顺序）。
    #[test]
    fn snapshot_is_stably_sorted_by_z() {
        let mut q = DrawQueue::new();
        q.push(DrawCommand::new(
            ZIndex::new(5),
            Shape2D::Empty,
            Style2D::default(),
        ));
        q.push(DrawCommand::new(
            ZIndex::new(1),
            Shape2D::Line {
                start: Vec2::ZERO,
                end: Vec2::new(1.0, 1.0),
            },
            Style2D::default(),
        ));
        q.push(DrawCommand::new(
            ZIndex::new(5),
            Shape2D::Empty,
            Style2D::default(),
        ));
        let snap = snapshot_queue(&q);
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].z, 1);
        // 同 z 的两条保持录制顺序：第 1、3 条都是 Empty。
        assert_eq!(snap[1].z, 5);
        assert_eq!(snap[2].z, 5);
        assert!(matches!(snap[1].shape, Shape2D::Empty));
    }

    /// MSAA 采样数常量（验收项）。
    #[test]
    fn msaa_is_4x() {
        assert_eq!(MSAA_SAMPLES, 4);
    }

    /// **端到端回归**（"2D 视图在 X 方向被压扁"）：在真实 device 上复刻
    /// egui-wgpu 对回调的处理方式 —— 主 pass 的 viewport 被设成回调矩形
    /// （`set_viewport(视口 px)`）—— 再交给合成器把离屏纹理合成到主 pass，
    /// 然后回读**主目标**像素，断言 2D 内容落在视口内的正确位置：
    ///
    /// - 视口像素矩形 (100,50)-(400,250)，离屏尺寸 = 视口尺寸；
    /// - 图元：屏幕矩形 (150,75)-(250,125)（= 离屏本地 (50,25)-(150,75)）；
    /// - 修复前：quad 用整屏尺寸换算 → 内容被压成 `300/1600` 宽并右移。
    ///
    /// 主 pass 清成不透明黑；2D 离屏为透明底（S3-2/C2），故"视口内、矩形外"
    /// 与"视口外"都保持黑 —— 这正好证明图层外是透明的、没有被离屏卡片盖死。
    #[test]
    fn composite_lands_inside_scoped_viewport_on_real_device() {
        // 用非 sRGB 的 RGBA 主目标：合成走 Identity 入口（字节直通），
        // 这样下面的颜色断言可以是逐字节精确的。
        let Ok(ctx) = crate::RenderContext::headless_with_format(wgpu::TextureFormat::Rgba8Unorm)
        else {
            return; // 无 GPU 环境：跳过
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);

        let screen = [1600u32, 1200u32];
        let g = geometry(300.0, 200.0, 1.0, screen);
        let (left, top, vw, vh) = g.viewport_px();
        assert_eq!((left, top, vw, vh), (100, 50, 300, 200));

        let mut q = DrawQueue::new();
        q.push(DrawCommand::new(
            ZIndex::new(0),
            Shape2D::Rect(Rect2D::new(Vec2::new(150.0, 75.0), Vec2::new(250.0, 125.0))),
            Style2D {
                fill: Some(Color32::new(255, 0, 0, 255)),
                space: MeasureSpace::Screen,
                ..Default::default()
            },
        ));
        let draws = snapshot_queue(&q);

        // 主 pass 目标（模拟 eframe 的帧缓冲）。
        let main = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("zela-2d-test-main-target"),
            size: wgpu::Extent3d {
                width: screen[0],
                height: screen[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ctx.target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let main_view = main.create_view(&wgpu::TextureViewDescriptor::default());

        backend.begin_frame_at(g);
        for d in &draws {
            backend.push(&d.shape, &d.transform, &d.style, d.z);
        }
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("zela-2d-test-e2e"),
            });
        let frame = backend.finish_frame(&mut enc, LayerId(0), &g);
        let mut compositor = Compositor::new(&ctx.device, ctx.target_format);
        let op = CompositeOp {
            texture: frame.offscreen.clone(),
            src_format: OFFSCREEN_FORMAT,
            blend: LayerBlend::Alpha,
            opacity: 1.0,
            rect_px: (left as f32, top as f32, vw as f32, vh as f32),
        };
        let ops = [op];
        // C3：管线惰性建 ⇒ 合成前先确保本帧用到的组合已就绪（1 = 首帧现场建一条）。
        assert_eq!(
            compositor.ensure_for(&ctx.device, &ops),
            1,
            "首帧应现场建 1 条"
        );
        assert_eq!(
            compositor.ensure_for(&ctx.device, &ops),
            0,
            "同组合第二帧应命中缓存（不重复建）"
        );
        let composite = compositor.prepare(&ctx.device, &ops);
        {
            let mut pass = enc
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("zela-2d-test-main-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &main_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            // egui-wgpu 的 "courtesy" 行为：回调 rect → render pass viewport。
            pass.set_viewport(left as f32, top as f32, vw as f32, vh as f32, 0.0, 1.0);
            pass.set_scissor_rect(left as u32, top as u32, vw, vh);
            compositor.composite_layers(&mut pass, &composite);
        }
        ctx.queue.submit([enc.finish()]);
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("提交后 poll");

        let px = read_texture(&ctx, &main, (screen[0], screen[1]));
        let at = |x: u32, y: u32| {
            let o = ((y * screen[0] + x) * 4) as usize;
            [px[o], px[o + 1], px[o + 2], px[o + 3]]
        };
        const BG: [u8; 4] = [0, 0, 0, 255];
        // 视口内、矩形内 → 红。
        assert_eq!(at(200, 100), [255, 0, 0, 255], "图元应在视口内的正确位置");
        // 视口内、矩形外 → 离屏为透明，混到黑底上仍是黑（图层卡片不可见）。
        assert_eq!(at(110, 60), BG, "视口左上角不应出现不透明卡片");
        assert_eq!(at(390, 240), BG, "视口右下角不应出现不透明卡片");
        // 视口外 → 主目标自身的清屏色（黑），证明没有越界。
        assert_eq!(at(50, 50), BG, "视口外不应被绘制");
        assert_eq!(at(500, 400), BG, "视口外不应被绘制");
        // 水平边界（±2px 容差）：矩形左边 150、右边 250。
        assert_eq!(at(152, 100), [255, 0, 0, 255]);
        assert_eq!(at(148, 100), BG, "矩形左侧应仍是背景");

        assert_eq!(at(248, 100), [255, 0, 0, 255]);
        assert_eq!(at(252, 100), BG, "矩形右侧应仍是背景");
    }

    /// 从纹理回读像素（未填充的紧凑 `w*4` 行距；解码交给调用方）。
    ///
    /// W1：256 对齐的行距处理已提升为 [`crate::RenderContext::read_texture`]，
    /// 本处只保留薄包装（调用点不变）。
    fn read_texture(
        ctx: &crate::RenderContext,
        texture: &wgpu::Texture,
        size: (u32, u32),
    ) -> Vec<u8> {
        ctx.read_texture(texture, size)
    }

    /// GPU 冒烟（真实 device）：完整跑通"三角化 → 几何上传 → uniform arena →
    /// 离屏 MSAA 4x pass → resolve"，回读离屏像素证明图元落在**预期位置**且颜色
    /// 与 uniform 一致（不只"没报错"）；再用变化的视口尺寸跑第二帧验证离屏目标
    /// 按尺寸重建（不拉伸、不越界——尺寸不匹配会被 wgpu 校验直接拒掉）。
    ///
    /// 无可用适配器（headless 构造失败）时跳过，与 `render_context` 既有约定一致。
    #[test]
    fn gpu_frame_rasterizes_and_recreates_targets_on_resize() {
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
        else {
            return; // 无 GPU 环境：跳过
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);

        // 视口 points (100,50)-(300,150)，ppp = 1 → 离屏 200×100 像素；
        // 屏幕坐标 → 离屏本地坐标 = 平移 (-100,-50)。
        let mut q = DrawQueue::new();
        q.push(DrawCommand::new(
            ZIndex::new(0),
            Shape2D::Rect(Rect2D::new(Vec2::new(150.0, 75.0), Vec2::new(250.0, 125.0))),
            Style2D {
                fill: Some(Color32::new(255, 0, 0, 255)),
                space: MeasureSpace::Screen,
                ..Default::default()
            },
        ));
        q.push(DrawCommand::new(
            ZIndex::new(1),
            Shape2D::Circle {
                center: Vec2::new(150.0, 100.0),
                radius: 10.0,
            },
            Style2D {
                // `Style2D::default()` 的 fill 是白色，这里显式关掉以只留描边
                // （让 draw_count 的断言能精确区分"填充/描边各一条几何"）。
                fill: None,
                stroke: Some(Color32::new(0, 255, 0, 255)),
                stroke_width: 4.0,
                space: MeasureSpace::Screen,
                ..Default::default()
            },
        ));
        let draws = snapshot_queue(&q);

        // ── 第一帧：200×100 ──
        let g = geometry(200.0, 100.0, 1.0, [400, 200]);
        backend.begin_frame_at(g);
        for d in &draws {
            backend.push(&d.shape, &d.transform, &d.style, d.z);
        }
        assert_eq!(backend.recorded_len(), 2);
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("zela-2d-test-frame-1"),
            });
        let frame = backend.finish_frame(&mut enc, LayerId(0), &g);
        assert_eq!(frame.draw_count, 2, "填充 + 描边各一条几何");
        ctx.queue.submit([enc.finish()]);
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("第一帧 submit 后 poll");

        let px = read_texture(
            &ctx,
            backend.offscreen_texture(LayerId(0)).unwrap(),
            (200, 100),
        );
        let at = |x: u32, y: u32| {
            let o = ((y * 200 + x) * 4) as usize;
            [px[o], px[o + 1], px[o + 2], px[o + 3]]
        };
        // 离屏 (100,50)：红色矩形内部（本地 (50,25)-(150,75)）。
        assert_eq!(at(100, 50), [255, 0, 0, 255], "矩形填充处应为红色");
        // 离屏 (60,50)：圆描边带内（本地圆心 (50,50)，半径 10、宽 4 → 带 [8,12]）。
        assert_eq!(at(60, 50), [0, 255, 0, 255], "圆描边处应为绿色");
        // 未绘制处 = 透明清屏（S3-2/C2 起离屏透明，背景由 host 提供）。
        assert_eq!(at(10, 10), [0, 0, 0, 0], "未绘制处应为透明");
        assert_eq!(at(199, 99), [0, 0, 0, 0]);

        // ── 第二帧：视口变 400×300 → 离屏目标重建 ──
        let g2 = geometry(400.0, 300.0, 1.0, [800, 600]);
        backend.begin_frame_at(g2);
        for d in &draws {
            backend.push(&d.shape, &d.transform, &d.style, d.z);
        }
        let mut enc2 = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("zela-2d-test-frame-2"),
            });
        let frame2 = backend.finish_frame(&mut enc2, LayerId(0), &g2);
        assert_eq!(frame2.draw_count, 2);
        assert_eq!(backend.offscreen_texture(LayerId(0)).unwrap().width(), 400);
        assert_eq!(backend.offscreen_texture(LayerId(0)).unwrap().height(), 300);
        ctx.queue.submit([enc2.finish()]);
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("第二帧 submit 后 poll");

        // 本地坐标不随视口变化（视口原点平移恒定），故内容位置与第一帧一致。
        let px2 = read_texture(
            &ctx,
            backend.offscreen_texture(LayerId(0)).unwrap(),
            (400, 300),
        );
        let at2 = |x: u32, y: u32| {
            let o = ((y * 400 + x) * 4) as usize;
            [px2[o], px2[o + 1], px2[o + 2], px2[o + 3]]
        };
        assert_eq!(at2(100, 50), [255, 0, 0, 255]);
        // 新增区域内侧边界仍为透明清屏（没有拉伸/残留）。
        assert_eq!(at2(390, 290), [0, 0, 0, 0]);
        assert_eq!(at2(10, 10), [0, 0, 0, 0]);
    }

    /// 2D 几何管线在两种主 pass 目标格式下都能建出来并跑通（eframe 的目标格式
    /// 依平台/配置而定，少覆盖一支就等于在别的机器上启动即 panic）；
    /// 合成管线的双入口由 `compositor` 侧覆盖。
    #[test]
    fn gpu_pipeline_builds_for_srgb_and_non_srgb_targets() {
        let Some((_gpu, ctx)) = crate::gpu_test::require_gpu() else {
            return;
        };
        for format in [
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba8Unorm,
        ] {
            let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
            // 合成器对两种目标格式都必须能建出管线。
            let compositor = Compositor::new(&ctx.device, format);
            let _ = &compositor;
            let g = geometry(64.0, 64.0, 1.0, [128, 128]);
            backend.begin_frame_at(g);
            backend.push(
                &Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(64.0, 64.0))),
                &Transform2D::identity(),
                &Style2D {
                    fill: Some(Color32::new(0, 0, 255, 255)),
                    space: MeasureSpace::Screen,
                    ..Default::default()
                },
                0,
            );
            let mut enc = ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("zela-2d-format-smoke"),
                });
            let frame = backend.finish_frame(&mut enc, LayerId(0), &g);
            assert_eq!(frame.draw_count, 1, "{format:?} 下应有一条几何");
            ctx.queue.submit([enc.finish()]);
            ctx.device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("提交后 poll");
        }
    }

    /// 🔴 B3-0d：三角化失败**不再 `eprintln + continue`**，而是显式 panic
    /// （错误路径可达、有测试；铁律 2「不静默」）。
    #[test]
    #[should_panic(expected = "不静默：2D 顶点生产失败")]
    fn gpu_unregistered_custom_shape_panics_explicitly() {
        let Some((_gpu, ctx)) = crate::gpu_test::require_gpu() else {
            return;
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
        let g = geometry(64.0, 64.0, 1.0, [128, 128]);
        backend.begin_frame_at(g);
        backend.push(
            &Shape2D::Custom(zela_render::CustomShapeData::new(
                zela_render::SpecId::new("test.missing"),
                Rect2D::new(Vec2::ZERO, Vec2::new(10.0, 10.0)),
            )),
            &Transform2D::identity(),
            &Style2D {
                fill: Some(Color32::WHITE),
                space: MeasureSpace::Screen,
                ..Default::default()
            },
            0,
        );
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("zela-2d-explicit-error"),
            });
        let _ = backend.finish_frame(&mut enc, LayerId(0), &g);
    }

    /// 🔴 R1 的业务效果 / R5-1：几何内容（顶点数 + index 表 + 路径点）**与描边宽度无关**。
    ///
    /// 宽度只走 uniform（`params.x`），所以 2D 的缓存决策只认内容哈希，
    /// `VertexLayout.cache` 仅作声明（换标签不改变任何缓存行为）。
    ///
    /// # 为什么这里不断言「哈希逐字节相等」
    ///
    /// lyon 把 `normal` 定义为 `(position - position_on_path) / half_width`
    /// （`lyon_tessellation/src/stroke.rs:2037`），即"偏移量 ÷ 半宽"：换宽度就换除数，
    /// f32 末位舍入随之变化（实测相对差 ~1e-7，曲线与直线皆然）。
    /// 因此跨宽度的 `normal` **字节不相等是常态**，`hash_tessellation` 亦可能因此不同。
    /// 代价仅为：同一路径改宽度时多重建一次几何（重新 miss）；
    /// **永不产生画面差异** —— 渲染用该条目自己的 `normal × 宽度`，
    /// 与旧 CPU 管线（lyon 直接烘进几何）同源（数值证明见
    /// `zela_render::tessellate::tests::shader_offset_reproduces_lyon_contour_points`）。
    #[test]
    fn geometry_topology_is_independent_of_width() {
        let shape = Shape2D::Line {
            start: Vec2::new(0.0, 0.0),
            end: Vec2::new(30.0, 10.0),
        };
        let thin = tessellate(&shape, &Transform2D::identity(), &stroke_style(2.0), 0.05)
            .expect("三角化");
        let thick = tessellate(&shape, &Transform2D::identity(), &stroke_style(8.0), 0.05)
            .expect("三角化");
        assert_eq!(thin.len(), 1);
        assert_eq!(thick.len(), 1);
        let (a, b) = (&thin[0], &thick[0]);
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.vertices.len(), b.vertices.len(), "顶点数必须与宽度无关");
        assert_eq!(a.indices, b.indices, "index 表必须与宽度无关");
        for (i, (x, y)) in a.vertices.iter().zip(&b.vertices).enumerate() {
            assert_eq!(
                x.position, y.position,
                "第 {i} 个顶点的路径点必须逐字节相同"
            );
            assert_eq!(x.side, y.side, "第 {i} 个顶点的 side 必须逐字节相同");
            assert!(
                (x.normal[0] - y.normal[0]).abs() < 1e-5
                    && (x.normal[1] - y.normal[1]).abs() < 1e-5,
                "第 {i} 个顶点的法线应只差 f32 末位（lyon 口径）：{:?} vs {:?}",
                x.normal,
                y.normal
            );
        }
    }

    fn stroke_style(width: f32) -> Style2D {
        Style2D {
            fill: None,
            stroke: Some(Color32::new(0, 255, 0, 255)),
            stroke_width: width,
            space: MeasureSpace::Screen,
            ..Default::default()
        }
    }

    /// 🔴 R5-2 / R5-3（真实 device，G7 回校）：连续 60 帧让"动画内容"每帧都变
    /// （形变的圆 + 平移的线段），断言四条不变量：
    /// 1. 几何缓存存活条目恒 = 本帧条目数（内容哈希逐帧全 miss、上一帧全被驱逐）；
    /// 2. `pending_geometry_destroy()` 稳态**不增长**（graveyard 有界，
    ///    上界 = `graveyard_period × 每帧条目数`）；
    /// 3. arena `used_blocks` 恒 = 本帧 draw 数 —— 这是 `begin_frame` **每帧重置**的直接证据
    ///    （若没重置，它会逐帧单调增长 2/4/6/…）；
    /// 4. arena `capacity_blocks` 扩容一次后**稳定**。
    ///
    /// 无可用适配器（headless 构造失败）时跳过。
    #[test]
    fn animated_content_keeps_cache_and_arena_bounded() {
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
        else {
            return; // 无 GPU 环境：跳过
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
        // 视口 points (100,50)-(300,150)，ppp = 1 → 离屏 200×100 像素。
        let g = geometry(200.0, 100.0, 1.0, [400, 200]);

        const FRAMES: u32 = 60;
        const DRAWS: u32 = 2;
        let mut pending_at_frame_20 = 0usize;
        let mut capacity_after_first = 0u32;
        for frame in 0..FRAMES {
            backend.begin_frame_at(g);
            // 形变的圆：半径每帧变 → 顶点内容哈希每帧不同（全 miss → 全重建）。
            backend.push(
                &Shape2D::Circle {
                    center: Vec2::new(200.0, 100.0),
                    radius: 8.0 + frame as f32 * 0.25,
                },
                &Transform2D::identity(),
                &stroke_style(2.0),
                0,
            );
            // 平移的线段：形状不变、位置每帧变 → 同样每帧 miss。
            backend.push(
                &Shape2D::Line {
                    start: Vec2::new(150.0 + frame as f32, 60.0),
                    end: Vec2::new(250.0 + frame as f32, 60.0),
                },
                &Transform2D::identity(),
                &stroke_style(1.0),
                1,
            );

            let mut enc = ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("zela-2d-bounded-frame"),
                });
            let out = backend.finish_frame(&mut enc, LayerId(0), &g);
            assert_eq!(
                out.draw_count, DRAWS as usize,
                "第 {frame} 帧：draw 数应为 {DRAWS}"
            );
            ctx.queue.submit([enc.finish()]);
            ctx.device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("帧提交后 poll 失败");

            assert_eq!(
                backend.geometry_cache_len(),
                DRAWS as usize,
                "第 {frame} 帧：缓存条目应恒 = 本帧条目数"
            );
            assert_eq!(
                backend.arena_usage().0,
                DRAWS,
                "第 {frame} 帧：arena 占用应恒 = 本帧 draw 数（每帧重置）"
            );
            if frame == 0 {
                capacity_after_first = backend.arena_usage().1;
            }
            if frame == 20 {
                pending_at_frame_20 = backend.pending_geometry_destroy();
            }
        }

        assert_eq!(
            backend.geometry_cache_len(),
            DRAWS as usize,
            "条目数不得随帧数增长"
        );
        assert_eq!(
            backend.pending_geometry_destroy(),
            pending_at_frame_20,
            "graveyard pending 稳态不得增长"
        );
        assert_eq!(
            backend.arena_usage().1,
            capacity_after_first,
            "arena 容量扩容一次后应稳定（不单调增长）"
        );
    }

    /// 🔴 S4.2 可证伪断言：同 `(color, stroke_width)` 的多条几何被合并成**一次**
    /// draw call —— `batch_count < draw_count`。
    ///
    /// 证伪：去掉趟 2 的合批（逐条 `Prepared2D`）后 `batch_count == draw_count`，
    /// 本断言必然失败。
    #[test]
    fn same_color_geometries_batch_into_one_draw_call() {
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
        else {
            return; // 无 GPU 环境：跳过
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
        let g = geometry(200.0, 100.0, 1.0, [400, 200]);

        backend.begin_frame_at(g);
        // 6 条同色同宽的线（几何各不相同 → 6 条几何，应合并为 1 批）。
        for i in 0..6 {
            let y = 60.0 + i as f32 * 6.0;
            backend.push(
                &Shape2D::Line {
                    start: Vec2::new(120.0, y),
                    end: Vec2::new(280.0, y),
                },
                &Transform2D::identity(),
                &stroke_style(2.0),
                0,
            );
        }
        // 1 条异色线（自成一 批）。
        backend.push(
            &Shape2D::Line {
                start: Vec2::new(120.0, 95.0),
                end: Vec2::new(280.0, 95.0),
            },
            &Transform2D::identity(),
            &Style2D {
                fill: None,
                stroke: Some(Color32::new(255, 0, 0, 255)),
                stroke_width: 2.0,
                space: MeasureSpace::Screen,
                ..Default::default()
            },
            1,
        );

        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("zela-2d-batch-test"),
            });
        let frame = backend.finish_frame(&mut enc, LayerId(0), &g);
        ctx.queue.submit([enc.finish()]);

        assert_eq!(frame.draw_count, 7, "7 条几何");
        assert_eq!(frame.batch_count, 2, "同色 6 条合并为 1 批 + 异色 1 批");
        assert!(
            frame.batch_count < frame.draw_count,
            "合批生效时 batch_count({}) 必须小于 draw_count({})",
            frame.batch_count,
            frame.draw_count
        );
        assert_eq!(backend.batch_cache_len(), 2, "两批各一个合批缓冲条目");
    }

    /// 🔴 S4.2 缓存可观测：稳态（内容逐帧不变）下 `geom_misses` **不随帧数增长**
    /// （第 2 帧起恒为 0），且几何/合批条目数稳定。
    ///
    /// 证伪：若命中不刷新 `last_used_frame`（或帧末驱逐策略变严），条目会在帧末
    /// 被驱逐 → `geom_misses` 恒等于 `geom_requests`，本断言失败。
    #[test]
    fn steady_state_geometry_cache_misses_stop_growing() {
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
        else {
            return; // 无 GPU 环境：跳过
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
        let g = geometry(200.0, 100.0, 1.0, [400, 200]);

        const FRAMES: usize = 10;
        let mut misses: Vec<usize> = Vec::with_capacity(FRAMES);
        for _ in 0..FRAMES {
            backend.begin_frame_at(g);
            backend.push(
                &Shape2D::Circle {
                    center: Vec2::new(200.0, 100.0),
                    radius: 20.0,
                },
                &Transform2D::identity(),
                &stroke_style(2.0),
                0,
            );
            let mut enc = ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("zela-2d-steady-frame"),
                });
            let out = backend.finish_frame(&mut enc, LayerId(0), &g);
            ctx.queue.submit([enc.finish()]);
            assert_eq!(out.cache.geom_requests, 1);
            assert_eq!(out.batch_count, 1);
            misses.push(out.cache.geom_misses);
        }

        assert_eq!(misses[0], 1, "首帧必然未命中（需三角化 + 建条目）");
        assert!(
            misses[1..].iter().all(|&m| m == 0),
            "稳态下不得再出现 miss（miss 数不随帧数增长）：{misses:?}"
        );
        assert_eq!(backend.geometry_cache_len(), 1);
        assert_eq!(backend.batch_cache_len(), 1);
    }

    /// 🔴 S4.3 可证伪断言：形变由 **`t` uniform** 驱动，几何帧间不变。
    ///
    /// 1. `t = 0` 光栅化出 `from` 的边界、`t = 1` 光栅化出 `to` 的边界
    ///    —— 若 `t` 没进 uniform（或没插值），各帧像素会完全相同，断言失败；
    /// 2. 逐帧改 `t` 时 `morph_misses` 恒为 0（几何与 `t` 无关 ⇒ 缓存命中）
    ///    —— 若把 `t` 混进缓存键，`morph_misses` 会逐帧 +1，断言失败。
    #[test]
    fn morph_t_uniform_drives_geometry_and_stays_cached() {
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
        else {
            return; // 无 GPU 环境：跳过
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
        let g = geometry(200.0, 100.0, 1.0, [400, 200]);

        // 屏幕坐标下的同心方框（视口 (100,50) 起点 ⇒ 屏幕 (200,100) = 离屏本地 (100,50)）：
        // half 由 20 → 40 形变。
        let center = Vec2::new(200.0, 100.0);
        let square = |half: f32| {
            ze_notion::Path2D::from_polyline(
                &[
                    Vec2::new(center.x - half, center.y - half),
                    Vec2::new(center.x + half, center.y - half),
                    Vec2::new(center.x + half, center.y + half),
                    Vec2::new(center.x - half, center.y + half),
                ],
                true,
            )
        };
        let from = square(20.0);
        let to = square(40.0);
        let style = Style2D {
            fill: None,
            stroke: Some(Color32::new(255, 180, 0, 255)),
            stroke_width: 4.0,
            space: MeasureSpace::Screen,
            ..Default::default()
        };
        // 离屏纹理格式是 `OFFSCREEN_FORMAT = Rgba8Unorm` ⇒ 内存字节序 [R, G, B, A]。
        let orange = [255u8, 180, 0, 255];

        let mut misses: Vec<usize> = Vec::new();
        for (frame, t) in [0.0f32, 0.25, 0.5, 0.75, 1.0].into_iter().enumerate() {
            backend.begin_frame_at(g);
            backend.push_morph(MorphItem {
                from: from.clone(),
                to: to.clone(),
                t,
                samples: 4,
                style,
            });
            assert_eq!(backend.recorded_morph_len(), 1);

            let mut enc = ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("zela-2d-morph-frame"),
                });
            let out = backend.finish_frame(&mut enc, LayerId(0), &g);
            ctx.queue.submit([enc.finish()]);
            ctx.device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("形变帧提交后 poll");

            assert_eq!(out.cache.morph_requests, 1);
            assert_eq!(out.batch_count, 1, "一个形变项 = 一次 draw call");
            misses.push(out.cache.morph_misses);

            let px = read_texture(
                &ctx,
                backend.offscreen_texture(LayerId(0)).unwrap(),
                (200, 100),
            );
            let at = |x: u32, y: u32| {
                let o = ((y * 200 + x) * 4) as usize;
                [px[o], px[o + 1], px[o + 2], px[o + 3]]
            };
            // 右上边界随 t 线性外移：half = 20 + 20t（全部落在整数像素上 ⇒ 命中带心）。
            let edge = (100.0 + 20.0 + 20.0 * t).round() as u32;
            assert_eq!(
                at(edge, 50),
                orange,
                "第 {frame} 帧（t = {t}）：局部 x = {edge} 处应命中描边"
            );
            assert_eq!(
                at(100, 50),
                [0, 0, 0, 0],
                "第 {frame} 帧（t = {t}）：方框内部应透明（仅描边）"
            );
        }

        assert_eq!(misses[0], 1, "首帧建形变条目（必然未命中）");
        assert!(
            misses[1..].iter().all(|&m| m == 0),
            "`t` 变化不得导致缓存未命中（几何与 t 无关）：{misses:?}"
        );
        assert_eq!(backend.morph_cache_len(), 1);
    }

    /// **分镜 3 的形态**（P5）：`from` 是**退化点**（全部点重合），`to` 是完整折线。
    ///
    /// 这是"曲线从左到右生长"的实现方式（`storyboard.rs`）。它比"同心方框"更极端
    /// ——零长度路径进 lyon **不产出几何**，必须实测（否则形变项会被整条跳过、
    /// 画面空白，而队列层看不出问题）。
    ///
    /// 可证伪断言（按面积随 `t` 单调增长判定，不依赖具体像素坐标）：
    /// 1. `t = 0` 时**完全不可见**（三角形退化）——"从一点长出"的起点；
    /// 2. `t = 0.5` 与 `t = 1.0` 都**已画出**，且覆盖像素数严格递增
    ///    —— 若退化 `from` 被跳过（`batch_count == 0`）或像素不随 `t` 变化，断言失败。
    #[test]
    fn morph_from_degenerate_point_grows_into_target_line() {
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
        else {
            return; // 无 GPU 环境：跳过
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
        let g = geometry(200.0, 100.0, 1.0, [400, 200]);

        // 屏幕坐标下的浅弧（非共线 ⇒ 环有面积，绕向明确）；离屏本地 = 屏幕 - (100,50)。
        let start = Vec2::new(120.0, 130.0);
        let from = ze_notion::Path2D::from_polyline(&[start, start], false);
        let to = ze_notion::Path2D::from_polyline(
            &[
                start,
                Vec2::new(170.0, 90.0),
                Vec2::new(230.0, 90.0),
                Vec2::new(280.0, 130.0),
            ],
            false,
        );
        let style = Style2D {
            fill: None,
            stroke: Some(Color32::new(0, 200, 200, 255)),
            stroke_width: 4.0,
            space: MeasureSpace::Screen,
            ..Default::default()
        };

        let mut covered: Vec<usize> = Vec::new();
        for t in [0.0f32, 0.5, 1.0] {
            backend.begin_frame_at(g);
            backend.push_morph(MorphItem {
                from: from.clone(),
                to: to.clone(),
                t,
                samples: 32,
                style,
            });
            assert_eq!(backend.recorded_morph_len(), 1);

            let mut enc = ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("zela-2d-morph-degenerate-frame"),
                });
            let out = backend.finish_frame(&mut enc, LayerId(0), &g);
            ctx.queue.submit([enc.finish()]);
            ctx.device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("形变帧提交后 poll");

            assert_eq!(out.cache.morph_requests, 1);
            assert_eq!(
                out.batch_count, 1,
                "退化 `from` 不得被跳过（t = {t}）：形变项应产出 1 次 draw call"
            );

            let px = read_texture(
                &ctx,
                backend.offscreen_texture(LayerId(0)).unwrap(),
                (200, 100),
            );
            let opaque = px.chunks_exact(4).filter(|p| p[3] > 0).count();
            covered.push(opaque);
        }

        assert_eq!(
            covered[0], 0,
            "t = 0 应是退化点（不可见），实际覆盖 {covered:?}"
        );
        assert!(
            covered[1] > 0,
            "t = 0.5 应已画出一半形状，实际覆盖 {covered:?}"
        );
        assert!(
            covered[2] > covered[1],
            "像素覆盖应随 t 增长（形状在长大）：{covered:?}"
        );
    }

    // ── R0/R5：书写动画的 GPU 像素级验收 ─────────────────────────────────

    /// 屏幕坐标下的一条水平直线：视口 (100,50) 起点 ⇒ 离屏本地 `x ∈ [10,190]`、`y = 50`。
    fn reveal_line_path() -> ze_notion::Path2D {
        ze_notion::Path2D::from_polyline(&[Vec2::new(110.0, 100.0), Vec2::new(290.0, 100.0)], false)
    }

    /// 书写动画的描边样式（青色、宽 6 —— 足够宽，像素判据稳定）。
    fn reveal_style() -> Style2D {
        Style2D {
            fill: None,
            stroke: Some(Color32::new(0, 200, 200, 255)),
            stroke_width: 6.0,
            space: MeasureSpace::Screen,
            ..Default::default()
        }
    }

    /// 只画一条书写动画项并回读离屏像素（`200×100`，紧凑 `w*4` 行距）。
    fn reveal_frame(
        backend: &mut WgpuBackend2D,
        ctx: &crate::RenderContext,
        g: Frame2DGeometry,
        span: (f32, f32),
    ) -> Vec<u8> {
        backend.begin_frame_at(g);
        backend.push_reveal(RevealItem {
            path: reveal_line_path(),
            start: span.0,
            end: span.1,
            style: reveal_style(),
        });
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("zela-2d-reveal-frame"),
            });
        let out = backend.finish_frame(&mut enc, LayerId(0), &g);
        assert_eq!(out.batch_count, 1, "一个书写动画项 = 一次 draw call");
        assert_eq!(out.cache.reveal_requests, 1);
        ctx.queue.submit([enc.finish()]);
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("书写动画帧提交后 poll");
        read_texture(
            ctx,
            backend.offscreen_texture(LayerId(0)).unwrap(),
            (200, 100),
        )
    }

    /// 离屏像素 alpha（`w = 200` 的紧凑回读）。
    fn alpha_at(px: &[u8], x: u32, y: u32) -> u8 {
        px[((y * 200 + x) * 4 + 3) as usize]
    }

    /// 🔴 R5-1：`end` 从 0 → 1 时**覆盖像素数严格递增**，且 `end = 0` 时零像素。
    ///
    /// 证伪：若 `advance` 没进顶点（或 shader 没比较区间），各帧像素数会完全相同；
    /// 若区间外的顶点没有被塌缩，`end = 0` 就会是"整条线"。
    #[test]
    fn reveal_grows_monotonically() {
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
        else {
            return; // 无 GPU 环境：跳过
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
        let g = geometry(200.0, 100.0, 1.0, [400, 200]);

        let mut covered = Vec::new();
        for end in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let px = reveal_frame(&mut backend, &ctx, g, (0.0, end));
            covered.push(px.chunks_exact(4).filter(|p| p[3] > 0).count());
        }
        assert_eq!(covered[0], 0, "end = 0 应零像素：{covered:?}");
        for w in covered.windows(2) {
            assert!(w[1] > w[0], "覆盖像素数必须**严格递增**：{covered:?}");
        }
    }

    /// 🔴 R5-2：`end` 小时被覆盖的像素集中在**路径起点侧**（防止"两端同时出现"这类顺序错误）。
    #[test]
    fn reveal_is_ordered_along_path() {
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
        else {
            return; // 无 GPU 环境：跳过
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
        let g = geometry(200.0, 100.0, 1.0, [400, 200]);

        let px = reveal_frame(&mut backend, &ctx, g, (0.0, 0.25));
        let (mut min_x, mut max_x) = (u32::MAX, 0u32);
        for y in 0..100u32 {
            for x in 0..200u32 {
                if alpha_at(&px, x, y) > 0 {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                }
            }
        }
        // 本地 x ∈ [10,190]，总长 180 ⇒ `end = 0.25` 的切口在 x ≈ 55。
        assert!(min_x <= 12, "起点侧必须有像素（min_x = {min_x}）");
        assert!(
            max_x <= 58,
            "end = 0.25 时不得越过四分之一处（max_x = {max_x}）"
        );
    }

    /// 🔴 R5-3：`[0,1]` 全区间与**不带 reveal** 的同一条路径**逐位一致**。
    ///
    /// 这是"新增书写动画不改既有画面"的机器判据：几何各自三角化（一条带预细分、
    /// 一条不带），光栅化结果必须完全相同。
    #[test]
    fn reveal_full_range_matches_no_reveal() {
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
        else {
            return; // 无 GPU 环境：跳过
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
        let g = geometry(200.0, 100.0, 1.0, [400, 200]);

        // 基线：普通 2D 路径（走 `DrawQueue` → `tessellate`）。
        backend.begin_frame_at(g);
        backend.push(
            &Shape2D::Path(reveal_line_path()),
            &Transform2D::identity(),
            &reveal_style(),
            0,
        );
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("zela-2d-reveal-baseline"),
            });
        let out = backend.finish_frame(&mut enc, LayerId(0), &g);
        assert_eq!(out.cache.reveal_requests, 0, "基线不含 reveal 项");
        ctx.queue.submit([enc.finish()]);
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("基线帧提交后 poll");
        let plain = read_texture(
            &ctx,
            backend.offscreen_texture(LayerId(0)).unwrap(),
            (200, 100),
        );

        let reveal = reveal_frame(&mut backend, &ctx, g, (0.0, 1.0));
        assert_eq!(plain, reveal, "[0,1] 必须与不带 reveal 的画面逐位一致");
    }

    /// 🔴 R5-4：区间版本 `[a,b]` 只覆盖**中段**，两端无像素。
    #[test]
    fn reveal_interval_covers_middle_only() {
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
        else {
            return; // 无 GPU 环境：跳过
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
        let g = geometry(200.0, 100.0, 1.0, [400, 200]);

        // 本地 x ∈ [10,190] ⇒ `[0.3, 0.7]` 对应 x ∈ [64, 136]。
        let px = reveal_frame(&mut backend, &ctx, g, (0.3, 0.7));
        assert_eq!(alpha_at(&px, 20, 50), 0, "起点侧（x=20）不应有像素");
        assert_eq!(alpha_at(&px, 180, 50), 0, "终点侧（x=180）不应有像素");
        assert!(alpha_at(&px, 100, 50) > 0, "区间中段（x=100）必须有像素");
        assert!(alpha_at(&px, 70, 50) > 0, "区间内侧（x=70）必须有像素");
        assert!(alpha_at(&px, 130, 50) > 0, "区间内侧（x=130）必须有像素");
    }

    /// 契约布局：R0 后 stride 24 / 四槽（无硬编码字面量的可测证明）。
    #[test]
    fn pipeline_vertex_layout_comes_from_contract() {
        use zela_render::Vertex2D;
        let l = VertexLayout::standard(AaModel::None, CacheStrategy::PerFrame);
        l.validate().unwrap();
        assert_eq!(l.stride, 24);
        assert_eq!(std::mem::size_of::<Vertex2D>(), l.stride as usize);
        let mut attrs = Vec::new();
        let wgpu_layout = to_wgpu(&l, &mut attrs);
        assert_eq!(wgpu_layout.array_stride, 24);
        assert_eq!(wgpu_layout.attributes.len(), 4);
        assert_eq!(wgpu_layout.attributes[0].offset, 0);
        assert_eq!(wgpu_layout.attributes[1].offset, 8);
        assert_eq!(wgpu_layout.attributes[2].offset, 16);
        assert_eq!(wgpu_layout.attributes[3].offset, 20);
    }

    // ── N6：对象级可见性 / 不透明度的 GPU 像素级验收 ─────────────────────
    //
    // 这些用例走**完整录制路径**（`record_scene_tagged` 消费真实 ECS 场景）——
    // 覆盖"过滤发生在录制层、像素效果发生在后端"两段的**联合**行为。
    // hecs 只作为 **dev-dependency** 出现在此（库本体仍不认识 hecs）。

    use hecs::World;
    use ze_object::{Camera2D, Circle};
    use zela_render::{FillSolid, Opacity, record_scene_tagged};
    use zela_scene::contract::{GroupId, GroupVisibility, ObjectId, Visibility, WorldView};

    /// N6 视口矩形：与 `geometry(200, 100, 1.0, [400, 200])` 的 viewport 一致。
    fn n6_viewport() -> Rect2D {
        Rect2D::from_min_size(Vec2::new(100.0, 50.0), Vec2::new(200.0, 100.0))
    }

    /// 世界原点 → 视口中心 (200,100) → 离屏本地 (100,50)（scale 40）。
    fn n6_camera() -> Camera2D {
        Camera2D::new(Vec2::new(0.0, 0.0), 40.0, n6_viewport())
    }

    /// 走录制路径：世界 → `DrawQueue`（可见性过滤 + 不透明度施加都在其中）。
    fn n6_record(
        world: &World,
        camera_tag: Option<ObjectId>,
        groups: &GroupVisibility,
    ) -> DrawQueue {
        let mut q = DrawQueue::new();
        record_scene_tagged(
            &WorldView::new(world),
            &n6_camera(),
            camera_tag,
            groups,
            &mut q,
        );
        q
    }

    /// 把一条 `DrawQueue` 推给 2D 后端并回读离屏像素（`200×100`，紧凑 `w*4`）。
    fn render_draw_queue(
        backend: &mut WgpuBackend2D,
        ctx: &crate::RenderContext,
        g: Frame2DGeometry,
        queue: &DrawQueue,
    ) -> Vec<u8> {
        backend.begin_frame_at(g);
        for cmd in queue.commands() {
            backend.push(&cmd.shape, &Transform2D::identity(), &cmd.style, cmd.z.0);
        }
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("zela-2d-n6-frame"),
            });
        backend.finish_frame(&mut enc, LayerId(0), &g);
        ctx.queue.submit([enc.finish()]);
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("N6 帧提交后 poll");
        read_texture(
            ctx,
            backend.offscreen_texture(LayerId(0)).unwrap(),
            (200, 100),
        )
    }

    /// 离屏像素 `(x, y)`（`w = 200` 紧凑回读，内存序 `[R, G, B, A]`）。
    fn px_200x100(px: &[u8], x: u32, y: u32) -> [u8; 4] {
        let o = ((y * 200 + x) * 4) as usize;
        [px[o], px[o + 1], px[o + 2], px[o + 3]]
    }

    /// 世界原点处一个**填充**圆（半径 0.5 × scale 40 = 20px，圆心落在本地 (100,50)）。
    fn n6_circle() -> (Circle, FillSolid) {
        (
            Circle::new((0.0, 0.0), 0.5),
            FillSolid::new((255, 0, 0, 255)),
        )
    }

    /// 非零 alpha 像素数（"是否投影到画面"的统一判据）。
    fn covered(px: &[u8]) -> usize {
        px.chunks_exact(4).filter(|p| p[3] > 0).count()
    }

    /// 🔴 N6-1/4：`Visibility::global` + `Opacity::FULL` 与**两者都缺席**逐位一致。
    #[test]
    fn visibility_and_opacity_absent_match_global_full() {
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
        else {
            return; // 无 GPU 环境：跳过
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
        let g = geometry(200.0, 100.0, 1.0, [400, 200]);

        let mut bare = World::new();
        bare.spawn(n6_circle());
        let base = render_draw_queue(
            &mut backend,
            &ctx,
            g,
            &n6_record(&bare, None, &GroupVisibility::new()),
        );

        let mut tagged = World::new();
        let (c, f) = n6_circle();
        tagged.spawn((c, f, Visibility::global(), Opacity::FULL));
        let same = render_draw_queue(
            &mut backend,
            &ctx,
            g,
            &n6_record(&tagged, None, &GroupVisibility::new()),
        );

        assert!(px_200x100(&base, 100, 50)[3] > 0, "基线圆心必须有像素");
        assert_eq!(
            base, same,
            "`Visibility::global` + `Opacity::FULL` 必须与缺席**逐位一致**"
        );
    }

    /// 🔴 N6-2：相机 tag 过滤 —— 相机 A 下有像素、相机 B 下**零像素**。
    #[test]
    fn visibility_camera_tag_filters_pixels() {
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
        else {
            return;
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
        let g = geometry(200.0, 100.0, 1.0, [400, 200]);

        let cam_a = ObjectId::new(7, 1);
        let cam_b = ObjectId::new(8, 1);
        let mut world = World::new();
        let (c, f) = n6_circle();
        world.spawn((c, f, Visibility::for_camera(cam_a)));

        let hit = render_draw_queue(
            &mut backend,
            &ctx,
            g,
            &n6_record(&world, Some(cam_a), &GroupVisibility::new()),
        );
        let miss = render_draw_queue(
            &mut backend,
            &ctx,
            g,
            &n6_record(&world, Some(cam_b), &GroupVisibility::new()),
        );

        assert!(covered(&hit) > 0, "相机 A 下必须有像素");
        assert_eq!(covered(&miss), 0, "相机 B 下必须零覆盖像素");
    }

    /// 🔴 N6-3：组级开关过滤 —— 组开有像素、组关零像素。
    #[test]
    fn visibility_group_switch_filters_pixels() {
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
        else {
            return;
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
        let g = geometry(200.0, 100.0, 1.0, [400, 200]);

        let group = GroupId::new(3);
        let mut world = World::new();
        let (c, f) = n6_circle();
        world.spawn((c, f, Visibility::of_group(group)));

        let on = render_draw_queue(
            &mut backend,
            &ctx,
            g,
            &n6_record(&world, None, &GroupVisibility::new()),
        );
        let mut groups = GroupVisibility::new();
        groups.hide(group);
        let off = render_draw_queue(&mut backend, &ctx, g, &n6_record(&world, None, &groups));

        assert!(covered(&on) > 0, "组开 ⇒ 有像素");
        assert_eq!(covered(&off), 0, "组关 ⇒ 零覆盖像素");
    }

    /// 🔴 N6-5/7：`Opacity = 0.5` ⇒ 预乘后 `r ≈ a ≈ 128`，且半透明边缘无黑边
    /// （全图 `|r − a| ≤ 2`）——**这是"只缩 alpha"的可证伪断言**：若 `rgb` 与 `a`
    /// 同乘，预乘后 `r` 会变成 `o²` 档（≈64）而非 128。
    #[test]
    fn opacity_half_premultiplies_and_has_no_dark_halo() {
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
        else {
            return;
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
        let g = geometry(200.0, 100.0, 1.0, [400, 200]);

        let mut world = World::new();
        let (c, f) = n6_circle();
        world.spawn((c, f, Opacity::new(0.5)));
        let px = render_draw_queue(
            &mut backend,
            &ctx,
            g,
            &n6_record(&world, None, &GroupVisibility::new()),
        );

        let center = px_200x100(&px, 100, 50);
        assert!(
            (center[0] as i32 - 128).abs() <= 2,
            "预乘后 r ≈ 128（o=0.5）：{center:?}"
        );
        assert!(
            (center[3] as i32 - 128).abs() <= 2,
            "alpha ≈ 128：{center:?}"
        );
        assert_eq!(center[1], 0, "绿色分量应保持 0");

        for (i, p) in px.chunks_exact(4).enumerate() {
            if p[3] > 0 {
                assert!(
                    (p[0] as i32 - p[3] as i32).abs() <= 2,
                    "半透明像素出现黑边/halo（r 与 a 不等）：#{i} {p:?}"
                );
            }
        }
    }

    /// 🔴 N6-6：`Opacity = 0` ⇒ 零覆盖像素（预乘后 rgb 与 a 同时为 0）。
    #[test]
    fn opacity_zero_yields_no_pixels() {
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
        else {
            return;
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);
        let g = geometry(200.0, 100.0, 1.0, [400, 200]);

        let mut world = World::new();
        let (c, f) = n6_circle();
        world.spawn((c, f, Opacity::new(0.0)));
        let px = render_draw_queue(
            &mut backend,
            &ctx,
            g,
            &n6_record(&world, None, &GroupVisibility::new()),
        );

        assert_eq!(covered(&px), 0, "opacity = 0 ⇒ 零覆盖像素");
        assert!(px.iter().all(|b| *b == 0), "预乘后 rgb 与 a 都应为 0");
    }
}
