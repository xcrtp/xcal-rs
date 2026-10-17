// standard_backends.rs — W1：**不依赖 egui 的渲染环境装配 + headless 渲染入口**。
//
// # 为什么需要它
//
// 解耦的真实障碍**不是**"拿不到 device"（`RenderContext::headless()` 早就能建），
// 而是三个后端（`WgpuBackend3D` / `WgpuBackend2D` / `Compositor`）此前**只能**
// 装进 `egui_wgpu::Renderer::callback_resources`（全局 TypeMap）—— 因为 egui 回调
// 执行时只有 `&self`，后端必须常驻 TypeMap。
//
// 于是本模块给出**第二个家**：`StandardBackends` 自持有这三个后端，只为 device/queue/format
// （即 `&RenderContext`）。两条路径共用**同一份帧体**
// （[`crate::scene_callback::SceneCompositorCallback::prepare_frame`] +
// `paint_frame`）—— 通过 [`BackendStore`] 这一个抽象取后端
// ⇒ "headless 与 egui 逐位一致"是**结构保证**，不是巧合。
//
// # 分层（W1 把 `install_standard_backends` 拆成两步）
//
// ```text
// StandardBackends::new(&RenderContext)      ← 纯构造，零 egui 依赖
//    ├── 入 egui TypeMap：StandardBackends::install_into(resources)（feature = "egui-wgpu"）
//    └── headless 直接用：render_frame_headless(..)
// ```

use crate::compositor::Compositor;
use crate::render_context::RenderContext;
use crate::scene_callback::SceneCompositorCallback;
use crate::wgpu_2d::{Frame2DGeometry, WgpuBackend2D};
use crate::wgpu_3d::WgpuBackend3D;

/// 一帧的屏幕参数（`ScreenDescriptor` 的**去 egui 化**形态）。
///
/// 帧体只需要这两样：目标尺寸（物理像素）与 `pixels_per_point`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenParams {
    /// 渲染目标尺寸（物理像素）。
    pub size_in_pixels: [u32; 2],
    /// 每逻辑点的物理像素数。
    pub pixels_per_point: f32,
}

impl ScreenParams {
    pub fn new(size_in_pixels: [u32; 2], pixels_per_point: f32) -> Self {
        Self {
            size_in_pixels,
            pixels_per_point,
        }
    }
}

/// 一帧渲染所需的后端**取用口**。
///
/// 两个实现：`StandardBackends`（headless 自持有）与 `TypeMapStore`
/// （egui 的 `CallbackResources`）。帧体只经本 trait 取后端
/// ⇒ 两条宿主路径跑的是**同一份代码**。
pub trait BackendStore {
    /// 视口 2D 后端（离屏 + MSAA）。缺失 ⇒ 帧体跳过 2D 图层。
    fn backend_2d(&mut self) -> Option<&mut WgpuBackend2D>;

    /// **内容纹理层专用的** 2D 后端实例：与视口层隔离 uniform arena
    /// （同一帧两次 `finish_frame` 会互相覆盖 arena，见 `ContentBackend2D` 的说明）。
    /// 惰性创建：首次遇到内容层才建。
    fn content_backend_2d(&mut self, device: &wgpu::Device, queue: &wgpu::Queue)
    -> &mut WgpuBackend2D;

    /// 3D 后端。缺失 ⇒ 3D 图层与"内容纹理注册"都跳过。
    fn backend_3d(&mut self) -> Option<&mut WgpuBackend3D>;

    /// 合成器。缺失 ⇒ 帧体放弃本帧（与既有 `prepare` 的 `else { return }` 同口径）。
    fn compositor(&mut self) -> Option<&mut Compositor>;
}

/// **标准后端集合**（W1）：`WgpuBackend3D` + `WgpuBackend2D` (+ 内容层实例) + `Compositor`。
///
/// 构造只吃 [`RenderContext`] ⇒ headless 与 egui 两条路都能用同一份装配逻辑。
pub struct StandardBackends {
    backend_2d: WgpuBackend2D,
    /// 内容纹理层专用实例（惰性：与既有 egui 路径同为"首次遇到内容层才建"，
    /// 避免纯 2D / 纯 3D 场景白付一条管线的开销）。
    content_backend_2d: Option<WgpuBackend2D>,
    backend_3d: WgpuBackend3D,
    compositor: Compositor,
}

impl StandardBackends {
    /// 装配标准渲染环境。**只依赖 device / queue / target_format**（即 `&RenderContext`）
    /// —— 三个 `new` 的签名都只吃这些（已核查）。
    pub fn new(ctx: &RenderContext) -> Self {
        Self {
            backend_2d: WgpuBackend2D::new(ctx.device(), ctx.queue()),
            content_backend_2d: None,
            backend_3d: WgpuBackend3D::with_default_shader(
                ctx.device(),
                ctx.queue(),
                ctx.target_format(),
            ),
            compositor: Compositor::new(ctx.device(), ctx.target_format()),
        }
    }

    pub fn backend_2d_mut(&mut self) -> &mut WgpuBackend2D {
        &mut self.backend_2d
    }

    pub fn backend_3d_mut(&mut self) -> &mut WgpuBackend3D {
        &mut self.backend_3d
    }

    pub fn compositor_mut(&mut self) -> &mut Compositor {
        &mut self.compositor
    }

    /// 内容纹理层专用 2D 后端（惰性建）。命名带 `_mut` 以免与 [`BackendStore`]
    /// 的同名方法（返回 `&mut`，无 `Option`）在调用点上产生歧义。
    pub fn content_backend_2d_mut(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> &mut WgpuBackend2D {
        self.content_backend_2d
            .get_or_insert_with(|| WgpuBackend2D::new(device, queue))
    }
}

impl BackendStore for StandardBackends {
    fn backend_2d(&mut self) -> Option<&mut WgpuBackend2D> {
        Some(&mut self.backend_2d)
    }

    fn content_backend_2d(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> &mut WgpuBackend2D {
        self.content_backend_2d_mut(device, queue)
    }

    fn backend_3d(&mut self) -> Option<&mut WgpuBackend3D> {
        Some(&mut self.backend_3d)
    }

    fn compositor(&mut self) -> Option<&mut Compositor> {
        Some(&mut self.compositor)
    }
}

#[cfg(feature = "egui-wgpu")]
impl StandardBackends {
    /// **第 2 步**：把后端装进 egui 的 `CallbackResources`（TypeMap）。
    ///
    /// 插入顺序与拆分前逐位一致（3D → 2D → 合成器），故 egui 路径行为不变。
    pub fn install_into(self, resources: &mut egui_wgpu::CallbackResources) {
        resources.insert(self.backend_3d);
        resources.insert(self.backend_2d);
        resources.insert(self.compositor);
        if let Some(content) = self.content_backend_2d {
            resources.insert(ContentBackend2D(content));
        }
    }
}

/// 内容纹理层**专用**的 2D 后端资源（egui 路径：常驻 TypeMap 的那个实例）。
///
/// 与 [`WgpuBackend2D`] 是同一类型的**独立实例**。原因：`prepare` 同一帧会对 2D
/// 后端跑两次 `finish_frame`（趟 1 内容层、趟 2 视口层），而 `WgpuBackend2D` 的
/// `UniformArena` 在 `finish_frame` 里 `begin_frame()` 重置、`flush()` 从 offset 0
/// 重写——共用实例时后一次的 uniform 会覆盖前一次；又因两次 pass 都录进同一个
/// `egui_encoder`、稍后才 `submit`，内容层的 pass 在 GPU 上实际读到的是视口层的
/// uniform（clip 矩阵 / 颜色错乱），表现为"关掉网格（视口层无 draw → 不写 uniform）
/// 才显示正确内容"。独立实例从结构上隔离两类的 uniform arena。
#[cfg(feature = "egui-wgpu")]
pub(crate) struct ContentBackend2D(pub WgpuBackend2D);

/// [`BackendStore`] 的 egui 实现：后端住在全局 TypeMap 里。
#[cfg(feature = "egui-wgpu")]
pub(crate) struct TypeMapStore<'a>(pub &'a mut egui_wgpu::CallbackResources);

#[cfg(feature = "egui-wgpu")]
impl BackendStore for TypeMapStore<'_> {
    fn backend_2d(&mut self) -> Option<&mut WgpuBackend2D> {
        self.0.get_mut::<WgpuBackend2D>()
    }

    fn content_backend_2d(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> &mut WgpuBackend2D {
        if self.0.get::<ContentBackend2D>().is_none() {
            self.0
                .insert(ContentBackend2D(WgpuBackend2D::new(device, queue)));
        }
        &mut self
            .0
            .get_mut::<ContentBackend2D>()
            .expect("刚插入的内容层 2D 后端")
            .0
    }

    fn backend_3d(&mut self) -> Option<&mut WgpuBackend3D> {
        self.0.get_mut::<WgpuBackend3D>()
    }

    fn compositor(&mut self) -> Option<&mut Compositor> {
        self.0.get_mut::<Compositor>()
    }
}

/// **headless（非 egui）渲染入口**（W1 的产出）。
///
/// 与 egui 宿主跑**同一份帧体**（[`SceneCompositorCallback::prepare_frame`] +
/// [`SceneCompositorCallback::paint_frame`]），差异只在"谁给目标纹理与
/// viewport / scissor"：
/// - egui-wgpu 会为每个回调把 render pass 的 viewport 设成回调矩形；
/// - 本函数按回调的 `viewport_points` **自建同等尺寸的目标纹理**，并显式把
///   viewport / scissor 设成 `(0,0,w,h)` —— 于是 NDC 全屏四边形正好铺满目标，
///   与 egui 路径的几何关系完全一致。
///
/// 返回**紧凑像素**（行距 `w*4`，左上角为原点）；字节序为目标纹理格式
/// （默认 `Bgra8UnormSrgb` ⇒ **BGRA**）。喂给 PNG 编码器前需按
/// [`RenderContext::target_format`] 转换通道（示例：`xcal_app::export::to_rgba`）。
pub fn render_frame_headless(
    ctx: &RenderContext,
    backends: &mut StandardBackends,
    frame: &SceneCompositorCallback,
    screen: ScreenParams,
    clear: wgpu::Color,
) -> Vec<u8> {
    let geometry = Frame2DGeometry {
        viewport_points: frame.viewport_points(),
        pixels_per_point: screen.pixels_per_point,
        screen_size_px: screen.size_in_pixels,
    };
    let (_, _, width, height) = geometry.viewport_px();
    let (width, height) = (width.max(1), height.max(1));

    let target = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("xcal-headless-target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: ctx.target_format(),
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = ctx
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("xcal-headless-encoder"),
        });
    let composed = frame.prepare_frame(
        ctx.device(),
        ctx.queue(),
        &mut encoder,
        screen,
        backends,
    );

    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("xcal-headless-main-pass"),
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
        // 与 egui-wgpu 的 "courtesy" 行为同口径：viewport == scissor == 回调矩形。
        pass.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);
        pass.set_scissor_rect(0, 0, width, height);
        if let Some(compositor) = backends.compositor() {
            frame.paint_frame(compositor, &composed, &mut pass);
        }
    }

    ctx.submit(encoder.finish());
    ctx.read_texture(&target, (width, height))
}

/// headless 入口的**便捷形态**：`StandardBackends` 自建（跨帧缓存随之丢弃）。
///
/// ⚠️ 序列帧导出**不要**用它 —— 每帧重建后端会丢掉管线与几何缓存；
/// 那条路请自持一个 [`StandardBackends`] 反复喂给 [`render_frame_headless`]。
pub fn render_single_frame(
    ctx: &RenderContext,
    frame: &SceneCompositorCallback,
    screen: ScreenParams,
    clear: wgpu::Color,
) -> Vec<u8> {
    let mut backends = StandardBackends::new(ctx);
    render_frame_headless(ctx, &mut backends, frame, screen, clear)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ScreenParams` 只是两个字段的载体（去 egui 化的 `ScreenDescriptor`）。
    #[test]
    fn screen_params_carries_size_and_ppp() {
        let s = ScreenParams::new([64, 32], 2.0);
        assert_eq!(s.size_in_pixels, [64, 32]);
        assert_eq!(s.pixels_per_point, 2.0);
    }
}
