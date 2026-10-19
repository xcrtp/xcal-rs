// eframe_backend.rs
//
// EframeWgpuBackend：zela_render::RenderBackend 的 egui-wgpu 具体实现。
// 由 zela-render/src/render_backend.rs 迁移，逻辑不变。
//
// RenderContext（device/queue/format 句柄）见 render_context.rs。

use zela_render::render_backend::{GpuHandle, RenderBackend};

use crate::render_context::RenderContext;
use crate::standard_backends::StandardBackends;

/// 为 egui-wgpu 宿主**安装标准渲染环境**，返回本次会话的 [`RenderContext`]。
///
/// **W1：本函数已拆成两步**（组装逻辑归 [`StandardBackends`]，不依赖 egui）：
/// 1. [`StandardBackends::new`]（`&RenderContext`）—— 纯构造；
/// 2. [`StandardBackends::install_into`] —— 装进 `egui_wgpu` 的 `CallbackResources`
///    （全局 TypeMap）。
///
/// **对外签名与行为不变**（插入顺序仍是 3D → 2D → 合成器）；变的是"谁拥有装配知识"：
/// 现在 app / headless / 其他宿主共用同一份装配。
///
/// 为什么装进 TypeMap：回调执行时只有 `&self`，不能持有"重建中"的 GPU 资源，故后端与
/// 合成器必须常驻 TypeMap（见 `scene_callback` 的说明）。同一 TypeMap 里每种类型只放一份。
pub fn install_standard_backends(rs: &egui_wgpu::RenderState) -> RenderContext {
    let ctx = RenderContext::from_render_state(rs);
    StandardBackends::new(&ctx).install_into(&mut rs.renderer.write().callback_resources);
    ctx
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

    /// headless 构造（测试 / 离屏渲染）。仅 native `blocking` 构建可用。
    #[cfg(feature = "blocking")]
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
        let Some((_gpu, ctx)) = crate::gpu_test::require_gpu() else {
            return;
        };
        let mut backend = EframeWgpuBackend::from_context(ctx);

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
