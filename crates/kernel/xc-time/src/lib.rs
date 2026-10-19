// xc-time: 纯逻辑时钟树 + 动画曲线 + Director
//
// 本 crate 位于 kernel 层，仅依赖 xc-notion / serde / thiserror（glam 为禁用项），
// 零渲染 / 零 ECS / 零 UI 依赖。所有 eval_* 函数均为纯函数。
// 路线甲：可依赖 xc-notion（插值原语），但**不得**依赖 xc-content
// （语义配对的知识留在外层）。

pub mod clock;
pub mod curve;
pub mod director;
pub mod easing;
pub mod error;

pub use clock::{ClockForest, ClockId, ClockNode, PlayMode};
pub use curve::{AnimValue, Curve, Interp};
pub use director::Director;
pub use easing::Easing;
pub use error::TimeError;
