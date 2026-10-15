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

use xcal_core::domain::LayerBlend;
use xcal_render::{AaModel, CacheStrategy, Vertex2D, VertexLayout};

use crate::vertex_layout::to_wgpu;

/// 合成着色器：源纹理 → 主 pass（含 sRGB / 非 sRGB 双入口 + opacity uniform）。
pub const COMPOSITE_SHADER: &str = include_str!("../shaders/composite_2d.wgsl");

/// 每层 opacity uniform 的字节数（`vec4<f32>`）。
pub const OPACITY_UNIFORM_SIZE: u64 = 16;

/// 合成四边形（裁剪坐标 + UV；TriangleStrip：左上 → 右上 → 左下 → 右下）。
pub const COMPOSITE_QUAD: [Vertex2D; 4] = [
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

/// 图层合成器：按 `(LayerBlend, Conversion)` 预建的管线组 + 采样器 + 全屏四边形。
pub struct Compositor {
    /// 主 pass 目标格式（域转换的参照）。
    target_format: wgpu::TextureFormat,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    quad: wgpu::Buffer,
    /// 3 种 blend × 3 种域转换 = 9 条管线（wgpu 无动态 blend / 无动态入口点）。
    pipelines: HashMap<(LayerBlend, Conversion), wgpu::RenderPipeline>,
}

impl Compositor {
    /// 构造合成器（`target_format` = 主 pass 目标格式，决定 sRGB / 非 sRGB 入口）。
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

        let make_pipeline = |blend: LayerBlend, conversion: Conversion| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("xcal-compositor-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[Some(wgpu_vertex_layout.clone())],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(conversion.entry_point()),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: Some(blend_state(blend)),
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
        };

        // 3 blend × 3 域转换全量预建：命中组合只有 2~4 种，但预建避免运行期
        // 延迟编译（pipeline 创建会阻塞首帧，且需要 `&mut`/内部可变性）。
        let mut pipelines = HashMap::new();
        for blend in [LayerBlend::Opaque, LayerBlend::Alpha, LayerBlend::Additive] {
            for conversion in [
                Conversion::Identity,
                Conversion::GammaToLinear,
                Conversion::LinearToGamma,
            ] {
                pipelines.insert((blend, conversion), make_pipeline(blend, conversion));
            }
        }

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

        Self {
            target_format,
            bind_group_layout,
            sampler,
            quad,
            pipelines,
        }
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
                    key: (op.blend, Conversion::for_formats(op.src_format, self.target_format)),
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

/// 把 Copy 切片视为字节切片（顶点/uniform 上传用）。
fn bytes_of<T: Copy>(data: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, std::mem::size_of_val(data)) }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_ne!(blend_state(LayerBlend::Alpha), blend_state(LayerBlend::Additive));
    }

    /// 四边形：4 顶点、全 NDC 覆盖、UV (0,0)-(1,1)（契约布局 stride 20）。
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
        }
    }

    /// 顶点字节布局与契约一致（position 8B + UV 8B + attrib1 4B = 20B/顶点）。
    #[test]
    fn quad_bytes_are_twenty_per_vertex() {
        assert_eq!(bytes_of(&COMPOSITE_QUAD).len(), 4 * 20);
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
        assert_eq!(Conversion::for_formats(Rgba8Unorm, Bgra8Unorm), Conversion::Identity);
        assert_eq!(Conversion::for_formats(Rgba8Unorm, Rgba8Unorm), Conversion::Identity);
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
        assert_eq!(Conversion::GammaToLinear.entry_point(), "fs_gamma_to_linear");
        assert_eq!(Conversion::LinearToGamma.entry_point(), "fs_linear_to_gamma");
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
    fn composite_pixel(ctx: &crate::RenderContext, comp: &Compositor, ops: &[CompositeOp]) -> [u8; 4] {
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

        // 回读：bytes_per_row 必须 256 对齐。
        let padded = 256u32;
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xcal-compositor-readback"),
            size: (padded * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(4),
                },
            },
            wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue.submit([enc.finish()]);

        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map_async 失败"));
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll 失败");
        let data = slice.get_mapped_range().expect("合成结果映射失败");
        let px = [data[0], data[1], data[2], data[3]];
        drop(data);
        buffer.unmap();
        px
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
        let comp = Compositor::new(&ctx.device, TEST_FORMAT);
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
        let px = composite_pixel(&ctx, &comp, &ops);
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
        let px_rev = composite_pixel(&ctx, &comp, &reversed);
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
        let comp = Compositor::new(&ctx.device, TEST_FORMAT);

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
        let px = composite_pixel(&ctx, &comp, &ops);
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
        let comp = Compositor::new(&ctx.device, TEST_FORMAT);

        // 不透明红打底 + 蓝色上层。
        let px_alpha_0 = composite_pixel(
            &ctx,
            &comp,
            &[
                op(solid_texture(&ctx, [255, 0, 0, 255]), LayerBlend::Alpha, 1.0),
                op(solid_texture(&ctx, [0, 0, 255, 255]), LayerBlend::Alpha, 0.0),
            ],
        );
        assert_eq!(
            px_alpha_0,
            [255, 0, 0, 255],
            "opacity=0 时该层应完全不可见（下层原样保留）"
        );

        let px_alpha_1 = composite_pixel(
            &ctx,
            &comp,
            &[
                op(solid_texture(&ctx, [255, 0, 0, 255]), LayerBlend::Alpha, 1.0),
                op(solid_texture(&ctx, [0, 0, 255, 255]), LayerBlend::Alpha, 1.0),
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
            &comp,
            &[
                op(solid_texture(&ctx, [255, 0, 0, 255]), LayerBlend::Alpha, 1.0),
                op(solid_texture(&ctx, [0, 0, 255, 255]), LayerBlend::Alpha, 2.0),
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
            op(solid_texture(&ctx, [255, 0, 0, 255]), LayerBlend::Opaque, 1.0),
            op(solid_texture(&ctx, [0, 255, 0, 255]), LayerBlend::Alpha, 1.0),
            op(solid_texture(&ctx, [0, 0, 255, 255]), LayerBlend::Additive, 1.0),
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
}
