// eframe_backend.rs
//
// EframeWgpuBackend：xcal_render::RenderBackend 的 egui-wgpu 具体实现。
// 由 xcal-render/src/render_backend.rs 迁移，逻辑不变。
//
// RenderContext（device/queue/format 句柄）见 render_context.rs。

use xcal_render::render_backend::{GpuHandle, RenderBackend};

use crate::compositor::Compositor;
use crate::render_context::RenderContext;
use crate::wgpu_2d::WgpuBackend2D;
use crate::wgpu_3d::WgpuBackend3D;

/// 为 egui-wgpu 宿主**安装标准渲染环境**，返回本次会话的 [`RenderContext`]。
///
/// 依次安装（顺序即依赖顺序）：
/// 1. [`WgpuBackend3D`]（默认着色器）；
/// 2. [`WgpuBackend2D`]（离屏 + MSAA；S4.1 后唯一的 2D 路径）；
/// 3. [`Compositor`]（按图层声明序合成；S3-2）。
///
/// 为什么放在本 crate 而不是 app：这是"**为宿主准备渲染环境**"的职责，而被安装的
/// 三个类型都是本 crate 的实现细节 —— 将来增删后端、改装配顺序只动这里，app 一行不变。
///
/// 为什么装进 `egui_wgpu` 的 `Renderer::callback_resources`（全局 TypeMap）：
/// 回调执行时只有 `&self`，不能持有"重建中"的 GPU 资源，故后端与合成器必须常驻 TypeMap
/// （见 `scene_callback` 的说明）。同一 TypeMap 里每种类型只放一份。
pub fn install_standard_backends(rs: &egui_wgpu::RenderState) -> RenderContext {
    let backend_3d = WgpuBackend3D::with_default_shader(&rs.device, &rs.queue, rs.target_format);
    rs.renderer.write().callback_resources.insert(backend_3d);

    let backend_2d = WgpuBackend2D::new(&rs.device, &rs.queue);
    rs.renderer.write().callback_resources.insert(backend_2d);

    let compositor = Compositor::new(&rs.device, rs.target_format);
    rs.renderer.write().callback_resources.insert(compositor);

    RenderContext::from_render_state(rs)
}

/// 基于 eframe/egui_wgpu 的 RenderBackend 实现。
///
/// 从 [`egui_wgpu::RenderState`] 抓取 device/queue；也支持 headless 构造
/// （测试 / 离屏渲染）。
pub struct EframeWgpuBackend {
    ctx: RenderContext,
    target: Option<wgpu::TextureView>,
    frame_active: bool,
}

impl EframeWgpuBackend {
    pub fn from_render_state(rs: &egui_wgpu::RenderState) -> Self {
        Self {
            ctx: RenderContext::from_render_state(rs),
            target: None,
            frame_active: false,
        }
    }

    pub fn headless() -> crate::render_context::Result<Self> {
        Ok(Self {
            ctx: RenderContext::headless()?,
            target: None,
            frame_active: false,
        })
    }

    pub fn from_context(ctx: RenderContext) -> Self {
        Self {
            ctx,
            target: None,
            frame_active: false,
        }
    }

    pub fn render_context(&self) -> &RenderContext {
        &self.ctx
    }

    pub fn is_frame_active(&self) -> bool {
        self.frame_active
    }
}

impl RenderBackend for EframeWgpuBackend {
    fn device(&self) -> &wgpu::Device {
        self.ctx.device()
    }

    fn queue(&self) -> &wgpu::Queue {
        self.ctx.queue()
    }

    fn target_format(&self) -> wgpu::TextureFormat {
        self.ctx.target_format()
    }

    fn create_buffer(&self, desc: &wgpu::BufferDescriptor<'_>) -> GpuHandle {
        GpuHandle::new(self.ctx.device().create_buffer(desc))
    }

    fn create_texture(&self, desc: &wgpu::TextureDescriptor<'_>) -> GpuHandle {
        GpuHandle::new(self.ctx.device().create_texture(desc))
    }

    fn create_pipeline(&self, desc: &wgpu::RenderPipelineDescriptor<'_>) -> GpuHandle {
        GpuHandle::new(self.ctx.device().create_render_pipeline(desc))
    }

    fn set_target(&mut self, view: Option<wgpu::TextureView>) {
        self.target = view;
    }

    fn target(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref()
    }

    fn begin_frame(&mut self) {
        self.frame_active = true;
    }

    fn end_frame(&mut self) {
        self.frame_active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn test_backend_types_are_send_sync() {
        assert_send_sync::<EframeWgpuBackend>();
    }

    #[test]
    fn test_headless_backend_lifecycle() {
        let Ok(mut backend) = EframeWgpuBackend::headless() else {
            eprintln!("no GPU adapter, skipping");
            return;
        };

        assert!(backend.target().is_none());
        assert!(!backend.is_frame_active());

        backend.begin_frame();
        assert!(backend.is_frame_active());

        let buf = backend.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test-buffer"),
            size: 16,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        assert!(buf.downcast_ref::<wgpu::Buffer>().is_some());

        let tex = backend.create_texture(&wgpu::TextureDescriptor {
            label: Some("test-texture"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: backend.target_format(),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let texture = tex.downcast_ref::<wgpu::Texture>().expect("texture");
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        backend.set_target(Some(view));
        assert!(backend.target().is_some());

        backend.end_frame();
        assert!(!backend.is_frame_active());

        backend.set_target(None);
        assert!(backend.target().is_none());
    }
}
