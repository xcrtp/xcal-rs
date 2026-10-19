// telemetry/clock.rs
//
// 可注入的时间源。
//
// 为什么不能硬编码 `std::time::Instant`：`wasm32-unknown-unknown` 上 std 的
// wasm PAL 没有实现时钟，`Instant::now()` 会直接 panic。Scheduler 默认开启
// telemetry ⇒ 若把 `Instant` 写死在每帧计时里，浏览器端第一帧即崩。
//
// 因此计时源以「可注入 + 默认安全」为口径：
// - **默认** [`ZeroClock`]：不注入也能跑（耗时恒 0，不 panic）；
// - native 宿主（`xcal-app`）显式注入 [`InstantClock`]，telemetry 行为与注入前一致；
// - 浏览器入口注入基于 `performance.now()` 的实现。
//
// 本 crate 不写 `#[cfg(target_arch)]`：平台差异由调用方选择实现来表达。

use std::time::Duration;
use std::time::Instant;

/// 单调时间源：返回值的原点任意，只用于求差。
pub trait FrameClock {
    fn now(&self) -> Duration;
}

/// **默认时间源**：恒返回 `Duration::ZERO`。
///
/// 使"忘记注入"成为**安全行为**（耗时恒 0），而不是在 wasm 上 panic。
/// 需要真实计时时由宿主注入 [`InstantClock`] 或平台等价实现。
#[derive(Debug, Default, Clone, Copy)]
pub struct ZeroClock;

impl FrameClock for ZeroClock {
    fn now(&self) -> Duration {
        Duration::ZERO
    }
}

/// native 默认时钟：`Instant` 相对构造时刻的单调读数。
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
