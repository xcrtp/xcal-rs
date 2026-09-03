// render_backend.rs
//
// 后端 trait 定义：
// - GpuHandle：类型擦除的 GPU 资源句柄（无 wgpu 依赖，纯 Any）；
// - RenderBackend：资源型后端 trait（device/queue/资源创建，wgpu 抽象）；
// - RenderBackend3D：3D 层提交后端（把 RenderLayer 录制进 command encoder）。
//
// 具体实现（EframeWgpuBackend / WgpuBackend3D）位于 xcal-render-backends。

use std::any::{Any, TypeId};

#[cfg(feature = "wgpu")]
use crate::render_layer::RenderLayer;

// ─── GpuHandle ────────────────────────────────────────────────────────────

/// 类型擦除的 GPU 资源句柄（buffer / texture / pipeline 等）。
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

// ─── RenderBackend trait（资源型，wgpu） ──────────────────────────────────

/// 资源型渲染后端：提供 device/queue 句柄与 GPU 资源创建入口。
///
/// 2D/3D 资源缓存（[`crate::ResourceCache`]）通过此 trait 上传资源，
/// 与具体后端（eframe / headless）解耦。
#[cfg(feature = "wgpu")]
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

// ─── RenderBackend3D trait ─────────────────────────────────────────────────

/// 3D 层提交后端：把一个 [`RenderLayer`] 录制进 command encoder。
///
/// 与 2D 的 [`RenderBackend`] 独立：3D 路径以「层」为提交单元
/// （相机 + 视口 + 绘制队列 + 渲染目标），自包含地 begin/end render pass。
#[cfg(feature = "wgpu")]
pub trait RenderBackend3D: Send + Sync {
    /// 将一个 RenderLayer 提交到 GPU：
    /// begin render pass（目标 = layer.target，clear 黑色）→
    /// 绘制 `layer.draw_queue` 全部内容 → end pass。
    ///
    /// 调用方负责提交 encoder（`queue.submit`）。
    fn submit_layer(&self, layer: &RenderLayer, encoder: &mut wgpu::CommandEncoder);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn test_handle_types_are_send_sync() {
        assert_send_sync::<GpuHandle>();
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
}
