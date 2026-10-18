// compositor.rs — S3-2 合成器：把 N 张图层纹理按**声明顺序**混合进主 pass。
//
// 设计（计划 §6）：
// - 复用 `composite_2d.wgsl`（已含 sRGB / 非 sRGB 双入口），泛化为通用图层合成；
// - 混合方式由 [`LayerBlend`] 决定，wgpu **不支持动态 blend**，故按 blend 预建三组
//   pipeline（`HashMap<LayerBlend, RenderPipeline>`）；
// - `opacity` 走 uniform（blend constant 在部分后端受限），不进 blend state；
// - 四边形永远是**全 NDC + UV 0..1**：egui-wgpu 会为每个回调把 render pass 的
//   viewport 设成该回调矩形，所以 NDC 全屏 == 该图层自己的视口（见
//   `composite_quad_is_full_ndc_box_with_uv` / `..._fills_callback_viewport_at_any_position`）。
//
// **顺序即声明**：`CompositeFrame::ops` 的索引顺序就是绘制顺序（先画的在下），
// 合成器**不得重排**（无 z 排序、无去重）。

use std::collections::HashMap;

use wgpu::util::DeviceExt as _;

use xcal_render::{
    AaModel, CacheStrategy, POST_UNIFORM_SIZE, PostFilterChain, Vertex2D, VertexLayout,
};
use xcal_scene::domain::LayerBlend;

use crate::gpu_util::bytes_of;
use crate::vertex_layout::to_wgpu;

/// 合成着色器：源纹理 → 主 pass（含 sRGB / 非 sRGB 双入口 + opacity uniform）。
pub const COMPOSITE_SHADER: &str = include_str!("../shaders/composite_2d.wgsl");

/// 滤镜链着色器（C1 uber shader：4 固定槽位，按 `kind` 分支）。
pub const POST_SHADER: &str = include_str!("../shaders/post_filter.wgsl");

/// 每层 opacity uniform 的字节数（`vec4<f32>`）。
pub const OPACITY_UNIFORM_SIZE: u64 = 16;

/// 合成四边形（裁剪坐标 + UV；TriangleStrip：左上 → 右上 → 左下 → 右下）。
///
/// `attrib1(side)` / `attrib2(advance)` 都不参与合成（`composite_2d.wgsl` /
/// `post_filter.wgsl` 只读 location 0/1），但**必须**存在：契约布局 R0 后是四槽，
/// 缓冲按 `bytes_of(&[Vertex2D])` 原样上传 ⇒ 顶点结构体与 stride 必须一致。
pub const COMPOSITE_QUAD: [Vertex2D; 4] = [
    Vertex2D {
        position: [-1.0, 1.0],
        normal: [0.0, 0.0],
        side: 0.0,
        advance: 0.0,
    },
    Vertex2D {
        position: [1.0, 1.0],
        normal: [1.0, 0.0],
        side: 0.0,
        advance: 0.0,
    },
    Vertex2D {
        position: [-1.0, -1.0],
        normal: [0.0, 1.0],
        side: 0.0,
        advance: 0.0,
    },
    Vertex2D {
        position: [1.0, -1.0],
        normal: [1.0, 1.0],
        side: 0.0,
        advance: 0.0,
    },
];

/// 一次合成操作（声明式）：源纹理 + 落地矩形 + 混合参数。
pub struct CompositeOp {
    /// 源图层纹理（2D 离屏 / 3D 离屏）。
    pub texture: wgpu::TextureView,
    /// 源纹理格式。**只用于**判断域转换（`is_srgb`）：
    /// 2D 离屏存的是 gamma 值（`Rgba8Unorm`）；3D 离屏用主目标格式。
    pub src_format: wgpu::TextureFormat,
    /// 与下层的混合方式。
    pub blend: LayerBlend,
    /// 不透明度（[0,1]，越界由 `prepare` 钳制）。
    pub opacity: f32,
    /// 该层在屏幕上的像素矩形 `(left, top, width, height)`。
    ///
    /// 仅作声明/诊断：四边形本身是全 NDC（egui-wgpu 已为该回调设好 viewport），
    /// 因此合成器不消费此字段来做变换。**也没有 ppp 换算问题**：图层离屏纹理
    /// 按物理像素建（2D 走 `Frame2DGeometry::viewport_px()`、3D 走 `w_pt × ppp`），
    /// 而 egui-wgpu 给回调设的 render pass viewport 也是同一套物理像素矩形，
    /// 于是「全 NDC 四边形 == 该视口」这一步自动对齐，无需再做任何换算。
    pub rect_px: (f32, f32, f32, f32),
}

/// 源纹理域 → 主 pass 目标域的色彩转换（决定用哪个 fragment 入口）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Conversion {
    /// 两侧同域：采样值原样输出（逐字节保持图层内容）。
    Identity,
    /// 源存 gamma 值、目标 sRGB → 先 gamma→linear（硬件随后编码）。
    GammaToLinear,
    /// 源为 sRGB 格式（采样已解码）、目标非 sRGB → 先 linear→gamma。
    LinearToGamma,
}

impl Conversion {
    /// 由源格式与目标格式的 sRGB 属性决定。
    pub fn for_formats(src: wgpu::TextureFormat, target: wgpu::TextureFormat) -> Self {
        match (src.is_srgb(), target.is_srgb()) {
            (false, true) => Conversion::GammaToLinear,
            (true, false) => Conversion::LinearToGamma,
            _ => Conversion::Identity,
        }
    }

    fn entry_point(self) -> &'static str {
        match self {
            Conversion::Identity => "fs_identity",
            Conversion::GammaToLinear => "fs_gamma_to_linear",
            Conversion::LinearToGamma => "fs_linear_to_gamma",
        }
    }
}

/// 已准备的合成帧（`prepare` 产出、`composite_layers` 消费）。
///
/// 顺序 = 输入 `ops` 顺序（先 = 下）。
///
/// `Default` = **空帧**（W1：合成器缺失时帧体要能产出"什么都没画"的合法结果，
/// 而不是让调用方去构造一个不可能的实例）。
#[derive(Default)]
pub struct CompositeFrame {
    ops: Vec<PreparedCompositeOp>,
}

struct PreparedCompositeOp {
    bind_group: wgpu::BindGroup,
    /// (混合方式, 域转换) —— 绘制时据此选管线。
    key: (LayerBlend, Conversion),
    /// 保活：bind group 引用的 opacity uniform。
    _opacity_buffer: wgpu::Buffer,
}

impl CompositeFrame {
    /// 合成帧中的图层数。
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// 是否为空帧。
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// 第 `index` 层的混合方式（按绘制顺序）。越界返回 `None`。
    pub fn blend_at(&self, index: usize) -> Option<LayerBlend> {
        self.ops.get(index).map(|o| o.key.0)
    }

    /// 第 `index` 层的域转换（按绘制顺序）。越界返回 `None`。
    pub fn conversion_at(&self, index: usize) -> Option<Conversion> {
        self.ops.get(index).map(|o| o.key.1)
    }
}

/// 图层合成器：按 `(LayerBlend, Conversion)` **惰性**建/缓存管线 + 采样器 + 全屏四边形。
///
/// # C3：管线惰性化（实测结论）
///
/// 早期版本在 `new` 里**全量预建** 9 条（3 blend × 3 域转换）。实际命中组合远少于此
/// （app 只用 `Alpha` + 1 种域转换）⇒ 改为 `ensure_for` 现场建、建后入表。
///
/// 关于调用点约束（计划 §8.7.3 要求先核查）：`paint` 拿到的是
/// `&egui_wgpu::CallbackResources`（**不可变**），所以**不能**把 `composite_layers`
/// 改成 `&mut self`。本实现的落点是：**在 `prepare`（有 `&mut`）里 `ensure_for`**，
/// `composite_layers` 签名与借用形式**完全不变** —— 既不需要内部可变性（`RefCell`），
/// 也不需要改回调签名。
pub struct Compositor {
    /// 主 pass 目标格式（域转换的参照）。
    target_format: wgpu::TextureFormat,
    bind_group_layout: wgpu::BindGroupLayout,
    /// 合成管线的布局（惰性建管线时复用）。
    pipelines_layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    quad: wgpu::Buffer,
    /// 已建的 `(blend, 域转换)` 管线（惰性填充）。
    pipelines: HashMap<(LayerBlend, Conversion), wgpu::RenderPipeline>,
    /// 累计建的管线条数（C3 记账：首帧开销的证据）。
    pipeline_builds: usize,
    /// 滤镜链管线（单条：输入 = 中间纹理、输出 = 主 pass）。
    post_bind_group_layout: wgpu::BindGroupLayout,
    post_pipeline: wgpu::RenderPipeline,
    /// 中间全屏纹理（按视口物理像素尺寸惰性重建）。
    post_target: Option<PostTarget>,
}

/// 中间全屏纹理（post 路径的渲染目标 = 滤镜链的输入）。
///
/// 格式与主 pass 目标**一致** ⇒ 中间层不做域转换（少一次转换，见 §8.4.3）。
struct PostTarget {
    /// 纹理本体（保活：`view` 内部已持引用，此处仅为显式持有与尺寸记账）。
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

/// 滤镜链的一次 blit 所需的 GPU 资源（`prepare` 建、`paint` 用）。
///
/// **保活**：bind group 引用 uniform，uniform 必须活到 `paint`（同 `CompositeFrame`
/// 的 opacity uniform 纪律）。
#[derive(Debug)]
pub struct PostBlit {
    bind_group: wgpu::BindGroup,
    _uniform: wgpu::Buffer,
}

impl Compositor {
    /// 构造合成器（`target_format` = 主 pass 目标格式，决定 sRGB / 非 sRGB 入口）。
    ///
    /// **不再预建**任何合成管线（C3）；管线由 [`Compositor::ensure_for`] 按需建立。
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xcal-compositor-bgl"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(OPACITY_UNIFORM_SIZE),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xcal-compositor-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xcal-compositor-shader"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_SHADER.into()),
        });
        // 顶点布局同样来自契约（position = 裁剪坐标、attrib0 = UV）：
        // 四边形是静态数据，缓存策略声明为 `Static`。
        let vertex_layout = VertexLayout::standard(AaModel::None, CacheStrategy::Static);
        let mut attrs = Vec::new();
        let wgpu_vertex_layout = to_wgpu(&vertex_layout, &mut attrs);
        drop(shader); // 合成管线惰性建：着色器在 `ensure_for` 里按需创建

        // 3 blend × 3 域转换**不再预建**（C3）：`ensure_for` 现场建、建后入表。
        let pipelines = HashMap::new();

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("xcal-compositor-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("xcal-compositor-quad"),
            contents: bytes_of(&COMPOSITE_QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // ── 滤镜链管线（C1）：输入 = 中间纹理、输出 = 主 pass ──
        let post_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("xcal-post-bgl"),
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
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: std::num::NonZeroU64::new(POST_UNIFORM_SIZE),
                        },
                        count: None,
                    },
                ],
            });
        let post_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xcal-post-pipeline-layout"),
            bind_group_layouts: &[Some(&post_bind_group_layout)],
            immediate_size: 0,
        });
        let post_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xcal-post-shader"),
            source: wgpu::ShaderSource::Wgsl(POST_SHADER.into()),
        });
        let post_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("xcal-post-pipeline"),
            layout: Some(&post_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &post_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu_vertex_layout.clone())],
            },
            fragment: Some(wgpu::FragmentState {
                module: &post_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                // **混合方式 = 预乘 alpha over**：中间纹理里是"全部图层合成后的预乘结果"，
                // 把它 over 到主 pass（egui 已铺好的背景）上，与"逐层直接 over 到主 pass"
                // 等价（预乘 over 满足结合律；⚠️ 仅当首层不是 `Opaque`(REPLACE) 时严格成立
                // —— app 的图层一律声明 `Alpha`，见 `build_scene_view`）。
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(blend_state(LayerBlend::Alpha)),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            target_format,
            bind_group_layout,
            pipelines_layout: pipeline_layout,
            sampler,
            quad,
            pipelines,
            pipeline_builds: 0,
            post_bind_group_layout,
            post_pipeline,
            post_target: None,
        }
    }

    /// **C3**：为本帧的 `ops` 现场建立缺的 `(blend, 域转换)` 管线（建后缓存）。
    ///
    /// 返回本次新建的条数（0 = 全部命中）。只在 `prepare`（`&mut` 可用）里调用。
    pub fn ensure_for(&mut self, device: &wgpu::Device, ops: &[CompositeOp]) -> usize {
        let mut built = 0;
        for op in ops {
            let key = (
                op.blend,
                Conversion::for_formats(op.src_format, self.target_format),
            );
            if self.pipelines.contains_key(&key) {
                continue;
            }
            let vertex_layout = VertexLayout::standard(AaModel::None, CacheStrategy::Static);
            let mut attrs = Vec::new();
            let wgpu_vertex_layout = to_wgpu(&vertex_layout, &mut attrs);
            let pipeline = make_composite_pipeline(
                device,
                &self.bind_group_layout,
                &self.pipelines_layout,
                self.target_format,
                key,
                wgpu_vertex_layout,
            );
            self.pipelines.insert(key, pipeline);
            built += 1;
        }
        if built > 0 {
            self.pipeline_builds += built;
        }
        built
    }

    /// 已缓存的管线条数（C3 记账）。
    pub fn pipeline_count(&self) -> usize {
        self.pipelines.len()
    }

    /// 累计建的管线条数（C3 记账：首帧开销证据）。
    pub fn pipeline_builds(&self) -> usize {
        self.pipeline_builds
    }

    /// **C0/C1**：把本帧全部图层合成到**中间全屏纹理**（清屏为透明），返回其视图。
    ///
    /// 中间纹理格式 = 主 pass 目标格式 ⇒ 合成时的域转换判定与直通路径**同一套**
    /// （`Conversion::for_formats(src, target_format)` 不变），且 blit 时不再转换。
    pub fn render_post(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        size_px: (u32, u32),
        frame: &CompositeFrame,
    ) -> wgpu::TextureView {
        let view = self.ensure_post_target(device, size_px).clone();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("xcal-post-composite-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // 与 2D/3D 离屏同口径：**透明**清屏（预乘 0）。
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        self.composite_layers(&mut pass, frame);
        drop(pass);
        view
    }

    /// **C1**：为滤镜链准备一次 blit 的绑定资源（uniform 编码 + bind group）。
    ///
    /// `texel_px`（1/宽, 1/高）随 uniform 一起给 shader —— 邻域滤镜需要它。
    pub fn build_post_blit(
        &self,
        device: &wgpu::Device,
        chain: &PostFilterChain,
        src: &wgpu::TextureView,
        size_px: (u32, u32),
    ) -> PostBlit {
        let texel = [1.0 / size_px.0.max(1) as f32, 1.0 / size_px.1.max(1) as f32];
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("xcal-post-uniform"),
            contents: &chain.to_uniform_bytes(texel),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xcal-post-bind-group"),
            layout: &self.post_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        PostBlit {
            bind_group,
            _uniform: uniform,
        }
    }

    /// **C1**：把中间纹理经滤镜链 blit 进当前 pass（一次全屏 draw）。
    pub fn blit_post(&self, pass: &mut wgpu::RenderPass<'_>, blit: &PostBlit) {
        pass.set_pipeline(&self.post_pipeline);
        pass.set_bind_group(0, &blit.bind_group, &[]);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.draw(0..COMPOSITE_QUAD.len() as u32, 0..1);
    }

    /// 惰性（重）建中间全屏纹理：尺寸变化时重建。
    fn ensure_post_target(
        &mut self,
        device: &wgpu::Device,
        size: (u32, u32),
    ) -> &wgpu::TextureView {
        let size = (size.0.max(1), size.1.max(1));
        if self.post_target.as_ref().map(|t| t.size) != Some(size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("xcal-post-intermediate"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.target_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.post_target = Some(PostTarget {
                _texture: texture,
                view,
                size,
            });
        }
        &self.post_target.as_ref().expect("刚建好").view
    }

    /// 把声明式的 [`CompositeOp`] 列表准备成 GPU 资源（逐层一条 opacity uniform + 绑定组）。
    ///
    /// 顺序**原样保留**（不排序、不去重）：索引 0 = 最下层。
    pub fn prepare(&self, device: &wgpu::Device, ops: &[CompositeOp]) -> CompositeFrame {
        let prepared = ops
            .iter()
            .map(|op| {
                let opacity = op.opacity.clamp(0.0, 1.0);
                let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("xcal-compositor-opacity"),
                    contents: bytes_of(&[opacity, 0.0, 0.0, 0.0]),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("xcal-compositor-bind-group"),
                    layout: &self.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&op.texture),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: uniform.as_entire_binding(),
                        },
                    ],
                });
                PreparedCompositeOp {
                    bind_group,
                    key: (
                        op.blend,
                        Conversion::for_formats(op.src_format, self.target_format),
                    ),
                    _opacity_buffer: uniform,
                }
            })
            .collect();

        CompositeFrame { ops: prepared }
    }

    /// 按序把所有图层合成进当前 render pass。
    ///
    /// 调用方负责在此之前设好 scissor / viewport（egui 回调路径由宿主设）。
    pub fn composite_layers(&self, pass: &mut wgpu::RenderPass<'_>, frame: &CompositeFrame) {
        if frame.is_empty() {
            return;
        }
        pass.set_vertex_buffer(0, self.quad.slice(..));
        for op in &frame.ops {
            let Some(pipeline) = self.pipelines.get(&op.key) else {
                continue;
            };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &op.bind_group, &[]);
            pass.draw(0..COMPOSITE_QUAD.len() as u32, 0..1);
        }
    }

    /// 单图层便捷入口（一次 prepare + 一次 draw）。多图层请用
    /// [`Compositor::prepare`] + [`Compositor::composite_layers`]。
    pub fn composite_one(
        &self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'_>,
        texture: wgpu::TextureView,
        src_format: wgpu::TextureFormat,
        blend: LayerBlend,
        opacity: f32,
    ) {
        let frame = self.prepare(
            device,
            &[CompositeOp {
                texture,
                src_format,
                blend,
                opacity,
                rect_px: (0.0, 0.0, 0.0, 0.0),
            }],
        );
        self.composite_layers(pass, &frame);
    }
}

/// 建一条合成管线（`(blend, 域转换)` → 一条 pipeline；wgpu 无动态 blend / 无动态入口点）。
fn make_composite_pipeline<'a>(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
    pipeline_layout: &wgpu::PipelineLayout,
    target_format: wgpu::TextureFormat,
    key: (LayerBlend, Conversion),
    wgpu_vertex_layout: wgpu::VertexBufferLayout<'a>,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("xcal-compositor-shader"),
        source: wgpu::ShaderSource::Wgsl(COMPOSITE_SHADER.into()),
    });
    let _ = bind_group_layout; // 布局已固化在 pipeline_layout 里（仅作可读性参数）
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("xcal-compositor-pipeline"),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu_vertex_layout)],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some(key.1.entry_point()),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: Some(blend_state(key.0)),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        // 与 egui 主 pass 的采样数一致（主 pass 为 1x，同 2D/3D 回调口径）。
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// [`LayerBlend`] → wgpu 混合状态。
///
/// - `Opaque`：`REPLACE`（该层完全覆盖下层，要求图层纹理自身不透明）；
/// - `Alpha`：预乘 alpha（`One / OneMinusSrcAlpha`）；
/// - `Additive`：`One / One`。
fn blend_state(blend: LayerBlend) -> wgpu::BlendState {
    match blend {
        LayerBlend::Opaque => wgpu::BlendState::REPLACE,
        LayerBlend::Alpha => wgpu::BlendState {
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
        },
        LayerBlend::Additive => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `FilterSlot` 只在**测试**里直接构造（库内路径只经 `PostFilterChain`）。
    use xcal_render::FilterSlot;

    /// 三组 blend 的即时语义（wgpu 无动态 blend，必须逐 blend 预建 pipeline）。
    #[test]
    fn blend_state_matches_declared_semantics() {
        // Opaque = REPLACE。
        assert_eq!(blend_state(LayerBlend::Opaque), wgpu::BlendState::REPLACE);
        // Alpha = 预乘 alpha。
        let alpha = blend_state(LayerBlend::Alpha);
        assert_eq!(alpha.color.src_factor, wgpu::BlendFactor::One);
        assert_eq!(alpha.color.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);
        assert_eq!(alpha.alpha.src_factor, wgpu::BlendFactor::One);
        assert_eq!(alpha.alpha.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);
        // Additive = One/One。
        let additive = blend_state(LayerBlend::Additive);
        assert_eq!(additive.color.src_factor, wgpu::BlendFactor::One);
        assert_eq!(additive.color.dst_factor, wgpu::BlendFactor::One);
        assert_eq!(additive.alpha.src_factor, wgpu::BlendFactor::One);
        assert_eq!(additive.alpha.dst_factor, wgpu::BlendFactor::One);
        // 三者互不相同（否则 HashMap 会互相覆盖）。
        assert_ne!(
            blend_state(LayerBlend::Alpha),
            blend_state(LayerBlend::Additive)
        );
    }

    /// 四边形：4 顶点、全 NDC 覆盖、UV (0,0)-(1,1)（契约布局 R0 后 stride 24）。
    #[test]
    fn composite_quad_is_full_ndc_box_with_uv() {
        assert_eq!(COMPOSITE_QUAD.len(), 4);
        // TriangleStrip 顶点序：左上 → 右上 → 左下 → 右下。
        assert_eq!(COMPOSITE_QUAD[0].position, [-1.0, 1.0]);
        assert_eq!(COMPOSITE_QUAD[1].position, [1.0, 1.0]);
        assert_eq!(COMPOSITE_QUAD[2].position, [-1.0, -1.0]);
        assert_eq!(COMPOSITE_QUAD[3].position, [1.0, -1.0]);
        // UV 覆盖 0..1。
        assert_eq!(COMPOSITE_QUAD[0].normal, [0.0, 0.0]);
        assert_eq!(COMPOSITE_QUAD[1].normal, [1.0, 0.0]);
        assert_eq!(COMPOSITE_QUAD[2].normal, [0.0, 1.0]);
        assert_eq!(COMPOSITE_QUAD[3].normal, [1.0, 1.0]);
        for v in &COMPOSITE_QUAD {
            assert_eq!(v.side, 0.0, "attrib1 不参与合成，必须为 0");
            // R0：attrib2（advance）同样不参与合成 —— 合成四边形**不得**被 reveal 裁掉
            // （区间 `[0,0]` 时 `advance=0` 仍在区间内，但语义上是"无 reveal"）。
            assert_eq!(v.advance, 0.0, "attrib2 不参与合成，必须为 0");
        }
    }

    /// 顶点字节布局与契约一致（position 8B + UV 8B + attrib1 4B + attrib2 4B
    /// = 24B/顶点）。
    #[test]
    fn quad_bytes_are_twenty_four_per_vertex() {
        assert_eq!(bytes_of(&COMPOSITE_QUAD).len(), 4 * 24);
    }

    /// **回归**（"2D 视图在 X 方向被压扁"）：egui-wgpu 会为每个回调把 render pass 的
    /// viewport 设成该回调矩形，所以全 NDC 四边形在**任意**视口位置/尺寸下都正好铺满
    /// 该视口。这里按 wgpu 的 viewport 变换把顶点换算回帧缓冲像素，断言边界一致。
    #[test]
    fn composite_quad_fills_callback_viewport_at_any_position() {
        // (left, top, w, h)（像素），含"左侧有控制面板"的偏移形态。
        for (left, top, w, h) in [
            (100.0f32, 50.0f32, 300.0f32, 200.0f32),
            (1100.0, 50.0, 300.0, 200.0),
            (200.0, 120.0, 900.0, 700.0),
        ] {
            let to_fb = |ndc: [f32; 2]| {
                let x = left + (ndc[0] + 1.0) * 0.5 * w;
                let y = top + (1.0 - ndc[1]) * 0.5 * h;
                (x.round(), y.round())
            };
            let (x_l, y_t) = to_fb(COMPOSITE_QUAD[0].position);
            let (x_r, y_b) = to_fb(COMPOSITE_QUAD[3].position);
            assert_eq!(x_l, left.round());
            assert_eq!(y_t, top.round());
            assert_eq!(x_r, x_l + w.round());
            assert_eq!(y_b, y_t + h.round());
        }
    }

    // ── 纯 CPU：域转换选择（决定 fragment 入口） ────────────────────────────

    /// 源/目标 sRGB 属性 → 域转换：同域 `Identity`、源 gamma→sRGB 目标 `GammaToLinear`、
    /// 源 sRGB→非 sRGB 目标 `LinearToGamma`。
    #[test]
    fn conversion_for_formats_matches_srgb_domains() {
        use wgpu::TextureFormat::{Bgra8Unorm, Bgra8UnormSrgb, Rgba8Unorm, Rgba8UnormSrgb};
        // 同域（含非 sRGB ↔ 非 sRGB、sRGB ↔ sRGB）：原样输出。
        assert_eq!(
            Conversion::for_formats(Rgba8Unorm, Bgra8Unorm),
            Conversion::Identity
        );
        assert_eq!(
            Conversion::for_formats(Rgba8Unorm, Rgba8Unorm),
            Conversion::Identity
        );
        assert_eq!(
            Conversion::for_formats(Bgra8UnormSrgb, Rgba8UnormSrgb),
            Conversion::Identity
        );
        // 2D 离屏（非 sRGB 存 gamma）→ sRGB 主目标：采样值即 gamma，需解码。
        assert_eq!(
            Conversion::for_formats(Rgba8Unorm, Bgra8UnormSrgb),
            Conversion::GammaToLinear
        );
        // 3D 离屏（sRGB 格式，采样已解码）→ 非 sRGB 主目标：需再编码。
        assert_eq!(
            Conversion::for_formats(Bgra8UnormSrgb, Rgba8Unorm),
            Conversion::LinearToGamma
        );
        // 三个入口点各不相同。
        assert_eq!(Conversion::Identity.entry_point(), "fs_identity");
        assert_eq!(
            Conversion::GammaToLinear.entry_point(),
            "fs_gamma_to_linear"
        );
        assert_eq!(
            Conversion::LinearToGamma.entry_point(),
            "fs_linear_to_gamma"
        );
    }

    // ── 真实设备：顺序 / opacity / 像素结果 ────────────────────────────────

    const TEST_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    /// 建一张 1×1 常色源纹理（线性采样下整屏都是该色）。
    fn solid_texture(ctx: &crate::RenderContext, rgba: [u8; 4]) -> wgpu::TextureView {
        solid_texture_fmt(ctx, TEST_FORMAT, rgba)
    }

    /// 建一张指定格式的 1×1 常色源纹理（用于混合域回归用例）。
    fn solid_texture_fmt(
        ctx: &crate::RenderContext,
        format: wgpu::TextureFormat,
        rgba: [u8; 4],
    ) -> wgpu::TextureView {
        let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xcal-compositor-src"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        tex.create_view(&wgpu::TextureViewDescriptor::default())
    }

    fn op(texture: wgpu::TextureView, blend: LayerBlend, opacity: f32) -> CompositeOp {
        op_fmt(texture, TEST_FORMAT, blend, opacity)
    }

    /// 指定源格式的 [`CompositeOp`]（混合域用例）。
    fn op_fmt(
        texture: wgpu::TextureView,
        src_format: wgpu::TextureFormat,
        blend: LayerBlend,
        opacity: f32,
    ) -> CompositeOp {
        CompositeOp {
            texture,
            src_format,
            blend,
            opacity,
            rect_px: (0.0, 0.0, 4.0, 4.0),
        }
    }

    /// 采集 (0,0) 像素：按给定 ops 合成到 4×4 目标（先清成不透明黑）。
    ///
    /// C3：先 `ensure_for` 现场建管线 —— 管线惰性化后，合成前必须让本条链路可用。
    fn composite_pixel(
        ctx: &crate::RenderContext,
        comp: &mut Compositor,
        ops: &[CompositeOp],
    ) -> [u8; 4] {
        let _ = comp.ensure_for(&ctx.device, ops);
        let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xcal-compositor-target"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEST_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let frame = comp.prepare(&ctx.device, ops);

        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xcal-compositor-test-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            comp.composite_layers(&mut pass, &frame);
        }
        // 先提交合成 pass（回读是**另一次** encoder+submit）。
        ctx.submit(enc.finish());

        // 回读：W1 起统一走 `RenderContext::read_texture`（256 对齐的行距处理在那边，
        // 本模块的测试副本已消除）。取左上角像素。
        let pixels = ctx.read_texture(&target, (4, 4));
        [pixels[0], pixels[1], pixels[2], pixels[3]]
    }

    /// 无可用适配器时跳过（与仓库既有 headless 约定一致）。
    fn headless() -> Option<crate::RenderContext> {
        crate::RenderContext::headless_with_format(TEST_FORMAT).ok()
    }

    /// C4 验收（真实设备）：N 层**按输入顺序**合成，三种 blend 各有对应 pipeline。
    #[test]
    fn gpu_composite_respects_declaration_order() {
        let Some(ctx) = headless() else {
            return;
        };
        let mut comp = Compositor::new(&ctx.device, TEST_FORMAT);
        // 红（底层，Alpha / 1.0）+ 蓝（上层，Alpha / 0.5）
        let red = solid_texture(&ctx, [255, 0, 0, 255]);
        let blue = solid_texture(&ctx, [0, 0, 255, 255]);
        let ops = [
            op(red, LayerBlend::Alpha, 1.0),
            op(blue, LayerBlend::Alpha, 0.5),
        ];
        let frame = comp.prepare(&ctx.device, &ops);
        // 顺序原样保留（不重排）。
        assert_eq!(frame.blend_at(0), Some(LayerBlend::Alpha));
        assert_eq!(frame.blend_at(1), Some(LayerBlend::Alpha));

        // 预乘 alpha：蓝 ×0.5 叠加到红上 → (0.5, 0, 0.5)。
        let px = composite_pixel(&ctx, &mut comp, &ops);
        assert!(
            (px[0] as i32 - 128).abs() <= 2 && px[1] <= 2 && (px[2] as i32 - 128).abs() <= 2,
            "顺序/半透明混合结果应为 (128,0,128)±2，实得 {px:?}"
        );

        // 反序（蓝在下、红在上）→ 结果不同，证明顺序真的生效。
        // 红在上且 opacity = 1（源 alpha = 1）→ 完全覆盖，结果应为纯红。
        let red2 = solid_texture(&ctx, [255, 0, 0, 255]);
        let blue2 = solid_texture(&ctx, [0, 0, 255, 255]);
        let reversed = [
            op(blue2, LayerBlend::Alpha, 0.5),
            op(red2, LayerBlend::Alpha, 1.0),
        ];
        let px_rev = composite_pixel(&ctx, &mut comp, &reversed);
        assert!(
            (px_rev[0] as i32 - 255).abs() <= 2 && px_rev[1] <= 2 && px_rev[2] <= 2,
            "反序结果应为 (255,0,0)±2（红在上、完全不透明），实得 {px_rev:?}"
        );
        assert_ne!(px, px_rev, "顺序必须影响合成结果");
    }

    /// **回归（S3-2 修复）**：多层 `CompositeOp` 必须进**同一个** `CompositeFrame`
    /// 且**顺序不重排**；混合域（2D 非 sRGB 源 + 3D sRGB 源）各层选对自己的域转换，
    /// 并在一次 `composite_layers` 内跑通（这正是实机 2D/3D 同帧共存的路径）。
    #[test]
    fn gpu_composite_mixed_domain_ops_stay_in_one_frame_in_order() {
        let Some(ctx) = headless() else {
            return;
        };
        // 主目标为非 sRGB（与多数 eframe 平台口径一致）。
        let mut comp = Compositor::new(&ctx.device, TEST_FORMAT);

        // 下层：2D 离屏（非 sRGB，存 gamma）→ Identity。
        // 上层：3D 离屏（sRGB 格式，采样已解码）→ LinearToGamma。
        let ops = [
            op_fmt(
                solid_texture_fmt(&ctx, TEST_FORMAT, [255, 0, 0, 255]),
                TEST_FORMAT,
                LayerBlend::Alpha,
                1.0,
            ),
            op_fmt(
                solid_texture_fmt(&ctx, wgpu::TextureFormat::Rgba8UnormSrgb, [0, 0, 255, 255]),
                wgpu::TextureFormat::Rgba8UnormSrgb,
                LayerBlend::Alpha,
                1.0,
            ),
        ];
        let frame = comp.prepare(&ctx.device, &ops);

        assert_eq!(frame.len(), 2, "全部图层必须进同一个 CompositeFrame");
        // 顺序原样保留（先 = 下）。
        assert_eq!(frame.conversion_at(0), Some(Conversion::Identity));
        assert_eq!(frame.conversion_at(1), Some(Conversion::LinearToGamma));
        assert_eq!(frame.blend_at(0), Some(LayerBlend::Alpha));
        assert_eq!(frame.blend_at(1), Some(LayerBlend::Alpha));

        // 单帧内真的跑通：上层不透明蓝完全覆盖下层红。
        let px = composite_pixel(&ctx, &mut comp, &ops);
        assert!(
            px[0] <= 8 && px[1] <= 8 && px[2] >= 200,
            "混合域同帧合成后应为蓝（上层覆盖），实得 {px:?}"
        );
    }

    /// C4 验收（真实设备）：`opacity=0` 该层不可见，`opacity=1` 完全可见。
    #[test]
    fn gpu_composite_opacity_zero_invisible_one_opaque() {
        let Some(ctx) = headless() else {
            return;
        };
        let mut comp = Compositor::new(&ctx.device, TEST_FORMAT);

        // 不透明红打底 + 蓝色上层。
        let px_alpha_0 = composite_pixel(
            &ctx,
            &mut comp,
            &[
                op(
                    solid_texture(&ctx, [255, 0, 0, 255]),
                    LayerBlend::Alpha,
                    1.0,
                ),
                op(
                    solid_texture(&ctx, [0, 0, 255, 255]),
                    LayerBlend::Alpha,
                    0.0,
                ),
            ],
        );
        assert_eq!(
            px_alpha_0,
            [255, 0, 0, 255],
            "opacity=0 时该层应完全不可见（下层原样保留）"
        );

        let px_alpha_1 = composite_pixel(
            &ctx,
            &mut comp,
            &[
                op(
                    solid_texture(&ctx, [255, 0, 0, 255]),
                    LayerBlend::Alpha,
                    1.0,
                ),
                op(
                    solid_texture(&ctx, [0, 0, 255, 255]),
                    LayerBlend::Alpha,
                    1.0,
                ),
            ],
        );
        assert_eq!(
            px_alpha_1,
            [0, 0, 255, 255],
            "opacity=1 时该层应完全覆盖下层"
        );

        // 越界 opacity 被钳制（2.0 → 1.0）。
        let px_clamped = composite_pixel(
            &ctx,
            &mut comp,
            &[
                op(
                    solid_texture(&ctx, [255, 0, 0, 255]),
                    LayerBlend::Alpha,
                    1.0,
                ),
                op(
                    solid_texture(&ctx, [0, 0, 255, 255]),
                    LayerBlend::Alpha,
                    2.0,
                ),
            ],
        );
        assert_eq!(px_clamped, [0, 0, 255, 255], "opacity 应被钳制到 [0,1]");
    }

    /// C4 验收（真实设备）：三种 blend 都能建出 pipeline 并跑通（空白帧也不例外）。
    #[test]
    fn gpu_composite_builds_all_three_blend_pipelines() {
        let Some(ctx) = headless() else {
            return;
        };
        // sRGB 与非 sRGB 主目标各建一次（eframe 的目标格式依平台而定）。
        for format in [TEST_FORMAT, wgpu::TextureFormat::Bgra8UnormSrgb] {
            let comp = Compositor::new(&ctx.device, format);
            for blend in [LayerBlend::Opaque, LayerBlend::Alpha, LayerBlend::Additive] {
                let ops = [op(solid_texture(&ctx, [255, 0, 0, 255]), blend, 1.0)];
                let frame = comp.prepare(&ctx.device, &ops);
                assert_eq!(frame.blend_at(0), Some(blend));
                // 空帧不画。
                let empty = comp.prepare(&ctx.device, &[]);
                assert!(empty.is_empty());
            }
        }
    }

    /// C4 验收（真实设备）：`CompositeFrame` 的层序 = `CompositeOp` 输入顺序（不得重排）。
    /// 用三个互不相同的 blend 作标记，任意重排都会改变 `blend_at` 序列。
    #[test]
    fn gpu_composite_preserves_op_order_with_distinct_blends() {
        let Some(ctx) = headless() else {
            return;
        };
        let comp = Compositor::new(&ctx.device, TEST_FORMAT);
        let ops = [
            op(
                solid_texture(&ctx, [255, 0, 0, 255]),
                LayerBlend::Opaque,
                1.0,
            ),
            op(
                solid_texture(&ctx, [0, 255, 0, 255]),
                LayerBlend::Alpha,
                1.0,
            ),
            op(
                solid_texture(&ctx, [0, 0, 255, 255]),
                LayerBlend::Additive,
                1.0,
            ),
        ];
        let frame = comp.prepare(&ctx.device, &ops);
        assert_eq!(frame.len(), 3);
        assert_eq!(frame.blend_at(0), Some(LayerBlend::Opaque));
        assert_eq!(frame.blend_at(1), Some(LayerBlend::Alpha));
        assert_eq!(frame.blend_at(2), Some(LayerBlend::Additive));
        // 同域（Rgba8Unorm 源 → Rgba8Unorm 目标）→ Identity。
        assert_eq!(frame.conversion_at(0), Some(Conversion::Identity));
        assert_eq!(frame.conversion_at(2), Some(Conversion::Identity));
    }

    // ── C 线：post 路径 / 滤镜链 / 惰性管线 ────────────────────────────────

    /// 建一张 `size×size` 的可回读目标纹理。
    fn target_texture(ctx: &crate::RenderContext, size: u32) -> wgpu::Texture {
        ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xcal-post-test-target"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEST_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    /// 回读 `size×size` 纹理的全部像素（`size = 64` ⇒ `64×4 = 256` 恰好对齐）。
    fn read_all(ctx: &crate::RenderContext, texture: &wgpu::Texture, size: u32) -> Vec<u8> {
        let padded = (size * 4).div_ceil(256) * 256;
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xcal-post-test-readback"),
            size: (padded * size) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
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
                    rows_per_image: Some(size),
                },
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue.submit([enc.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map_async 失败"));
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll 失败");
        let data = slice.get_mapped_range().expect("映射失败");
        let mut out = Vec::with_capacity((size * size * 4) as usize);
        for row in 0..size {
            let start = (row * padded) as usize;
            out.extend_from_slice(&data[start..start + (size * 4) as usize]);
        }
        drop(data);
        buffer.unmap();
        out
    }

    /// 像素 `(x, y)`（左上角为原点）。
    fn px_of(pixels: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        pixels[i..i + 4].try_into().expect("pixel bytes")
    }

    /// 建一张带图案的源纹理（逐像素给色）。`width*4` 必须 256 对齐 ⇒ 用 64。
    fn patterned_texture(
        ctx: &crate::RenderContext,
        size: u32,
        f: impl Fn(u32, u32) -> [u8; 4],
    ) -> wgpu::TextureView {
        let mut data = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                data.extend_from_slice(&f(x, y));
            }
        }
        let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xcal-post-test-src"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEST_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        tex.create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// 走 **post 路径**（中间纹理 + 滤镜链 blit）渲染到 `size×size` 目标，返回目标像素。
    ///
    /// `clear`：主目标清屏色（`alpha = 0` ⇒ 读到的就是滤镜输出的预乘值本身，
    /// 用于检验预乘不变量；`alpha = 1` ⇒ 与直通路径同起点）。
    fn post_route_pixels(
        ctx: &crate::RenderContext,
        comp: &mut Compositor,
        ops: &[CompositeOp],
        chain: &PostFilterChain,
        clear: wgpu::Color,
        size: u32,
    ) -> Vec<u8> {
        let _ = comp.ensure_for(&ctx.device, ops);
        let frame = comp.prepare(&ctx.device, ops);
        let target = target_texture(ctx, size);
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let src = comp.render_post(&ctx.device, &mut enc, (size, size), &frame);
        let blit = comp.build_post_blit(&ctx.device, chain, &src, (size, size));
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xcal-post-test-main-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            comp.blit_post(&mut pass, &blit);
        }
        ctx.queue.submit([enc.finish()]);
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll 失败");
        read_all(ctx, &target, size)
    }

    /// 链：[`FilterSlot::none`]（恒等）。
    fn identity_chain() -> PostFilterChain {
        let mut c = PostFilterChain::new();
        c.push(FilterSlot::none());
        c
    }

    /// **C0 验收（§8.4.4 / §11.6）**：空链走直通；**恒等滤镜槽**（真实走一遍
    /// 中间纹理 + 全屏 pass）与直通**逐位一致** ⇒ 中间纹理路径本身不改变画面。
    #[test]
    fn gpu_post_route_with_identity_filter_matches_direct_bit_exactly() {
        let Some(ctx) = headless() else {
            return;
        };
        let mut comp = Compositor::new(&ctx.device, TEST_FORMAT);
        let ops = [
            op(
                solid_texture(&ctx, [255, 0, 0, 255]),
                LayerBlend::Alpha,
                1.0,
            ),
            op(
                solid_texture(&ctx, [0, 0, 255, 255]),
                LayerBlend::Alpha,
                0.5,
            ),
        ];
        // 直通：合成到不透明黑底（既有路径）。
        let direct = composite_pixel(&ctx, &mut comp, &ops);
        // post：合成到中间纹理（透明清屏）→ 恒等滤镜 → blit 到**同起点**的主目标。
        let post = post_route_pixels(
            &ctx,
            &mut comp,
            &ops,
            &identity_chain(),
            wgpu::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            4,
        );
        assert_eq!(
            px_of(&post, 4, 0, 0),
            direct,
            "恒等滤镜（中间纹理路径）必须与直通路径逐位一致"
        );
    }

    /// **C2 验收（§8.6）**：三类代表性滤镜各改变画面，且点运算按**预乘不变量**缩放。
    ///
    /// 源图案：左侧一根红色竖条（`x ∈ [4, 20]`，不透明），其余**全透明** ⇒ 边界处
    /// 同时检验"邻域滤镜是否在预乘空间做"（非预乘会出现黑边）。
    #[test]
    fn gpu_post_filters_point_geometry_neighborhood() {
        let Some(ctx) = headless() else {
            return;
        };
        const SIZE: u32 = 64;
        let mut comp = Compositor::new(&ctx.device, TEST_FORMAT);
        let src = patterned_texture(&ctx, SIZE, |x, _| {
            if (4..20).contains(&x) {
                [255, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let ops = [op_fmt(src, TEST_FORMAT, LayerBlend::Alpha, 1.0)];
        // 主目标透明清屏 ⇒ 读到的就是滤镜输出的**预乘值**（可验证 rgb ≤ a）。
        let transparent = wgpu::Color::TRANSPARENT;

        let base = post_route_pixels(&ctx, &mut comp, &ops, &identity_chain(), transparent, SIZE);
        // 基线：条内是纯红（预乘 1,0,0,1），条外全透明。
        assert_eq!(px_of(&base, SIZE, 12, 32), [255, 0, 0, 255]);
        assert_eq!(px_of(&base, SIZE, 40, 32), [0, 0, 0, 0]);

        // ── 点运算（亮度 0.5）：**rgb 与 a 同乘**（预乘不变量）──
        let mut brightness = PostFilterChain::new();
        brightness.push(FilterSlot::brightness(0.5));
        let dim = post_route_pixels(&ctx, &mut comp, &ops, &brightness, transparent, SIZE);
        let d = px_of(&dim, SIZE, 12, 32);
        assert!(
            (d[0] as i32 - 128).abs() <= 2 && (d[3] as i32 - 128).abs() <= 2,
            "亮度 0.5 应把预乘 rgb 与 a 同时减半，实得 {d:?}"
        );
        assert!(d[0] <= d[3], "预乘不变量 rgb ≤ a 必须保持：{d:?}");

        // ── 几何重映射（桶形畸变 k1 = 1.5）：紫外坐标移动 ⇒ 条的位置/宽度改变 ──
        let mut distort = PostFilterChain::new();
        distort.push(FilterSlot::distortion(1.5, 0.0, 1.0));
        let warped = post_route_pixels(&ctx, &mut comp, &ops, &distort, transparent, SIZE);
        assert_ne!(warped, base, "畸变必须改变画面（uv 重映射）");
        let count_red = |buf: &[u8]| {
            (0..SIZE * SIZE)
                .filter(|i| px_of(buf, SIZE, i % SIZE, i / SIZE)[0] > 128)
                .count()
        };
        assert_ne!(
            count_red(&warped),
            count_red(&base),
            "桶形畸变应改变红色区域的像素数（条被推宽/推窄）"
        );

        // ── 邻域（3×3 模糊）：边界出现半透明；**无黑边**（r 与 a 同步 ⇒ r/a 保持 1）──
        let mut blur = PostFilterChain::new();
        blur.push(FilterSlot::blur(1.0));
        let blurred = post_route_pixels(&ctx, &mut comp, &ops, &blur, transparent, SIZE);
        let edge = px_of(&blurred, SIZE, 20, 32);
        assert!(
            edge[3] > 0 && edge[3] < 255,
            "模糊应在条边界产生半透明像素，实得 {edge:?}"
        );
        assert!(
            (edge[0] as i32 - edge[3] as i32).abs() <= 2,
            "预乘空间模糊：r 应随 a 同步缩小（否则是黑边/halo），实得 {edge:?}"
        );
    }

    /// **C1 验收**：滤镜**不是管线维度** —— 用三个不同滤镜跑完整链路，合成管线数不变。
    ///
    /// 顺带记账 **C3**：首帧现场建（1 条）+ 次帧命中（0 条），并打印首建耗时。
    #[test]
    fn gpu_filters_add_no_pipelines_and_pipelines_are_lazy() {
        let Some(ctx) = headless() else {
            return;
        };
        let mut comp = Compositor::new(&ctx.device, TEST_FORMAT);
        let src = patterned_texture(&ctx, 64, |x, _| {
            if x < 32 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let ops = [op_fmt(src, TEST_FORMAT, LayerBlend::Alpha, 1.0)];

        // C3：首帧现场建。
        let t0 = std::time::Instant::now();
        let built = comp.ensure_for(&ctx.device, &ops);
        let first_ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(built, 1, "首帧应现场建 1 条合成管线");
        let t1 = std::time::Instant::now();
        let again = comp.ensure_for(&ctx.device, &ops);
        let hit_us = t1.elapsed().as_secs_f64() * 1e6;
        assert_eq!(again, 0, "同组合次帧必须命中缓存");
        eprintln!(
            "C3 记账：首建 {first_ms:.2} ms（debug 构建）、命中 {hit_us:.1} µs；累计建 {} 条",
            comp.pipeline_builds()
        );

        // C1：三种滤镜（含 4 槽上限）走一遍，**合成管线条数不变**（滤镜只在 uniform 里）。
        let mut chain = PostFilterChain::new();
        chain.push(FilterSlot::brightness(0.9));
        chain.push(FilterSlot::distortion(0.3, 0.1, 1.0));
        chain.push(FilterSlot::blur(0.5));
        chain.push(FilterSlot::none());
        assert_eq!(chain.slots.len(), 4);
        let pixels = post_route_pixels(
            &ctx,
            &mut comp,
            &ops,
            &chain,
            wgpu::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            64,
        );
        assert_eq!(pixels.len(), (64 * 64 * 4) as usize);
        assert_eq!(
            comp.pipeline_count(),
            1,
            "滤镜不得引入新管线（滤镜不是管线维度）"
        );
        assert_eq!(comp.pipeline_builds(), 1, "整条链路只建过 1 条合成管线");
    }
}
