// render_backend.rs
//
// GPU 资源标识与类型擦除句柄（纯 Any，无 wgpu 依赖）：
// - GpuResourceId：世代感知的 index+generation 标识符；
// - AccessError：generation 校验失败原因；
// - GpuHandle：类型擦除的 GPU 资源句柄。
//
// 需要 wgpu 类型的后端 trait（RenderBackend / RenderBackend3D）位于 zela-render。

use std::any::{Any, TypeId};
use std::fmt;

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
///
/// **不施加 `Send` / `Sync` 约束**：wasm 上 wgpu 资源内部含 `Rc<RefCell<…>>`，
/// 浏览器资源只能在创建它的线程使用；本项目的渲染架构是单线程、主线程唯一，
/// GPU 资源本就不需要跨线程。`Any` 仍然保留（`downcast_ref` 需要它）。
pub struct GpuHandle {
    id: GpuResourceId,
    inner: Box<dyn Any>,
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
    pub fn new<T: Any>(value: T) -> Self {
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
    pub fn take(self) -> Box<dyn Any> {
        self.inner
    }

    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.inner.downcast_ref::<T>()
    }

    pub fn downcast_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.inner.downcast_mut::<T>()
    }

    pub fn into_inner<T: Any>(self) -> Option<T> {
        self.inner.downcast::<T>().ok().map(|b| *b)
    }

    pub fn type_id(&self) -> TypeId {
        (*self.inner).type_id()
    }

    pub fn is<T: Any>(&self) -> bool {
        self.inner.is::<T>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
