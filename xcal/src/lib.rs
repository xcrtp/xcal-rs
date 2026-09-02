pub mod app;
pub mod executor;
pub mod render;
pub mod render_backend;
pub mod render_context;
pub mod resource_cache;
pub mod scene;
pub mod scheduler;
pub mod types;
pub mod errrors;
pub mod utils;

pub use app::App;
#[cfg(feature = "egui")]
pub use app::EguiFrameContext;
pub use executor::{ExecutorContext, Resources, System};
pub use errrors::Error;
pub use render_backend::{GpuHandle, RenderBackend};
pub use render_context::RenderContext;
pub use resource_cache::{CacheKey, GpuResourceCache, ResourceCache};
pub use scheduler::{Phase, Scheduler};
pub use render_backend::EframeWgpuBackend;
pub type Result<T> = std::result::Result<T, Error>;

// 在 crate 自身内部为派生宏生成的 `xcal::` 路径提供别名。
// `#[derive(xcal_macro::Transform2D)]` 生成的代码引用 `xcal::render::Transform2D` 等，
// 在 crate 内部默认无法直接用 `xcal::` 访问自身，需要此别名。
extern crate self as xcal;
