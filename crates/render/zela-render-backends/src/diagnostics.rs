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

// ─── B12 Step 5：缓存命中的对外出口 + 资源计数 ───────────────────────────
//
// 此前 `FrameCacheStats` 有定义、有填充、**零外部消费者**：每帧都算，没人看。
// 这里把它接出来（进程级累计），并补一个"纹理创建次数"计数 ——
// B8 Step 0 修的正是"每帧建纹理"，而当时没有任何计数能证明修好了。

static CACHE_GEOM_REQUESTS: AtomicUsize = AtomicUsize::new(0);
static CACHE_GEOM_MISSES: AtomicUsize = AtomicUsize::new(0);
static CACHE_MORPH_REQUESTS: AtomicUsize = AtomicUsize::new(0);
static CACHE_MORPH_MISSES: AtomicUsize = AtomicUsize::new(0);
static CACHE_REVEAL_REQUESTS: AtomicUsize = AtomicUsize::new(0);
static CACHE_REVEAL_MISSES: AtomicUsize = AtomicUsize::new(0);
static TEXTURES_CREATED: AtomicUsize = AtomicUsize::new(0);

/// 缓存计数累计（语义与 `FrameCacheStats` 逐字段一致）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheTotals {
    pub geom_requests: usize,
    pub geom_misses: usize,
    pub morph_requests: usize,
    pub morph_misses: usize,
    pub reveal_requests: usize,
    pub reveal_misses: usize,
}

impl CacheTotals {
    /// 一行 JSON（`state` 之外不掺任何时间/次数以外的解释）。
    pub fn json(&self) -> String {
        format!(
            "{{\"geom\":{{\"requests\":{},\"misses\":{}}},\
             \"morph\":{{\"requests\":{},\"misses\":{}}},\
             \"reveal\":{{\"requests\":{},\"misses\":{}}}}}",
            self.geom_requests,
            self.geom_misses,
            self.morph_requests,
            self.morph_misses,
            self.reveal_requests,
            self.reveal_misses,
        )
    }
}

/// 后端每帧上报一次（`scene_callback` 的两个 2D finish 点各一次）。
pub(crate) fn record_cache(
    geom_requests: usize,
    geom_misses: usize,
    morph_requests: usize,
    morph_misses: usize,
    reveal_requests: usize,
    reveal_misses: usize,
) {
    CACHE_GEOM_REQUESTS.fetch_add(geom_requests, Ordering::Relaxed);
    CACHE_GEOM_MISSES.fetch_add(geom_misses, Ordering::Relaxed);
    CACHE_MORPH_REQUESTS.fetch_add(morph_requests, Ordering::Relaxed);
    CACHE_MORPH_MISSES.fetch_add(morph_misses, Ordering::Relaxed);
    CACHE_REVEAL_REQUESTS.fetch_add(reveal_requests, Ordering::Relaxed);
    CACHE_REVEAL_MISSES.fetch_add(reveal_misses, Ordering::Relaxed);
}

/// 取走累计缓存计数（取走即清零 ⇒ 每次运行拿到的是"本次运行"的值）。
pub fn take_cache_totals() -> CacheTotals {
    CacheTotals {
        geom_requests: CACHE_GEOM_REQUESTS.swap(0, Ordering::Relaxed),
        geom_misses: CACHE_GEOM_MISSES.swap(0, Ordering::Relaxed),
        morph_requests: CACHE_MORPH_REQUESTS.swap(0, Ordering::Relaxed),
        morph_misses: CACHE_MORPH_MISSES.swap(0, Ordering::Relaxed),
        reveal_requests: CACHE_REVEAL_REQUESTS.swap(0, Ordering::Relaxed),
        reveal_misses: CACHE_REVEAL_MISSES.swap(0, Ordering::Relaxed),
    }
}

/// 记录一次纹理创建（每创建一个 `wgpu::Texture` 调一次；**创建** ≠ 每帧）。
pub(crate) fn record_textures_created(count: usize) {
    TEXTURES_CREATED.fetch_add(count, Ordering::Relaxed);
}

/// 自进程启动以来创建的纹理总数。
///
/// 稳态（内容不变、尺寸不变）下，逐帧增量应为 **0** —— 这就是 B8 "每帧建纹理"
/// 修好与否的判据。
pub fn textures_created() -> usize {
    TEXTURES_CREATED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B12 Step 5：缓存计数可累加、可**取走即清零**（每次运行拿到本次运行的值）。
    #[test]
    fn cache_totals_accumulate_and_are_taken() {
        let _ = take_cache_totals(); // 清掉并行测试可能留下的值
        record_cache(3, 1, 2, 0, 5, 4);
        let totals = take_cache_totals();
        assert_eq!(totals.geom_requests, 3);
        assert_eq!(totals.geom_misses, 1);
        assert_eq!(totals.morph_requests, 2);
        assert_eq!(totals.morph_misses, 0);
        assert_eq!(totals.reveal_requests, 5);
        assert_eq!(totals.reveal_misses, 4);
        assert!(totals.json().contains("\"geom\":{\"requests\":3,\"misses\":1}"));
        assert_eq!(take_cache_totals(), CacheTotals::default(), "取走后必须清零");
    }

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
