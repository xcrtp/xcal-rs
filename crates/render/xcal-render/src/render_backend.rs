// render_backend.rs
//
// 后端 trait 定义：
// - GpuHandle：类型擦除的 GPU 资源句柄（无 wgpu 依赖，纯 Any），带 GpuResourceId/generation；
// - RenderBackend：资源型后端 trait（device/queue/资源创建，wgpu 抽象）；
// - RenderBackend3D：3D 层提交后端（把 RenderLayer 录制进 command encoder）。
//
// 具体实现（EframeWgpuBackend / WgpuBackend3D）位于 xcal-render-backends。

use std::any::{Any, TypeId};
use std::fmt;

#[cfg(feature = "wgpu")]
use crate::render_layer::RenderLayer;

// ─── GpuResourceId ────────────────────────────────────────────────────────

/// 世代感知的 GPU 资源标识符（index+generation）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GpuResourceId {
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

impl GpuResourceId {
    /// 空/未分配的哨兵值。
    pub const SENTINEL: Self = Self {
        index: u32::MAX,
        generation: 0,
    };

    #[inline]
    pub fn is_sentinel(self) -> bool {
        self.index == u32::MAX
    }

    #[inline]
    pub fn index(self) -> u32 {
        self.index
    }

    #[inline]
    pub fn generation(self) -> u32 {
        self.generation
    }
}

// ─── AccessError ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AccessError {
    Stale { id: GpuResourceId },
    TypeMismatch,
    Dropped,
}

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AccessError::Stale { id } => write!(
                f,
                "stale GPU resource access: id=(index={}, gen={})",
                id.index(),
                id.generation()
            ),
            AccessError::TypeMismatch => write!(f, "GPU resource type mismatch"),
            AccessError::Dropped => write!(f, "GPU resource has been dropped"),
        }
    }
}

impl std::error::Error for AccessError {}

// ─── GpuHandle ────────────────────────────────────────────────────────────

/// 类型擦除的 GPU 资源句柄（buffer / texture / pipeline 等）。
///
/// 附带一个 [`GpuResourceId`]：创建时默认为哨兵值，
/// 由 `ResourceCache` 在插入条目时通过 `assign_id` 赋予实际 id。
pub struct GpuHandle {
    id: GpuResourceId,
    inner: Box<dyn Any + Send + Sync>,
}

impl std::fmt::Debug for GpuHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuHandle")
            .field("id", &self.id)
            .field("inner_type_id", &(*self.inner).type_id())
            .finish()
    }
}

impl PartialEq for GpuHandle {
    fn eq(&self, other: &Self) -> bool {
        // 基于 id 与内部类型 id 的相等，不比较内部包装对象值（类型擦除后不可比）
        self.id == other.id && (*self.inner).type_id() == (*other.inner).type_id()
    }
}

impl Eq for GpuHandle {}

impl GpuHandle {
    pub fn new<T: Any + Send + Sync>(value: T) -> Self {
        Self {
            id: GpuResourceId::SENTINEL,
            inner: Box::new(value),
        }
    }

    pub fn id(&self) -> GpuResourceId {
        self.id
    }

    /// 由 resource_cache 在插入时赋予有效的资源 id（crate 内可见）。
    pub(crate) fn assign_id(&mut self, id: GpuResourceId) {
        self.id = id;
    }

    /// 剥离所有权：延迟销毁器/缓存需要移动内部资源而不立刻 drop 它时使用。
    pub fn take(self) -> Box<dyn Any + Send + Sync> {
        self.inner
    }

    pub fn downcast_ref<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.inner.downcast_ref::<T>()
    }

    pub fn downcast_mut<T: Any + Send + Sync>(&mut self) -> Option<&mut T> {
        self.inner.downcast_mut::<T>()
    }

    pub fn into_inner<T: Any + Send + Sync>(self) -> Option<T> {
        self.inner.downcast::<T>().ok().map(|b| *b)
    }

    pub fn type_id(&self) -> TypeId {
        (*self.inner).type_id()
    }

    pub fn is<T: Any>(&self) -> bool {
        self.inner.is::<T>()
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

    #[test]
    fn test_resource_id_sentinel() {
        let id = GpuResourceId::SENTINEL;
        assert!(id.is_sentinel());
        assert_ne!(id, GpuResourceId {
            index: 0,
            generation: 1
        });
    }
}
