// zela-schedule: 通用 ECS 调度内核
//
// 与「场景」概念无关的通用能力，独立于 zela-scene 的场景数据层：
//      resource（Resource marker + Resources + ResourceTuple）、
//      access（Access 冲突检测）、
//      context（ExecutorContext + debug BorrowTracker）、
//      system（System trait + Sys 闭包适配器 + InitSystem + ErasedSystem）、
//      scheduler（Phase + Scheduler + 冲突校验 + 借用追踪）、
//      telemetry（日志 / FrameReport / World 快照）
//
// 依赖仅 hecs（+ telemetry feature 下的 tracing/serde），不依赖 zela-scene /
// zela-render / zela-egui / wgpu。

pub mod access;
pub mod context;
pub mod resource;
pub mod scheduler;
pub mod system;
// 可观测性基础设施（日志 / FrameReport / World 快照），feature = "telemetry"。
#[cfg(feature = "telemetry")]
pub mod telemetry;

pub use access::Access;
#[cfg(debug_assertions)]
pub use context::BorrowTracker;
pub use context::ExecutorContext;
pub use resource::{Resource, ResourceTuple, ResourceTupleDefault, Resources};
pub use scheduler::{Phase, Scheduler};
pub use system::{ErasedSystem, InitSystem, Sys, System, sys};
