// diagnostics.rs — 渲染期**可观测的软失败计数**。
//
// # 口径（写死）
//
// 三角化失败（`Shape2D::Custom` 的 `SpecId` 未注册 / 规格无几何）是**软失败**：
// 记录一条**命名原因**的日志、跳过该绘制、继续渲染 —— **不中断整帧、不改变退出码**。
//
// 但"软"不等于"无声"：本模块提供计数，
// - 每帧结束打印 `本帧 N 个绘制被跳过`；
// - 进程退出前可读累计值（`zela-render` 会打印）。
//
// bundle 路径另有**前置硬闸**（`Bundle::check_restorable`）；`--script` 路径没有，
// 靠这里的计数事后发现。若将来改为硬失败，只需让调用点在 `record_skipped_draw`
// 后返回错误 —— 口径变更点集中在这一个函数。

use std::sync::atomic::{AtomicUsize, Ordering};

static FRAME_SKIPPED: AtomicUsize = AtomicUsize::new(0);
static TOTAL_SKIPPED: AtomicUsize = AtomicUsize::new(0);

/// 记录一次"因三角化失败而跳过"的绘制（后端调用）。
///
/// B3-0d 起 2D 三角化失败改为**显式错误**（不再跳过）⇒ 生产路径不再调用本函数；
/// 计数器与其对外读取口保留（诊断 API 稳定，测试仍覆盖）。
#[allow(dead_code)]
pub(crate) fn record_skipped_draw() {
    FRAME_SKIPPED.fetch_add(1, Ordering::Relaxed);
    TOTAL_SKIPPED.fetch_add(1, Ordering::Relaxed);
}

/// 取走**本帧**的跳过数（帧末汇报用；取走即清零）。
pub fn take_frame_skipped() -> usize {
    FRAME_SKIPPED.swap(0, Ordering::Relaxed)
}

/// 自进程启动以来的累计跳过数（退出前汇总用）。
pub fn total_skipped() -> usize {
    TOTAL_SKIPPED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_are_frame_scoped_and_cumulative() {
        // 测试并行下其他用例也可能计数 ⇒ 只断言"增量"与"取走即清零"。
        let before_total = total_skipped();
        record_skipped_draw();
        record_skipped_draw();
        assert!(total_skipped() >= before_total + 2);
        let frame = take_frame_skipped();
        assert!(frame >= 2, "frame = {frame}");
        assert_eq!(take_frame_skipped(), 0, "取走后必须清零");
    }
}
