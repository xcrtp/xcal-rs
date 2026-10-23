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

/// 跨平台墙钟（逐帧计时的起点）。
///
/// **为什么不用 `std::time::Instant::now()`**：在 `wasm32-unknown-unknown` 上它直接
/// `panic!("time not implemented on this platform")` —— B8 引入逐帧计时后，网页第一次
/// 出帧就崩（页面黑屏、作品按钮停在禁用态）。`web-time` 在 wasm 用 `performance.now()`，
/// native 直接转发 `std::time::Instant` ⇒ 两条路径同一份代码。
fn clock() -> web_time::Instant {
    web_time::Instant::now()
}

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

/// 一帧的**CPU 侧**分段耗时（B8 Step 1）。
///
/// 只在录制侧能分开的三段：`prepare`（构帧体 + 三角化 + 写 uniform）、
/// `paint`（主 pass 录制）、`submit`（`queue.submit`）。GPU 实际执行发生在
/// 提交之后、由回读的 `poll(wait)` 承担，因此 `submit` 不含 GPU 时间 —— 那是
/// `t_read` 的一部分（见 `HeadlessTiming`）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderTiming {
    pub prepare: std::time::Duration,
    pub paint: std::time::Duration,
    pub submit: std::time::Duration,
}

/// 一帧的分段耗时：录制三段 + 回读（含 GPU 等待）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HeadlessTiming {
    pub render: RenderTiming,
    pub read: std::time::Duration,
}

/// **把一帧渲染进给定 view**（S1）：不建纹理、不回读。
///
/// 这是显示路径与离屏路径的**唯一共享帧体入口**：
/// - 浏览器显示：`view` = canvas surface view；
/// - headless / 对拍：`view` = [`render_frame_headless`] 自建的离屏纹理 view。
///
/// viewport / scissor 仍设为回调矩形 `(0,0,w,h)`（与 egui 的 courtesy 行为同口径）。
pub fn render_frame_to_view(
    ctx: &RenderContext,
    backends: &mut StandardBackends,
    frame: &SceneCompositorCallback,
    screen: ScreenParams,
    clear: wgpu::Color,
    view: &wgpu::TextureView,
) {
    let _ = render_frame_to_view_timed(ctx, backends, frame, screen, clear, view);
}

/// 同 [`render_frame_to_view`]，但回报三段 CPU 计时（B8 Step 1）。
pub fn render_frame_to_view_timed(
    ctx: &RenderContext,
    backends: &mut StandardBackends,
    frame: &SceneCompositorCallback,
    screen: ScreenParams,
    clear: wgpu::Color,
    view: &wgpu::TextureView,
) -> RenderTiming {
    let mut timing = RenderTiming::default();
    let geometry = Frame2DGeometry {
        viewport_points: frame.viewport_points(),
        pixels_per_point: screen.pixels_per_point,
        screen_size_px: screen.size_in_pixels,
    };
    let (_, _, width, height) = geometry.viewport_px();
    let (width, height) = (width.max(1), height.max(1));

    let mut encoder = ctx
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("zela-frame-encoder"),
        });
    let prepare = clock();
    let composed = frame.prepare_frame(
        ctx.device(),
        ctx.queue(),
        &mut encoder,
        screen,
        backends,
    );
    timing.prepare = prepare.elapsed();

    let paint = clock();
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("zela-frame-main-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
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
    timing.paint = paint.elapsed();

    let submit = clock();
    ctx.submit(encoder.finish());
    timing.submit = submit.elapsed();
    timing
}

/// **headless 渲染目标缓存**（B8 Step 0）：序列帧导出时把 `create_texture` 提到循环外。
///
/// 为什么需要：`render_frame_headless` 原先每帧 `create_texture` + `create_view`
/// （第 296 行那个反模式 —— 与第 356 行注释警告的"每帧重建后端丢缓存"是同一类问题）。
/// 逐帧分配一张 640×480 纹理本身不贵，但它让 GPU 无法稳定复用同一块显存，
/// 且与"回读 buffer 每帧新建"叠在一起成为固定开销。
///
/// 复用边界（与计划 §2.2 对应）：
/// - **尺寸 / 格式变化** ⇒ 重建（不得复用错误规格）；
/// - **设备丢失** ⇒ 调用 [`Self::invalidate`]（本类型持有的是旧 device 的句柄；
///   设备丢失后整个 `RenderContext` 都要重建，缓存必须跟着丢）；
/// - **上一帧仍被持有** ⇒ [`crate::render_frame_headless_cached`] 在**回读完成后**
///   才返回，因此下一次复用时上一帧的拷贝已结束（同步路径安全）。
///   异步（双缓冲）路径**不得**直接复用本缓存，见 B8 Step 4。
#[cfg(feature = "blocking")]
#[derive(Default)]
pub struct HeadlessTarget {
    inner: Option<HeadlessTargetInner>,
}

#[cfg(feature = "blocking")]
struct HeadlessTargetInner {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
    format: wgpu::TextureFormat,
}

#[cfg(feature = "blocking")]
impl HeadlessTarget {
    pub fn new() -> Self {
        Self::default()
    }

    /// 丢弃当前纹理（设备丢失 / 需要强制重分配时调用）。
    pub fn invalidate(&mut self) {
        self.inner = None;
    }

    /// 按 `size` / 当前目标格式取可用的目标（不匹配则重建）；返回纹理与视图。
    fn ensure(
        &mut self,
        ctx: &RenderContext,
        size: [u32; 2],
    ) -> (&wgpu::Texture, &wgpu::TextureView) {
        let format = ctx.target_format();
        let reusable = self
            .inner
            .as_ref()
            .is_some_and(|inner| inner.size == size && inner.format == format);
        if !reusable {
            let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
                label: Some("zela-headless-target"),
                size: wgpu::Extent3d {
                    width: size[0].max(1),
                    height: size[1].max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.inner = Some(HeadlessTargetInner {
                texture,
                view,
                size,
                format,
            });
        }
        let inner = self.inner.as_ref().expect("刚确保过目标纹理");
        (&inner.texture, &inner.view)
    }
}

/// **headless（非 egui）渲染入口**（W1 的产出）。
///
/// 按回调的 `viewport_points` **自建同等尺寸的目标纹理** →
/// [`render_frame_to_view`] → 回读紧凑像素。
///
/// 返回**紧凑像素**（行距 `w*4`，左上角为原点）；字节序为目标纹理格式
/// （默认 `Bgra8UnormSrgb` ⇒ **BGRA**）。喂给 PNG 编码器前需按
/// [`RenderContext::target_format`] 转换通道（示例：`zela_playback::to_rgba`）。
///
/// ⚠️ 单帧便捷入口：序列帧导出请用 [`render_frame_headless_cached`]，
/// 否则每帧重建目标纹理（B8 Step 0）。
#[cfg(feature = "blocking")]
pub fn render_frame_headless(
    ctx: &RenderContext,
    backends: &mut StandardBackends,
    frame: &SceneCompositorCallback,
    screen: ScreenParams,
    clear: wgpu::Color,
) -> Vec<u8> {
    let mut target = HeadlessTarget::new();
    render_frame_headless_cached(ctx, backends, frame, screen, clear, &mut target)
}

/// 同 [`render_frame_headless`]，但**复用**调用方持有的目标纹理（B8 Step 0）。
///
/// 返回时回读已完成 ⇒ 下一帧复用同一纹理是安全的（同步路径）。
#[cfg(feature = "blocking")]
pub fn render_frame_headless_cached(
    ctx: &RenderContext,
    backends: &mut StandardBackends,
    frame: &SceneCompositorCallback,
    screen: ScreenParams,
    clear: wgpu::Color,
    target: &mut HeadlessTarget,
) -> Vec<u8> {
    render_frame_headless_cached_timed(ctx, backends, frame, screen, clear, target).0
}

/// 同 [`render_frame_headless_cached`]，并回报分段耗时（B8 Step 1）。
///
/// `read` 段包含 **GPU 执行等待**（`poll(wait)`）—— 录制侧无法单独拿到 GPU 时间。
#[cfg(feature = "blocking")]
pub fn render_frame_headless_cached_timed(
    ctx: &RenderContext,
    backends: &mut StandardBackends,
    frame: &SceneCompositorCallback,
    screen: ScreenParams,
    clear: wgpu::Color,
    target: &mut HeadlessTarget,
) -> (Vec<u8>, HeadlessTiming) {
    let geometry = Frame2DGeometry {
        viewport_points: frame.viewport_points(),
        pixels_per_point: screen.pixels_per_point,
        screen_size_px: screen.size_in_pixels,
    };
    let (_, _, width, height) = geometry.viewport_px();
    let (width, height) = (width.max(1), height.max(1));

    let (texture, view) = target.ensure(ctx, [width, height]);
    let render = render_frame_to_view_timed(ctx, backends, frame, screen, clear, view);
    let read = clock();
    let pixels = ctx.read_texture(texture, (width, height));
    (
        pixels,
        HeadlessTiming {
            render,
            read: read.elapsed(),
        },
    )
}

/// **渲染进缓存目标但不回读**（B8 Step 1/2）：供 sink 化路径使用。
///
/// 返回本帧目标的尺寸；像素要等 [`RenderContext::read_texture`] 或 sink 自己回读。
#[cfg(feature = "blocking")]
pub fn render_frame_headless_to_target(
    ctx: &RenderContext,
    backends: &mut StandardBackends,
    frame: &SceneCompositorCallback,
    screen: ScreenParams,
    clear: wgpu::Color,
    target: &mut HeadlessTarget,
) -> (wgpu::Texture, wgpu::TextureView, [u32; 2]) {
    let geometry = Frame2DGeometry {
        viewport_points: frame.viewport_points(),
        pixels_per_point: screen.pixels_per_point,
        screen_size_px: screen.size_in_pixels,
    };
    let (_, _, width, height) = geometry.viewport_px();
    let (width, height) = (width.max(1), height.max(1));
    let (texture, view) = target.ensure(ctx, [width, height]);
    render_frame_to_view(ctx, backends, frame, screen, clear, view);
    (texture.clone(), view.clone(), [width, height])
}

/// **异步 headless 回读**（S3；浏览器对拍用）。
///
/// 与同步版逐字节一致，只是等待方式换成 `map_async` → oneshot → `await`。
pub async fn render_frame_headless_async(
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
        label: Some("zela-headless-target"),
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

    render_frame_to_view(ctx, backends, frame, screen, clear, &view);
    ctx.read_texture_async(&target, (width, height)).await
}

/// headless 入口的**便捷形态**：`StandardBackends` 自建（跨帧缓存随之丢弃）。
///
/// ⚠️ 序列帧导出**不要**用它 —— 每帧重建后端会丢掉管线与几何缓存；
/// 那条路请自持一个 [`StandardBackends`] 反复喂给 [`render_frame_headless`]。
#[cfg(feature = "blocking")]
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

