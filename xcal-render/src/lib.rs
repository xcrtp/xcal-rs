// xcal-render: wgpu 渲染层
// 依赖方向：xcal-core ← xcal-render
//
// 包含：
//   - scene::components::render 子模块（StrokeSolid / FillSolid / FillStyle / StrokeStyle）
//     并从 xcal_core::scene::components 重新导出非 render 部分 + Circle/Path/Line
//   - Painter2D / DrawQueue / ZIndex / Shape / Style / SceneRender / record_scene
//   - RenderBackend / GpuHandle / EframeWgpuBackend (render_backend.rs)
//   - RenderContext (render_context.rs)
//   - ResourceCache / GpuResourceCache / CacheKey (resource_cache.rs)

pub mod render_backend;
pub mod render_context;
pub mod resource_cache;
pub mod components;
pub mod draw;

pub use draw::{
    DrawCommand, DrawQueue, Painter2D, SceneRender, Shape, Style, ZIndex, record_scene,
};
pub use render_backend::{EframeWgpuBackend, GpuHandle, RenderBackend};
pub use render_context::RenderContext;
pub use resource_cache::{CacheKey, GpuResourceCache, ResourceCache};

// 为下游用户提供与原 xcal::scene::comp::{StrokeSolid, FillSolid} 一致的访问
// 通过在 scene 根重新导出 components + render 组件。
pub mod scene {
    pub use crate::components as comp;
    pub use crate::components as components;
    pub use xcal_core::scene::Scene;
}
