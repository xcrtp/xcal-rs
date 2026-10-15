// wgpu_2d.rs
//
// S3-1：wGPU 2D 后端 + 离屏纹理 + MSAA（4x）。
//
// 与 `egui_2d.rs`（旧 CPU 管线）**并行存在**，由运行期开关切换（见 host::present）。
// 本后端把 `Shape2D` 三角化（S3-0 的 lyon 适配层）后渲染到**离屏纹理**，
// 再作为一个四边形合成进 eframe 主 pass —— 为 S3-2（合成器）/ S3-3（2D/3D 互为纹理）铺路。
//
// 关键口径：
// - **顶点布局来自契约**：`to_wgpu(&VertexLayout::standard(AaModel::None, Static))`，
//   无任何硬编码 `Float32x2` 字面量（合成管线复用同一契约布局：position=裁剪坐标、
//   attrib0=UV、attrib1 未用）；
// - **MSAA 4x**：`MSAA_SAMPLES = 4`，离屏 pass = MSAA 纹理 → resolve 到单采样离屏纹理；
// - **2D 不用深度缓冲**：z 序由 `ZIndex` 稳定排序保证（画家算法）；
// - **uniform 复用 `UniformArena`**（256B 对齐 dynamic offset），不另造一套；
// - 几何缓冲按**内容哈希**缓存（`ResourceCache`），非"每帧必建"；
//   动画内容每帧哈希不同 → 帧末 `evict_unused(0)` + deferred destroy 保证有界。
//
// 已知限制（与计划 §7 一致）：本阶段 `AaModel::None`（硬边 + MSAA），
// shader 不解释 normal/side；描边宽度已由 CPU 侧 lyon 落进几何（未走 shader 补偿）。

use std::mem;

use glam::Mat4;
use wgpu::util::DeviceExt as _;

use xcal_render::resource_cache::{ResourceCache, ResourceKey};
use xcal_render::{
    AaModel, CacheStrategy, DrawQueue, GpuHandle, Painter2DTarget, Shape2D, Style2D, TessKind,
    Tessellation2D, Vertex2D, VertexLayout, hash_bytes, tessellate, tolerance_for_scale,
};
use xc_geometry::{Color32, MeasureSpace, Rect2D, Transform2D, Vec2};

use crate::vertex_layout::to_wgpu;
use crate::wgpu_3d::UniformArena;

/// MSAA 采样数（验收项：`multisample.count == 4`）。
pub const MSAA_SAMPLES: u32 = 4;

/// 离屏纹理格式：**非 sRGB**（存 gamma 值），与 egui-wgpu 对主目标的口径一致
/// （`egui.wgsl` 的 `fs_main_gamma_framebuffer` 分支 + 其 "egui prefers Rgba8Unorm" 提示）。
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// uniform 块大小：mat4x4（64）+ vec4（16）。
const UNIFORM_SIZE: u64 = 80;
/// wgpu dynamic uniform offset 要求的最小对齐。
const DYNAMIC_OFFSET_ALIGNMENT: u64 = 256;

/// 离屏清屏色：与 GridBuild 的背景矩形 `(20,20,20,255)` 保持一致。
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 20.0 / 255.0,
    g: 20.0 / 255.0,
    b: 20.0 / 255.0,
    a: 1.0,
};

/// 2D 几何着色器（几何 pass）。
pub const SHAPE_2D_SHADER: &str = include_str!("../shaders/shape_2d.wgsl");
/// 合成着色器（把离屏纹理画进主 pass）。
pub const COMPOSITE_2D_SHADER: &str = include_str!("../shaders/composite_2d.wgsl");

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
    /// 合成四边形本身不依赖它（见 [`COMPOSITE_QUAD`]）。
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

/// 一帧的合成数据（`prepare` 阶段产出、`paint` 阶段消费）。
pub struct Frame2DData {
    /// 离屏纹理（单采样）视图。
    pub offscreen: wgpu::TextureView,
    /// 合成绑定组（离屏纹理 + 采样器）。
    pub composite_bind_group: wgpu::BindGroup,
    /// 合成四边形顶点缓冲（裁剪坐标 + UV，契约布局）。
    pub quad: wgpu::Buffer,
    /// 合成四边形顶点数。
    pub quad_vertices: u32,
    /// 本帧实际 draw 数（0 表示只有清屏）。
    pub draw_count: usize,
    /// 保活：各 draw 的 bind group 引用的 uniform arena 缓冲。
    pub _arena_buffer: wgpu::Buffer,
}

// ─── WgpuBackend2D ────────────────────────────────────────────────────────

/// wGPU 2D 后端：2D 管线 + 合成管线 + 离屏/MSAA 目标 + uniform arena + 几何缓存。
pub struct WgpuBackend2D {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// 2D 几何管线（4x MSAA，目标 = 离屏 MSAA 纹理）。
    pipeline_2d: wgpu::RenderPipeline,
    /// 合成管线（主 pass，1x —— 与 egui 主 pass 采样数一致，同 3D 回调口径）。
    pipeline_composite: wgpu::RenderPipeline,
    /// 动态 uniform 绑定布局（与 [`UniformArena`] 配套）。
    bind_group_layout: wgpu::BindGroupLayout,
    /// 合成绑定布局（纹理 + 采样器）。
    composite_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    arena: UniformArena,
    geom_cache: ResourceCache<()>,
    targets: Option<OffscreenTargets>,
    /// 本帧几何（`begin_frame` 设置，`finish_frame` 消费）。
    geometry: Frame2DGeometry,
    recorder: Vec<Recorded2D>,
}

impl WgpuBackend2D {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, target_format: wgpu::TextureFormat) -> Self {
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
        let vertex_layout = VertexLayout::standard(AaModel::None, CacheStrategy::Static);
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

        // ── 合成管线 ──
        let composite_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xcal-2d-composite-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xcal-2d-composite-pipeline-layout"),
            bind_group_layouts: &[Some(&composite_bgl)],
            immediate_size: 0,
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xcal-2d-composite-shader"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_2D_SHADER.into()),
        });
        let mut composite_attrs = Vec::new();
        let composite_vertex_layout = to_wgpu(&vertex_layout, &mut composite_attrs);
        // 主 pass 目标为 sRGB 时硬件会做 linear→sRGB 编码，需先 gamma→linear；
        // 非 sRGB 目标直接输出 gamma 值（与 egui.wgsl 的两分支同口径）。
        let composite_entry = if target_format.is_srgb() {
            "fs_linear"
        } else {
            "fs_gamma"
        };
        let pipeline_composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("xcal-2d-composite-pipeline"),
            layout: Some(&composite_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(composite_vertex_layout)],
            },
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some(composite_entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            // 与 egui 主 pass 的采样数一致（同 3D 回调口径：主 pass 为 1x）。
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("xcal-2d-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            device,
            queue,
            pipeline_2d,
            pipeline_composite,
            bind_group_layout,
            composite_bgl,
            sampler,
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
                let block = self.arena.push(&uniform_bytes(clip, color));
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

        // ── 合成数据：四边形（裁剪坐标 + UV，契约布局）+ 绑定组 ──
        let quad_vertices = COMPOSITE_QUAD;
        let quad = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("xcal-2d-composite-quad"),
            contents: bytes_of(&quad_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let composite_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xcal-2d-composite-bind-group"),
            layout: &self.composite_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&targets.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        Frame2DData {
            offscreen: targets.view.clone(),
            composite_bind_group,
            quad,
            quad_vertices: quad_vertices.len() as u32,
            draw_count: prepared.len(),
            _arena_buffer: arena_buffer,
        }
    }

    /// `paint` 阶段：把离屏纹理合成到主 pass 的当前目标上。
    pub fn composite(&self, frame: &Frame2DData, render_pass: &mut wgpu::RenderPass<'static>) {
        render_pass.set_pipeline(&self.pipeline_composite);
        render_pass.set_bind_group(0, &frame.composite_bind_group, &[]);
        render_pass.set_vertex_buffer(0, frame.quad.slice(..));
        render_pass.draw(0..frame.quad_vertices, 0..1);
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

// ─── egui-wgpu 回调 ───────────────────────────────────────────────────────

/// 2D 图层回调：`prepare` 渲染到离屏纹理，`paint` 合成进主 pass。
///
/// 与 [`crate::Layer3DCallback`] 并列；两者都通过 `painter.add(Shape::Callback(..))`
/// 加入同一图层，**添加顺序即绘制顺序**（2D 先加 → 3D 覆盖其上，与旧管线 z 序一致）。
#[cfg(feature = "egui-wgpu")]
pub struct Layer2DCallback {
    viewport_points: Rect2D,
    draws: Vec<Recorded2D>,
}

#[cfg(feature = "egui-wgpu")]
impl Layer2DCallback {
    /// 从视口矩形（egui points）与 2D 队列快照构造。
    pub fn new(viewport_points: Rect2D, queue: &DrawQueue) -> Self {
        Self {
            viewport_points,
            draws: snapshot_queue(queue),
        }
    }

    /// 快照中的指令数。
    pub fn draw_count(&self) -> usize {
        self.draws.len()
    }
}

#[cfg(feature = "egui-wgpu")]
impl egui_wgpu::CallbackTrait for Layer2DCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        screen_descriptor: &egui_wgpu::ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let geometry = Frame2DGeometry {
            viewport_points: self.viewport_points,
            pixels_per_point: screen_descriptor.pixels_per_point,
            screen_size_px: screen_descriptor.size_in_pixels,
        };
        let frame = {
            let Some(backend) = callback_resources.get_mut::<WgpuBackend2D>() else {
                return Vec::new();
            };
            backend.begin_frame_at(geometry);
            for d in &self.draws {
                backend.push(&d.shape, &d.transform, &d.style, d.z);
            }
            // 复用 egui 的 encoder：离屏 pass 在主 pass 之前执行（同 encoder 内顺序）。
            backend.finish_frame(egui_encoder, &geometry)
        };
        callback_resources.insert(frame);
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::epaint::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let (Some(backend), Some(frame)) = (
            callback_resources.get::<WgpuBackend2D>(),
            callback_resources.get::<Frame2DData>(),
        ) else {
            return;
        };
        // 裁剪到回调区域，避免 2D 内容覆盖 egui 面板。
        let clip = info.clip_rect_in_pixels();
        render_pass.set_scissor_rect(
            clip.left_px.max(0) as u32,
            clip.top_px.max(0) as u32,
            clip.width_px.max(0) as u32,
            clip.height_px.max(0) as u32,
        );
        backend.composite(frame, render_pass);
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

/// uniform 字节：mat4 列主序（64）+ 预乘颜色 vec4（16）。
fn uniform_bytes(transform: Mat4, color: Color32) -> [u8; UNIFORM_SIZE as usize] {
    let mut data = [0u8; UNIFORM_SIZE as usize];
    data[..64].copy_from_slice(bytes_of(&transform.to_cols_array()));
    let a = color.a() as f32 / 255.0;
    let c = [
        color.r() as f32 / 255.0 * a,
        color.g() as f32 / 255.0 * a,
        color.b() as f32 / 255.0 * a,
        a,
    ];
    data[64..].copy_from_slice(bytes_of(&c));
    data
}

/// 合成四边形（TriangleStrip，4 顶点）：**写死覆盖 NDC [-1,1]²**，
/// position = 裁剪坐标、attrib0 = UV（复用契约布局的 stride 20 三槽，
/// `side` 未使用，避免任何硬编码顶点格式）。
///
/// 不能按"整屏尺寸"把视口像素矩形换算成裁剪坐标：egui-wgpu 在处理
/// `Primitive::Callback` 时会把 render pass 的 viewport 设成**该回调自己的矩形**
/// （见 egui-wgpu `renderer.rs` 里那段 "courtesy" 注释的 `set_viewport(...)`），
/// 也就是说 NDC [-1,1]² 在 `paint` 回调里对应的**就是**回调视口本身。
/// 若再拿整屏尺寸做一次换算，视口会被二次缩放：X 被压成 `视口宽/屏幕宽`、
/// Y 被压成 `视口高/屏幕高`（有侧边控制面板时前者远小于 1 → 2D 视图在 X 方向
/// 明显被压扁并向右偏移），这正是本 bug 的成因。
///
/// 顶点顺序按 `TriangleStrip` 解释：左上 → 右上 → 左下 → 右下
/// （wgpu 的 viewport 变换把 ndc.y = +1 映射到视口顶边）。
const COMPOSITE_QUAD: [Vertex2D; 4] = [
    Vertex2D {
        position: [-1.0, 1.0],
        normal: [0.0, 0.0],
        side: 0.0,
    },
    Vertex2D {
        position: [1.0, 1.0],
        normal: [1.0, 0.0],
        side: 0.0,
    },
    Vertex2D {
        position: [-1.0, -1.0],
        normal: [0.0, 1.0],
        side: 0.0,
    },
    Vertex2D {
        position: [1.0, -1.0],
        normal: [1.0, 1.0],
        side: 0.0,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use xcal_render::{DrawCommand, ZIndex};

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

    /// 合成四边形：4 顶点、覆盖视口、UV 为 (0,0)-(1,1)（契约布局）。
    #[test]
    fn composite_quad_is_full_ndc_box_with_uv() {
        let quad = COMPOSITE_QUAD;
        assert_eq!(quad.len(), 4);
        // TriangleStrip 的顶点序：左上 → 右上 → 左下 → 右下。
        assert_eq!(quad[0].position, [-1.0, 1.0]);
        assert_eq!(quad[1].position, [1.0, 1.0]);
        assert_eq!(quad[2].position, [-1.0, -1.0]);
        assert_eq!(quad[3].position, [1.0, -1.0]);
        assert_eq!(quad[0].normal, [0.0, 0.0]);
        assert_eq!(quad[1].normal, [1.0, 0.0]);
        assert_eq!(quad[2].normal, [0.0, 1.0]);
        assert_eq!(quad[3].normal, [1.0, 1.0]);
        for v in &quad {
            assert_eq!(v.side, 0.0);
        }
    }

    /// **回归**（"2D 视图在 X 方向被压扁"）：egui-wgpu 会为每个回调把 render pass
    /// 的 viewport 设成该回调矩形（`renderer.rs` 的 "courtesy" `set_viewport`），
    /// 所以合成四边形必须在**任意**视口位置/尺寸下都正好铺满该视口。
    /// 这里按 wgpu 的 viewport 变换把四边形顶点换算回帧缓冲像素，
    /// 断言左右/上下边界与 `viewport_px()` 完全一致（容忍 ±1px 取整）。
    #[test]
    fn composite_quad_fills_callback_viewport_at_any_position() {
        // 视口偏右且比屏幕窄（左侧有控制面板），正是暴露该 bug 的形态。
        for g in [
            geometry(300.0, 200.0, 1.0, [1600, 1200]),
            geometry(1350.0, 1150.0, 1.0, [1600, 1200]),
            geometry(300.0, 200.0, 1.5, [1600, 1200]),
        ] {
            let (left, top, w, h) = g.viewport_px();
            // wgpu viewport 变换：x_fb = left + (ndc.x+1)/2 * w；y_fb = top + (1-ndc.y)/2 * h
            let to_fb = |ndc: [f32; 2]| -> (f32, f32) {
                (
                    left as f32 + (ndc[0] + 1.0) * 0.5 * w as f32,
                    top as f32 + (1.0 - ndc[1]) * 0.5 * h as f32,
                )
            };
            let (x_l, y_t) = to_fb(COMPOSITE_QUAD[0].position);
            let (x_r, y_b) = to_fb(COMPOSITE_QUAD[3].position);
            assert!((x_l - left as f32).abs() < 0.5, "左边界 {x_l} != {left}");
            assert!((y_t - top as f32).abs() < 0.5, "上边界 {y_t} != {top}");
            assert!(
                (x_r - (left + w as i32) as f32).abs() < 0.5,
                "右边界 {x_r} != {}",
                left + w as i32
            );
            assert!(
                (y_b - (top + h as i32) as f32).abs() < 0.5,
                "下边界 {y_b} != {}",
                top + h as i32
            );
        }
    }

    /// uniform 字节：mat4 + 预乘颜色（alpha=255 时颜色不变）。
    #[test]
    fn uniform_bytes_layout() {
        let b = uniform_bytes(Mat4::IDENTITY, Color32::new(10, 20, 30, 255));
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
        let b2 = uniform_bytes(Mat4::IDENTITY, Color32::new(200, 0, 0, 128));
        let a = f32_at_of(&b2, 76);
        assert!((f32_at_of(&b2, 64) - 200.0 / 255.0 * a).abs() < 1e-6);
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
    /// （`set_viewport(视口 px)`）—— 再执行 `composite()`，然后回读**主目标**
    /// 像素，断言 2D 内容落在视口内的正确位置：
    ///
    /// - 视口像素矩形 (100,50)-(400,250)，离屏尺寸 = 视口尺寸；
    /// - 图元：屏幕矩形 (150,75)-(250,125)（= 离屏本地 (50,25)-(150,75)）；
    /// - 修复前：quad 用整屏尺寸换算 → 内容被压成 `300/1600` 宽并右移，
    ///   视口内左侧根本收不到离屏清屏色（本测试会失败）。
    #[test]
    fn composite_lands_inside_scoped_viewport_on_real_device() {
        // 用非 sRGB 的 RGBA 主目标：合成走 `fs_gamma`（字节直通），
        // 这样下面的颜色断言可以是逐字节精确的（sRGB 编解码路径由
        // `gpu_pipeline_builds_for_srgb_and_non_srgb_targets` 覆盖创建与运行）。
        let Ok(ctx) =
            crate::RenderContext::headless_with_format(wgpu::TextureFormat::Rgba8Unorm)
        else {
            return; // 无 GPU 环境：跳过
        };
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue, ctx.target_format);

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
            backend.composite(&frame, &mut pass);
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
        const CLEAR: [u8; 4] = [20, 20, 20, 255];
        // 视口内、矩形内 → 红。
        assert_eq!(at(200, 100), [255, 0, 0, 255], "图元应在视口内的正确位置");
        // 视口内、矩形外 → 离屏清屏色（证明离屏铺满了**整个**视口）。
        assert_eq!(at(110, 60), CLEAR, "视口左上角应被离屏覆盖");
        assert_eq!(at(390, 240), CLEAR, "视口右下角应被离屏覆盖");
        // 视口外 → 主目标自身的清屏色（黑），证明没有越界。
        assert_eq!(at(50, 50), [0, 0, 0, 255], "视口外不应被绘制");
        assert_eq!(at(500, 400), [0, 0, 0, 255], "视口外不应被绘制");
        // 水平边界（±2px 容差）：矩形左边 150、右边 250。
        assert_eq!(at(152, 100), [255, 0, 0, 255]);
        assert_eq!(at(148, 100), CLEAR, "矩形左侧应仍是离屏清屏色");
        assert_eq!(at(248, 100), [255, 0, 0, 255]);
        assert_eq!(at(252, 100), CLEAR, "矩形右侧应仍是离屏清屏色");
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
        let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue, ctx.target_format);

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
        // 未绘制处 = 清屏色。
        assert_eq!(at(10, 10), [20, 20, 20, 255], "未绘制处应为清屏色");
        assert_eq!(at(199, 99), [20, 20, 20, 255]);

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
        // 新增区域内侧边界仍为清屏色（没有拉伸/残留）。
        assert_eq!(at2(390, 290), [20, 20, 20, 255]);
        assert_eq!(at2(10, 10), [20, 20, 20, 255]);
    }

    /// 合成管线对两种主 pass 目标格式都能建出来并跑通（sRGB → `fs_linear`，
    /// 非 sRGB → `fs_gamma`）：eframe 的目标格式依平台/配置而定，
    /// 少覆盖一支就等于在别的机器上启动即 panic。
    #[test]
    fn gpu_pipeline_builds_for_srgb_and_non_srgb_targets() {
        let Ok(ctx) = crate::RenderContext::headless() else {
            return; // 无 GPU 环境：跳过
        };
        for format in [
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba8Unorm,
        ] {
            let mut backend = WgpuBackend2D::new(&ctx.device, &ctx.queue, format);
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

    /// 契约布局：stride 20 / 三槽（无硬编码字面量的可测证明）。
    #[test]
    fn pipeline_vertex_layout_comes_from_contract() {
        let l = VertexLayout::standard(AaModel::None, CacheStrategy::Static);
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
