// eframe_backend.rs
//
// EframeWgpuBackend：xcal_render::RenderBackend 的 egui-wgpu 具体实现。
// 由 xcal-render/src/render_backend.rs 迁移，逻辑不变。
//
// RenderContext（device/queue/format 句柄）见 render_context.rs。

use xcal_render::render_backend::{GpuHandle, RenderBackend};

use crate::render_context::RenderContext;

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
