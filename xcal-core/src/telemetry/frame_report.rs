// telemetry/frame_report.rs
//
// 单帧 System 执行报告：每个 System 的耗时、所属阶段、实体数等。
// 由 Scheduler 在 `feature = "telemetry"` 下每帧采集，
// 可用于控制台汇总（每 60 帧）或上层 egui health overlay。

use std::time::Duration;

use crate::scheduler::Phase;

/// 单个 System 在某一帧的执行遥测。
#[derive(Debug, Clone)]
pub struct SystemTelemetry {
    /// System 的完整类型名（`std::any::type_name`）。
    pub system_name: &'static str,
    /// 所属调度阶段。
    pub phase: Phase,
    /// 帧序号（Scheduler 启动以来单调递增）。
    pub tick: u64,
    /// 本次执行 wall-clock 耗时。
    pub duration: Duration,
    /// 执行前 World 中的实体数（粗略工作量参考）。
    pub entities_touched: usize,
    /// 声明读取的资源类型名（保留字段；当前从 TypeId 无法反查名称，恒空）。
    pub resources_read: Vec<&'static str>,
    /// 声明写入的资源类型名（同上，恒空）。
    pub resources_written: Vec<&'static str>,
}

/// 一帧内所有 System 的执行报告。
#[derive(Debug, Default, Clone)]
pub struct FrameReport {
    /// 帧序号。
    pub frame: u64,
    /// 整帧 wall-clock 耗时（含阶段间 CommandBuffer flush）。
    pub total_duration: Duration,
    /// 本帧执行的 System 遥测，按执行顺序排列。
    pub systems: Vec<SystemTelemetry>,
}

impl FrameReport {
    /// 记录一个 System 的执行结果，并累加总耗时。
    pub fn record(&mut self, tel: SystemTelemetry) {
        self.total_duration += tel.duration;
        self.systems.push(tel);
    }

    /// 按耗时降序返回 System 遥测，便于定位瓶颈。
    pub fn sorted(&self) -> Vec<&SystemTelemetry> {
        let mut v: Vec<&SystemTelemetry> = self.systems.iter().collect();
        v.sort_by_key(|t| std::cmp::Reverse(t.duration));
        v
    }

    /// 本帧执行的 System 数量。
    pub fn system_count(&self) -> usize {
        self.systems.len()
    }

    /// 控制台汇总（debug 构建下每 60 帧由 Scheduler 调用一次）。
    ///
    /// 输出帧号、整帧耗时（ms）与耗时最长的前 5 个 System。
    pub fn log_summary(&self) {
        let top = self.sorted();
        let top_n = top.iter().take(5).fold(String::new(), |mut acc, t| {
            use std::fmt::Write;
            let _ = write!(
                &mut acc,
                "\n    {:>8.3} ms  [{:?}] {}",
                t.duration.as_secs_f64() * 1000.0,
                t.phase,
                short_name(t.system_name)
            );
            acc
        });
        tracing::info!(
            "frame {}: {:.3} ms, {} systems; top:{}",
            self.frame,
            self.total_duration.as_secs_f64() * 1000.0,
            self.systems.len(),
            top_n
        );
    }
}

/// 把 `xcal_egui::systems::animation::AnimationApplySystem` 缩短为末段类型名。
fn short_name(full: &str) -> &str {
    full.rsplit("::").next().unwrap_or(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tel(name: &'static str, phase: Phase, micros: u64) -> SystemTelemetry {
        SystemTelemetry {
            system_name: name,
            phase,
            tick: 1,
            duration: Duration::from_micros(micros),
            entities_touched: 0,
            resources_read: Vec::new(),
            resources_written: Vec::new(),
        }
    }

    #[test]
    fn test_record_accumulates_total() {
        let mut r = FrameReport::default();
        r.record(tel("a", Phase::Input, 100));
        r.record(tel("b", Phase::Prepare, 200));
        assert_eq!(r.system_count(), 2);
        assert_eq!(r.total_duration, Duration::from_micros(300));
    }

    #[test]
    fn test_sorted_is_descending_by_duration() {
        let mut r = FrameReport::default();
        r.record(tel("fast", Phase::Input, 10));
        r.record(tel("slow", Phase::RenderCommit, 500));
        r.record(tel("mid", Phase::Prepare, 100));
        let sorted = r.sorted();
        assert_eq!(sorted[0].system_name, "slow");
        assert_eq!(sorted[1].system_name, "mid");
        assert_eq!(sorted[2].system_name, "fast");
    }

    #[test]
    fn test_short_name_takes_last_segment() {
        assert_eq!(
            short_name("xcal_egui::systems::TimeUpdateSystem"),
            "TimeUpdateSystem"
        );
        assert_eq!(short_name("BareSystem"), "BareSystem");
    }
}
