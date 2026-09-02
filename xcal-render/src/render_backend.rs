// render_backend.rs

use std::any::{Any, TypeId};

use crate::render_context::RenderContext;

// ─── GpuHandle ────────────────────────────────────────────────────────────

pub struct GpuHandle(Box<dyn Any + Send + Sync>);

impl GpuHandle {
    pub fn new<T: Any + Send + Sync>(value: T) -> Self {
        Self(Box::new(value))
    }

    pub fn downcast_ref<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.0.downcast_ref::<T>()
    }

    pub fn downcast_mut<T: Any + Send + Sync>(&mut self) -> Option<&mut T> {
        self.0.downcast_mut::<T>()
    }

    pub fn into_inner<T: Any + Send + Sync>(self) -> Option<T> {
        self.0.downcast::<T>().ok().map(|b| *b)
    }

    pub fn type_id(&self) -> TypeId {
        (*self.0).type_id()
    }

    pub fn is<T: Any>(&self) -> bool {
        self.0.is::<T>()
    }
}

// ─── RenderBackend trait ──────────────────────────────────────────────────

pub trait RenderBackend: Send + Sync {
    fn device(&self) -> &wgpu::Device;

    fn queue(&self) -> &wgpu::Queue;

    fn target_format(&self) -> wgpu::TextureFormat;

    fn create_buffer(&self, desc: &wgpu::BufferDescriptor<'_>) -> GpuHandle;

    fn create_texture(&self, desc: &wgpu::TextureDescriptor<'_>) -> GpuHandle;

    fn create_pipeline(&self, desc: &wgpu::RenderPipelineDescriptor<'_>) -> GpuHandle;

    fn set_target(&mut self, view: Option<wgpu::TextureView>);

    fn target(&self) -> Option<&wgpu::TextureView>;

    fn begin_frame(&mut self) {
        let _ = self;
    }

    fn end_frame(&mut self) {
        let _ = self;
    }
}

// ─── EframeWgpuBackend ────────────────────────────────────────────────────

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
        assert_send_sync::<GpuHandle>();
        assert_send_sync::<EframeWgpuBackend>();
    }

    #[test]
    fn test_gpu_handle_roundtrip() {
        let handle = GpuHandle::new(42_u32);
        assert!(handle.is::<u32>());
        assert_eq!(handle.downcast_ref::<u32>(), Some(&42));
        assert!(handle.downcast_ref::<i64>().is_none());

        let mut handle = handle;
        if let Some(v) = handle.downcast_mut::<u32>() {
            *v = 100;
        }
        assert_eq!(handle.downcast_ref::<u32>(), Some(&100));
        assert_eq!(handle.into_inner::<u32>(), Some(100));
    }

    #[test]
    fn test_gpu_handle_into_inner_wrong_type() {
        let handle = GpuHandle::new("hello".to_string());
        assert_eq!(handle.into_inner::<u32>(), None);
    }

    #[test]
    fn test_headless_backend_lifecycle() {
        let Ok(mut backend) = EframeWgpuBackend::headless() else { return };

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
            size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: backend.target_format(),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
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

    #[test]
    fn test_headless_backend_create_pipeline() {
        let Ok(backend) = EframeWgpuBackend::headless() else { return };

        let source = r#"
            @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
                var p = array(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
                return vec4f(p[i], 0.0, 1.0);
            }
            @fragment fn fs() -> @location(0) vec4f {
                return vec4f(1.0, 0.0, 0.0, 1.0);
            }
        "#;
        let shader = backend.device().create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("test-shader"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });

        let pipeline = backend.create_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("test-pipeline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: backend.target_format(),
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        assert!(pipeline.downcast_ref::<wgpu::RenderPipeline>().is_some());
    }
}
