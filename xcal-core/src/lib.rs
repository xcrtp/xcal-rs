// xcal-core: 纯逻辑层
// 包含：错误、类型(数学+相机)、Transform2D trait 数学部分、
//      resource（Resource marker + Resources + ResourceTuple）、
//      access（Access 冲突检测）、
//      context（ExecutorContext + debug BorrowTracker）、
//      system（System trait + Sys 闭包适配器 + InitSystem + ErasedSystem）、
//      scheduler（Phase + Scheduler + 冲突校验 + 借用追踪）、
//      scene 中的 marker/mesh/animation 子模块 + Circle/Path/Line/Scene 等纯逻辑组件、
//      utils 宏

pub mod access;
pub mod context;
pub mod errors;
pub mod resource;
pub mod scene;
pub mod scheduler;
pub mod system;
// 可观测性基础设施（日志 / FrameReport / World 快照），feature = "telemetry"。
#[cfg(feature = "telemetry")]
pub mod telemetry;
pub mod transform;
pub mod types;
pub mod utils;

pub use access::Access;
#[cfg(debug_assertions)]
pub use context::BorrowTracker;
pub use context::ExecutorContext;
pub use errors::Error;
pub use resource::{Resource, ResourceTuple, ResourceTupleDefault, Resources};
pub use scene::{Scene, comp, components, primitives};
pub use scheduler::{Phase, Scheduler};
pub use system::{ErasedSystem, InitSystem, Sys, System, sys};
pub use transform::{Transform2D, Transform2DInto};
pub use types::{Camera2D, Color32, I8Vec4, Rect, U8Vec4, Vec2, Vec3, Vec4};
// `#[macro_export]` 已经把 auto_into 宏导出到 crate 根（与模块无关），
// 下游通过 `#[macro_use]` 或 `xcal_core::auto_into!` 访问。

pub type Result<T> = std::result::Result<T, Error>;

// 为 xcal-macro 派生宏生成的 `xcal_core::` 路径提供别名。
// xcal-macro 现在生成 `xcal_core::transform::Transform2DInto` 等。
#[allow(unused_extern_crates)]
extern crate self as xcal_core;
