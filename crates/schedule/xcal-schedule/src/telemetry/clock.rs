// telemetry/clock.rs
//
// 可注入的时间源。
//
// 为什么不能硬编码 `std::time::Instant`：`wasm32-unknown-unknown` 上 std 的
// wasm PAL 没有实现时钟，`Instant::now()` 会直接 panic。Scheduler 默认开启
// telemetry ⇒ 若把 `Instant` 写死在每帧计时里，浏览器端第一帧即崩。
//
// 因此计时源由调用方注入：
// - native：默认 [`InstantClock`]（行为与注入前一致）；
// - 浏览器：在推进帧之前注入基于 `performance.now()` 的实现。
//
// 本 crate 不写 `#[cfg(target_arch)]`：平台差异由调用方选择实现来表达。

use std::time::Duration;
use std::time::Instant;

/// 单调时间源：返回值的原点任意，只用于求差。
pub trait FrameClock {
    fn now(&self) -> Duration;
}

/// 默认时间源：`Instant` 相对构造时刻的单调读数（native 行为不变）。
///
/// **惰性构造**：浏览器入口须在第一次 `run_frame` 之前调用
/// [`crate::Scheduler::set_clock`]，否则会落到本实现并在 wasm 上 panic。
#[derive(Debug)]
pub struct InstantClock {
    origin: Instant,
}

impl InstantClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for InstantClock {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameClock for InstantClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}
