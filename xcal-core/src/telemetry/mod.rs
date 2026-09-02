// telemetry/
//
// 可观测性基础设施：结构化日志、System 执行计时（FrameReport）、
// World 状态快照。
//
// 整个模块仅在 `feature = "telemetry"`（默认开启）下编译；
// 关闭 feature（`--no-default-features` 或 `--no-default-features
// --features egui`）时所有观测代码在编译期完全剔除，零运行时开销。
//
// 设计原则：
// - 零侵入：Span / 计时由 Scheduler 统一注入，业务 System 无需改动。
// - 正交性：本模块只看 ECS / Scheduler / 帧指标，不依赖渲染层。

mod frame_report;
mod log;
mod snapshot;

pub use frame_report::{FrameReport, SystemTelemetry};
pub use log::init_tracing;
pub use snapshot::{
    ComponentInfo, EntitySnapshot, ResourceSnapshot, SnapshotRegistry, WorldSnapshot,
    write_snapshot_json,
};
