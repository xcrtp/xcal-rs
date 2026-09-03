use thiserror::Error;

use crate::clock::ClockId;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TimeError {
    #[error("clock {0:?} not found")]
    ClockNotFound(ClockId),
    #[error("clock tree contains a cycle starting from {0:?}")]
    CycleDetected(ClockId),
    #[error("reverse rate requires seek-kind replay (B类演化器不支持O1 seek)")]
    ReverseNotSeekable,
}
