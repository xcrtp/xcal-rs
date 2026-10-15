// wgpu_2d.rs
//
// S3-1：wGPU 2D 后端 + 离屏纹理 + MSAA（4x）。
//
// 与 `egui_2d.rs`（旧 CPU 管线）**并行存在**，由运行期开关切换（见 host::present）。
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

use std::mem;

use glam::Mat4;
use wgpu::util::DeviceExt as _;

use xcal_render::resource_cache::{ResourceCache, ResourceKey};
use xcal_render::{
    AaModel, CacheStrategy, DrawQueue, GpuHandle, Painter2DTarget, Shape2D, Style2D, TessKind,
    Tessellation2D, VertexLayout, hash_bytes, tessellate, tolerance_for_scale,
};
use xc_geometry::{Color32, MeasureSpace, Rect2D, Transform2D, Vec2};

use crate::vertex_layout::to_wgpu;
use crate::wgpu_3d::UniformArena;

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

/// 从 [`DrawQueue`] 抓取快照（按 `ZIndex` 稳定排序，同 z 保录制顺序）。
pub fn snapshot_queue(queue: &DrawQueue) -> Vec<Recorded2D> {
    let mut cmds: Vec<Recorded2D> = queue
        .commands()
        .iter()
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

/// 缓存条目：一条 draw 的几何缓冲。
struct GeometryBuffers {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

// Safety: wgpu 资源句柄是内部引用计数的线程安全句柄（与 wgpu_3d.rs 的
// CachedMeshBuffers 同口径）。
unsafe impl Send for GeometryBuffers {}
unsafe impl Sync for GeometryBuffers {}

/// `ResourceKey` 类型标签：2D 三角化几何缓冲。
struct Geometry2DTag;

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

/// 已准备的单条绘制。
struct Prepared2D {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_offset: u32,
}

/// 一帧的离屏结果（`prepare` 阶段产出、由回调交给合成器）。
pub struct Frame2DData {
    /// 离屏纹理（单采样）视图（合成器的采样源）。
    pub offscreen: wgpu::TextureView,
    /// 本帧实际 draw 数（0 表示只有清屏）。
    pub draw_count: usize,
    /// 保活：各 draw 的 bind group 引用的 uniform arena 缓冲。
    pub _arena_buffer: wgpu::Buffer,
}

// ─── WgpuBackend2D ────────────────────────────────────────────────────────

/// wGPU 2D 后端：2D 几何管线 + 离屏/MSAA 目标 + uniform arena + 几何缓存。
///
/// **不含合成管线**（S3-2/C2 起统一由 [`crate::Compositor`] 承担）。
pub struct WgpuBackend2D {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// 2D 几何管线（4x MSAA，目标 = 离屏 MSAA 纹理）。
    pipeline_2d: wgpu::RenderPipeline,
    /// 动态 uniform 绑定布局（与 [`UniformArena`] 配套）。
    bind_group_layout: wgpu::BindGroupLayout,
    arena: UniformArena,
    geom_cache: ResourceCache<()>,
    targets: Option<OffscreenTargets>,
    /// 本帧几何（`begin_frame` 设置，`finish_frame` 消费）。
    geometry: Frame2DGeometry,
    recorder: Vec<Recorded2D>,
}

impl WgpuBackend2D {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let device = device.clone();
        let queue = queue.clone();

        // ── uniform：dynamic offset，与 UniformArena 配套 ──
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xcal-2d-bind-group-layout"),
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
            label: Some("xcal-2d-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // ── 顶点布局：来自契约（无硬编码字面量） ──
        let vertex_layout = VertexLayout::standard(AaModel::None, CacheStrategy::PerFrame);
        let mut attrs = Vec::new();
        let wgpu_vertex_layout = to_wgpu(&vertex_layout, &mut attrs);

        let shape_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xcal-2d-shape-shader"),
            source: wgpu::ShaderSource::Wgsl(SHAPE_2D_SHADER.into()),
        });

        let pipeline_2d = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("xcal-2d-pipeline"),
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

        // ── 合成管线已外移 ──
        // S3-2/C2：本后端只负责画离屏几何纹理；主 pass 的合成由
        // [`crate::Compositor`] 统一承担（含 sRGB / 非 sRGB 双入口 + opacity）。

        Self {
            device,
            queue,
            pipeline_2d,
            bind_group_layout,
            arena: UniformArena::new(UNIFORM_SIZE, DYNAMIC_OFFSET_ALIGNMENT),
            geom_cache: ResourceCache::new(),
            targets: None,
            geometry: Frame2DGeometry {
                viewport_points: Rect2D::default(),
                pixels_per_point: 1.0,
                screen_size_px: [1, 1],
            },
            recorder: Vec::new(),
        }
    }

    /// 本帧视口几何 + 清空录制器。
    pub fn begin_frame_at(&mut self, geometry: Frame2DGeometry) {
        self.geometry = geometry;
        self.recorder.clear();
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

    /// 当前离屏纹理视图（`Painter2DTarget` 路径的"输出纹理"）。
    pub fn offscreen_view(&self) -> Option<&wgpu::TextureView> {
        self.targets.as_ref().map(|t| &t.view)
    }

    /// 当前离屏纹理本体（S3-2/S3-3 把它当纹理共享/拷贝时的入口）。
    pub fn offscreen_texture(&self) -> Option<&wgpu::Texture> {
        self.targets.as_ref().map(|t| &t._color)
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

    /// G7 诊断（R5-2）：几何缓存的存活条目数（帧末 `evict_unused(0)` 之后）。
    pub fn geometry_cache_len(&self) -> usize {
        self.geom_cache.len()
    }

    /// G7 诊断（R5-2）：graveyard 中等待延迟销毁的几何条目数。
    pub fn pending_geometry_destroy(&self) -> usize {
        self.geom_cache.pending_destroy_count()
    }

    fn ensure_targets(&mut self, size: (u32, u32)) {
        if self.targets.as_ref().map(|t| t.size) == Some(size) {
            return;
        }
        let extent = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let color = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xcal-2d-offscreen"),
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
            label: Some("xcal-2d-msaa"),
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
        self.targets = Some(OffscreenTargets {
            size,
            _color: color,
            view,
            _msaa: msaa,
            msaa_view,
        });
    }

    /// 一帧落地：三角化 → 上传几何 → uniform → 离屏 MSAA pass。
    ///
    /// `encoder`：由调用方提供（egui 回调复用 egui 的 encoder ⇒ 离屏 pass 在主 pass 之前执行）。
    pub fn finish_frame(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        geometry: &Frame2DGeometry,
    ) -> Frame2DData {
        let size = geometry.offscreen_size();
        self.ensure_targets(size);

        let mut draws = mem::take(&mut self.recorder);
        // 稳定排序：z 升序，同 z 保录制顺序（画家算法）。
        draws.sort_by_key(|d| d.z);

        self.arena.begin_frame();
        self.geom_cache.begin_frame();

        let local = geometry.local_transform();
        // 展平容差按"≤0.25 物理像素"推导（几何在 points 空间，故除以 ppp）。
        let tolerance = tolerance_for_scale(geometry.pixels_per_point);
        let clip = geometry.clip_matrix();

        let mut prepared: Vec<Prepared2D> = Vec::new();
        for draw in &draws {
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
            for tess in tessellate(&draw.shape, &local, &style, tolerance) {
                if tess.vertices.is_empty() || tess.indices.is_empty() {
                    continue;
                }
                let key = ResourceKey::content::<Geometry2DTag>(hash_tessellation(&tess));
                let hit = self.geom_cache.get(key).map(|handle| {
                    let c = handle
                        .downcast_ref::<GeometryBuffers>()
                        .expect("2D 几何缓存条目必须是 GeometryBuffers");
                    (c.vertex_buffer.clone(), c.index_buffer.clone(), c.index_count)
                });
                let (vertex_buffer, index_buffer, index_count) = match hit {
                    Some(triple) => triple,
                    None => {
                        let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("xcal-2d-vertices"),
                            contents: bytes_of(&tess.vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                        let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("xcal-2d-indices"),
                            contents: bytes_of(&tess.indices),
                            usage: wgpu::BufferUsages::INDEX,
                        });
                        let index_count = tess.indices.len() as u32;
                        let handle = self.geom_cache.insert_handle(
                            key,
                            (),
                            GpuHandle::new(GeometryBuffers {
                                vertex_buffer: vb.clone(),
                                index_buffer: ib.clone(),
                                index_count,
                            }),
                        );
                        let c = handle
                            .downcast_ref::<GeometryBuffers>()
                            .expect("刚插入的条目必须是 GeometryBuffers");
                        (c.vertex_buffer.clone(), c.index_buffer.clone(), c.index_count)
                    }
                };

                let color = match tess.kind {
                    TessKind::Fill => style.fill.unwrap_or(Color32::WHITE),
                    TessKind::Stroke => style.stroke.unwrap_or(Color32::BLACK),
                };
                let block = self
                    .arena
                    .push(&uniform_bytes(clip, color, style.stroke_width));
                prepared.push(Prepared2D {
                    vertex_buffer,
                    index_buffer,
                    index_count,
                    uniform_offset: block * DYNAMIC_OFFSET_ALIGNMENT as u32,
                });
            }
        }

        // 帧末驱逐：动画内容的几何哈希每帧不同，必须驱逐否则缓存无界增长。
        // 被驱逐的 GPU 缓冲进入 deferred destroyer（graveyard 3 次 submit 后才真正 drop）。
        self.geom_cache.evict_unused(0);
        self.geom_cache.notify_submit();

        // 一次性上传全部 uniform，拿到本帧共享的 bind group（借用随后被 clone 掉）。
        let (arena_buffer, arena_bind_group) = {
            let (buf, bg, _stride) =
                self.arena
                    .flush(&self.device, &self.queue, &self.bind_group_layout);
            (buf.clone(), bg.clone())
        };

        let targets = self
            .targets
            .as_ref()
            .expect("ensure_targets 之后必然存在");

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xcal-2d-offscreen-pass"),
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
                for d in &prepared {
                    pass.set_bind_group(0, &arena_bind_group, &[d.uniform_offset]);
                    pass.set_vertex_buffer(0, d.vertex_buffer.slice(..));
                    pass.set_index_buffer(d.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..d.index_count, 0, 0..1);
                }
            }
        }

        // ── 产出离屏结果（合成由调用方 / 回调经合成器完成） ──
        Frame2DData {
            offscreen: targets.view.clone(),
            draw_count: prepared.len(),
            _arena_buffer: arena_buffer,
        }
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
                label: Some("xcal-2d-standalone-encoder"),
            });
        let _frame = self.finish_frame(&mut encoder, &geometry);
        self.queue.submit([encoder.finish()]);
    }
}

// ─── helpers ──────────────────────────────────────────────────────────────

/// 把 Copy 切片视为字节切片（顶点/索引上传用）。
fn bytes_of<T: Copy>(data: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, mem::size_of_val(data)) }
}

/// 三角化结果的内容哈希（顶点 + 索引 + 种类），用于几何缓冲复用。
fn hash_tessellation(t: &Tessellation2D) -> u64 {
    let mut h = hash_bytes(bytes_of(&t.vertices));
    for &i in &t.indices {
        h ^= i as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h ^= match t.kind {
        TessKind::Fill => 0,
        TessKind::Stroke => 0x9E37_79B9_7F4A_7C15,
    };
    h
}

/// uniform 字节：mat4 列主序（64）+ 预乘颜色 vec4（16）+ params vec4（16）。
///
/// `params.x` 写 `stroke_width`（偏移 80）——shader 用它沿 `normal` 外扩半个线宽，
/// 把顶点存着的**路径点**还原成轮廓点（R1 几何参数化）。
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
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcal_scene::domain::LayerBlend;
    use xcal_render::{DrawCommand, ZIndex};

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
        assert_eq!(w, ((1.5f32 * 400.0).round() - (1.5f32 * 100.0).round()) as u32);
        assert_eq!(h, ((1.5f32 * 250.0).round() - (1.5f32 * 50.0).round()) as u32);
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
        let nw = m * glam::vec4(0.0, 0.0, 0.0, 1.0);
        let se = m * glam::vec4(w as f32, h as f32, 0.0, 1.0);
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
    #[test]
    fn uniform_stroke_width_lands_at_offset_80() {
        for w in [0.0f32, 1.0, 2.0, 8.0] {
            let b = uniform_bytes(Mat4::IDENTITY, Color32::WHITE, w);
            // params.x（偏移 80）必须是原样写入的宽度。
            assert_eq!(f32_at_of(&b, 80), w, "params.x 应为 stroke_width={w}");
            // y/z/w 预留为 0（不参与偏移计算）。
            assert_eq!(f32_at_of(&b, 84), 0.0);
            assert_eq!(f32_at_of(&b, 88), 0.0);
            assert_eq!(f32_at_of(&b, 92), 0.0);
        }
    }

    fn f32_at_of(bytes: &[u8], off: usize) -> f32 {
        f32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
    }

    /// 队列快照：按 z 稳定排序（同 z 保录制顺序）。
    #[test]
    fn snapshot_is_stably_sorted_by_z() {
        let mut q = DrawQueue::new();
        q.push(DrawCommand::new(ZIndex::new(5), Shape2D::Empty, Style2D::default()));
        q.push(DrawCommand::new(
            ZIndex::new(1),
            Shape2D::Line {
                start: Vec2::ZERO,
                end: Vec2::new(1.0, 1.0),
            },
            Style2D::default(),
        ));
        q.push(DrawCommand::new(ZIndex::new(5), Shape2D::Empty, Style2D::default()));
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
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Rgba8Unorm)
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
            Shape2D::Rect(Rect2D::new(
                Vec2::new(150.0, 75.0),
                Vec2::new(250.0, 125.0),
            )),
            Style2D {
                fill: Some(Color32::new(255, 0, 0, 255)),
                space: MeasureSpace::Screen,
                ..Default::default()
            },
        ));
        let draws = snapshot_queue(&q);

        // 主 pass 目标（模拟 eframe 的帧缓冲）。
        let main = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xcal-2d-test-main-target"),
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
                label: Some("xcal-2d-test-e2e"),
            });
        let frame = backend.finish_frame(&mut enc, &g);
        let compositor = Compositor::new(&ctx.device, ctx.target_format);
        let composite = compositor.prepare(
            &ctx.device,
            &[CompositeOp {
                texture: frame.offscreen.clone(),
                src_format: OFFSCREEN_FORMAT,
                blend: LayerBlend::Alpha,
                opacity: 1.0,
                rect_px: (left as f32, top as f32, vw as f32, vh as f32),
            }],
        );
        {
            let mut pass = enc
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("xcal-2d-test-main-pass"),
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
    fn read_texture(
        ctx: &crate::RenderContext,
        texture: &wgpu::Texture,
        size: (u32, u32),
    ) -> Vec<u8> {
        let (w, h) = size;
        // `copy_texture_to_buffer` 要求 bytes_per_row 为 256 的倍数。
        let padded = (w * 4).div_ceil(256) * 256;
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xcal-2d-readback"),
            size: (padded * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("xcal-2d-readback-encoder"),
            });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue.submit([enc.finish()]);

        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map_async 失败"));
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll 失败");
        let data = slice.get_mapped_range().expect("离屏回读映射失败");
        let mut out = vec![0u8; (w * 4 * h) as usize];
        for y in 0..h {
            let src = (y * padded) as usize;
            let dst = (y * w * 4) as usize;
            out[dst..dst + (w * 4) as usize].copy_from_slice(&data[src..src + (w * 4) as usize]);
        }
        drop(data);
        buffer.unmap();
        out
    }

    /// GPU 冒烟（真实 device）：完整跑通"三角化 → 几何上传 → uniform arena →
    /// 离屏 MSAA 4x pass → resolve"，回读离屏像素证明图元落在**预期位置**且颜色
    /// 与 uniform 一致（不只"没报错"）；再用变化的视口尺寸跑第二帧验证离屏目标
    /// 按尺寸重建（不拉伸、不越界——尺寸不匹配会被 wgpu 校验直接拒掉）。
    ///
    /// 无可用适配器（headless 构造失败）时跳过，与 `render_context` 既有约定一致。
    #[test]
    fn gpu_frame_rasterizes_and_recreates_targets_on_resize() {
        let Ok(ctx) = crate::RenderContext::headless_with_format(
            wgpu::TextureFormat::Bgra8UnormSrgb,
        ) else {
            return; // 无 GPU 环境：跳过
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue);

        // 视口 points (100,50)-(300,150)，ppp = 1 → 离屏 200×100 像素；
        // 屏幕坐标 → 离屏本地坐标 = 平移 (-100,-50)。
        let mut q = DrawQueue::new();
        q.push(DrawCommand::new(
            ZIndex::new(0),
            Shape2D::Rect(Rect2D::new(
                Vec2::new(150.0, 75.0),
                Vec2::new(250.0, 125.0),
            )),
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
                label: Some("xcal-2d-test-frame-1"),
            });
        let frame = backend.finish_frame(&mut enc, &g);
        assert_eq!(frame.draw_count, 2, "填充 + 描边各一条几何");
        ctx.queue.submit([enc.finish()]);
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("第一帧 submit 后 poll");

        let px = read_texture(&ctx, backend.offscreen_texture().unwrap(), (200, 100));
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
                label: Some("xcal-2d-test-frame-2"),
            });
        let frame2 = backend.finish_frame(&mut enc2, &g2);
        assert_eq!(frame2.draw_count, 2);
        assert_eq!(backend.offscreen_texture().unwrap().width(), 400);
        assert_eq!(backend.offscreen_texture().unwrap().height(), 300);
        ctx.queue.submit([enc2.finish()]);
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("第二帧 submit 后 poll");

        // 本地坐标不随视口变化（视口原点平移恒定），故内容位置与第一帧一致。
        let px2 = read_texture(&ctx, backend.offscreen_texture().unwrap(), (400, 300));
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
        let Ok(ctx) = crate::RenderContext::headless() else {
            return; // 无 GPU 环境：跳过
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
                    label: Some("xcal-2d-format-smoke"),
                });
            let frame = backend.finish_frame(&mut enc, &g);
            assert_eq!(frame.draw_count, 1, "{format:?} 下应有一条几何");
            ctx.queue.submit([enc.finish()]);
            ctx.device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("提交后 poll");
        }
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
    /// `xcal_render::tessellate::tests::shader_offset_reproduces_lyon_contour_points`）。
    #[test]
    fn geometry_topology_is_independent_of_width() {
        let shape = Shape2D::Line {
            start: Vec2::new(0.0, 0.0),
            end: Vec2::new(30.0, 10.0),
        };
        let thin = tessellate(&shape, &Transform2D::identity(), &stroke_style(2.0), 0.05);
        let thick = tessellate(&shape, &Transform2D::identity(), &stroke_style(8.0), 0.05);
        assert_eq!(thin.len(), 1);
        assert_eq!(thick.len(), 1);
        let (a, b) = (&thin[0], &thick[0]);
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.vertices.len(), b.vertices.len(), "顶点数必须与宽度无关");
        assert_eq!(a.indices, b.indices, "index 表必须与宽度无关");
        for (i, (x, y)) in a.vertices.iter().zip(&b.vertices).enumerate() {
            assert_eq!(x.position, y.position, "第 {i} 个顶点的路径点必须逐字节相同");
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
                    label: Some("xcal-2d-bounded-frame"),
                });
            let out = backend.finish_frame(&mut enc, &g);
            assert_eq!(out.draw_count, DRAWS as usize, "第 {frame} 帧：draw 数应为 {DRAWS}");
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

    /// 契约布局：stride 20 / 三槽（无硬编码字面量的可测证明）。
    #[test]
    fn pipeline_vertex_layout_comes_from_contract() {
        use xcal_render::Vertex2D;
        let l = VertexLayout::standard(AaModel::None, CacheStrategy::PerFrame);
        l.validate().unwrap();
        assert_eq!(l.stride, 20);
        assert_eq!(std::mem::size_of::<Vertex2D>(), l.stride as usize);
        let mut attrs = Vec::new();
        let wgpu_layout = to_wgpu(&l, &mut attrs);
        assert_eq!(wgpu_layout.array_stride, 20);
        assert_eq!(wgpu_layout.attributes.len(), 3);
        assert_eq!(wgpu_layout.attributes[0].offset, 0);
        assert_eq!(wgpu_layout.attributes[1].offset, 8);
        assert_eq!(wgpu_layout.attributes[2].offset, 16);
    }
}
