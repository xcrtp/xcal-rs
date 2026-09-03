// xc-time: 纯逻辑时钟树 + 动画曲线 + Director
//
// 本 crate 位于 kernel 层，仅依赖 glam / serde / thiserror，
// 零渲染 / 零 ECS / 零 UI 依赖。所有 eval_* 函数均为纯函数。

pub mod clock;
pub mod curve;
pub mod director;
pub mod error;

pub use clock::{ClockForest, ClockId, ClockNode, PlayMode};
pub use curve::{AnimValue, Curve, Interp};
pub use director::Director;
pub use error::TimeError;
