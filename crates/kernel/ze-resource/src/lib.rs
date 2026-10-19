// ze-resource: GPU 资源契约与缓存（G2–G4）——只声明资源标识与缓存策略，不依赖任何后端。
//
//   - render_backend.rs：GpuResourceId / AccessError / GpuHandle（类型擦除，纯 Any）
//   - resource_cache.rs：ResourceKey / DeferredDestroyer / ResourceCache（generation-aware slot）
//
// 硬约束：本 crate **零依赖**（仅 std），不得引用 hecs / wgpu / egui。
// 需要 wgpu 类型的后端 trait（RenderBackend / RenderBackend3D）留在 zela-render；
// 需要 hecs::Entity 的旧 CacheKey 兼容层（From<CacheKey> for ResourceKey）留在 zela-render。

pub mod render_backend;
pub mod resource_cache;

pub use render_backend::{AccessError, GpuHandle, GpuResourceId};
pub use resource_cache::{DeferredDestroyer, GpuResourceCache, ResourceCache, ResourceKey};
