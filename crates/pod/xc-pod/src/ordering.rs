//! 逻辑时序（Ord）与显式 flush 边界。
//!
//! # 为什么不是墙钟
//!
//! 命令与事件都需要定序，但排序键**不得**包含 `Instant` / 时间戳：
//! 每次运行都不同，会把不确定性固化，并且**不报错** —— 只是每次输出微妙不同。
//!
//! 本模块的排序键是纯逻辑量：
//!
//! ```text
//! SeqKey = (frame, source, seq)
//!           │       │       └─ 该源在该帧内的递增序号
//!           │       └───────── 发布方 Pod 的寻址句柄（PodHandle）
//!           └───────────────── 逻辑帧号（由 Driver 的 Advance/Batch 推进）
//! ```
//!
//! `PodHandle` 的 `Ord` 是 `PodId` 的 `Ord`，因此键是**全序**：
//! 同帧同时间戳的两个事件也能靠 `(source, seq)` 定序。
//!
//! # Ord 由框架提供
//!
//! [`Sequenced<T>`] 的 `Ord` **只比较 [`SeqKey`]**，不要求 `T: Ord`，
//! 也不让实现方 `derive(Ord)`（枚举变体顺序会随定义顺序悄悄变化）。
//!
//! # flush 边界
//!
//! 排序只在一个 flush 批次内有效。边界必须由**显式逻辑事件**界定
//! （[`FlushBoundary::FrameEnd`] / [`FlushBoundary::BatchEnd`]），
//! **不得**用「每 16ms flush 一次」这类墙钟边界。
//!
//! [`FlushBuffer`] 默认走缓冲排序；`push_immediate` 是逃生舱，
//! **不保证顺序**（见方法文档）。

use std::cmp::Ordering;

use crate::handle::PodHandle;

/// 逻辑排序键：`(frame, source, seq)`。
///
/// 字段全部是整数 / 句柄，不含任何时间戳。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SeqKey {
    /// 逻辑帧号（`Advance` / `Batch` 推进）。
    pub frame: u64,
    /// 产生该条目的 Pod（扇出 / 排序的稳定身份）。
    pub source: PodHandle,
    /// 该源在该帧内的递增序号（由 [`Sequencer`] 分配）。
    pub seq: u64,
}

impl SeqKey {
    /// 逐字段构造。
    pub const fn new(frame: u64, source: PodHandle, seq: u64) -> Self {
        Self {
            frame,
            source,
            seq,
        }
    }
}

/// 框架提供的排序包装：`Ord` 只看 [`SeqKey`]。
///
/// 命令与事件都通过它获得逻辑序（见 [`OrderedCommand`] / [`OrderedEvent`]）。
///
/// # 相等性
///
/// `PartialEq` / `Eq` 也**只看键**（与 `Ord` 保持一致）：包装体的身份是它的
/// 逻辑位置，而不是负载值。要比较负载请显式用 [`Sequenced::value`]。
/// 好处是 `Ord` 对 `T` **零约束**（`T` 不必是 `Ord` / `Eq`）。
#[derive(Clone, Debug)]
pub struct Sequenced<T> {
    key: SeqKey,
    value: T,
}

impl<T> Sequenced<T> {
    /// 用逻辑键包装一个值。
    pub fn new(key: SeqKey, value: T) -> Self {
        Self { key, value }
    }

    /// 逻辑键。
    pub const fn key(&self) -> SeqKey {
        self.key
    }

    /// 值引用。
    pub fn value(&self) -> &T {
        &self.value
    }

    /// 拆成 `(键, 值)`。
    pub fn into_parts(self) -> (SeqKey, T) {
        (self.key, self.value)
    }

    /// 只映射值，键不变。
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Sequenced<U> {
        Sequenced {
            key: self.key,
            value: f(self.value),
        }
    }
}

impl<T> PartialEq for Sequenced<T> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl<T> Eq for Sequenced<T> {}

impl<T> PartialOrd for Sequenced<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Sequenced<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // **只比较逻辑键**：不要求 `T: Ord`，也不会退化成"按值排序"。
        self.key.cmp(&other.key)
    }
}

/// 有序命令的框架类型别名（命令与事件都必须经它获得逻辑序）。
pub type OrderedCommand<C> = Sequenced<C>;
/// 有序事件的框架类型别名。
pub type OrderedEvent<E> = Sequenced<E>;

/// 每个源 Pod 的逻辑序号分配器。
///
/// `seq` 在 `begin_frame` 时归零 ⇒ 键 `(frame, source, seq)` 在
/// "同一源 + 同一帧"内唯一，因而全序。
pub struct Sequencer {
    source: PodHandle,
    frame: u64,
    seq: u64,
}

impl Sequencer {
    /// 为 `source` 新建分配器（帧号 0、序号 0）。
    pub fn new(source: PodHandle) -> Self {
        Self {
            source,
            frame: 0,
            seq: 0,
        }
    }

    /// 源 Pod。
    pub const fn source(&self) -> PodHandle {
        self.source
    }

    /// 当前逻辑帧号。
    pub const fn frame(&self) -> u64 {
        self.frame
    }

    /// 当前序号（下一条要分配的 `seq`）。
    pub const fn next_seq(&self) -> u64 {
        self.seq
    }

    /// 进入新帧：复位帧内序号（**显式逻辑边界**，不是墙钟）。
    pub fn begin_frame(&mut self, frame: u64) {
        self.frame = frame;
        self.seq = 0;
    }

    /// 包装一个值并分配序号。
    pub fn next<T>(&mut self, value: T) -> Sequenced<T> {
        let key = SeqKey::new(self.frame, self.source, self.seq);
        self.seq += 1;
        Sequenced::new(key, value)
    }
}

/// 显式逻辑 flush 边界。
///
/// 只允许这两类：帧结束 / `Batch` 命令结束。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlushBoundary {
    /// 帧结束（`Advance(1)` 或 `Batch` 的每一帧）。
    FrameEnd {
        /// 已完成的逻辑帧号。
        frame: u64,
    },
    /// `Batch` 命令结束（整批推进完成）。
    BatchEnd {
        /// 该批次的命令序号（由调用方分配）。
        command_seq: u64,
    },
}

/// 一个 flush 批次的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlushBatch<T> {
    /// 触发该批次的显式逻辑边界。
    pub boundary: FlushBoundary,
    /// **已排序**的条目（按键升序）。
    pub ordered: Vec<Sequenced<T>>,
    /// 即时通道的条目（**不保证顺序**，按到达顺序返回）。
    pub immediate: Vec<T>,
}

/// flush 批次缓冲。
///
/// - 默认通道 [`FlushBuffer::push`]：进入批次，`flush` 时按逻辑键排序；
/// - 即时通道 [`FlushBuffer::push_immediate`]：逃生舱，**不参与排序**。
#[derive(Debug)]
pub struct FlushBuffer<T> {
    ordered: Vec<Sequenced<T>>,
    immediate: Vec<T>,
}

impl<T> Default for FlushBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> FlushBuffer<T> {
    /// 空缓冲。
    pub fn new() -> Self {
        Self {
            ordered: Vec::new(),
            immediate: Vec::new(),
        }
    }

    /// 默认通道：进入当前批次（`flush` 时排序）。
    pub fn push(&mut self, item: Sequenced<T>) {
        self.ordered.push(item);
    }

    /// 即时通道（逃生舱）：**不保证顺序**。
    ///
    /// 只用于"必须立刻处理、且不参与确定性断言"的观测类条目。
    /// 默认路径应使用 [`Self::push`]。
    pub fn push_immediate(&mut self, item: T) {
        self.immediate.push(item);
    }

    /// 缓冲中的有序条目数。
    pub fn len(&self) -> usize {
        self.ordered.len()
    }

    /// 是否为空（含即时通道）。
    pub fn is_empty(&self) -> bool {
        self.ordered.is_empty() && self.immediate.is_empty()
    }

    /// 在**显式逻辑边界**上取走整批：有序条目按 `SeqKey` 升序。
    pub fn flush(&mut self, boundary: FlushBoundary) -> FlushBatch<T> {
        let mut ordered = std::mem::take(&mut self.ordered);
        // 键由 `Sequencer` 保证唯一；`sort`（稳定）即使在重复键下也保持到达序。
        ordered.sort();
        FlushBatch {
            boundary,
            ordered,
            immediate: std::mem::take(&mut self.immediate),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handle::PodId;

    fn source() -> PodHandle {
        PodHandle::new(PodId::new(7))
    }

    #[test]
    fn key_is_a_total_order_across_sources() {
        let a = PodHandle::new(PodId::new(1));
        let b = PodHandle::new(PodId::new(2));
        let mut keys = vec![
            SeqKey::new(2, a, 0),
            SeqKey::new(1, b, 5),
            SeqKey::new(1, a, 3),
            SeqKey::new(1, a, 2),
        ];
        keys.sort();
        assert_eq!(
            keys,
            vec![
                SeqKey::new(1, a, 2),
                SeqKey::new(1, a, 3),
                SeqKey::new(1, b, 5),
                SeqKey::new(2, a, 0),
            ],
            "同帧时按 (source, seq) 定序"
        );
    }

    #[test]
    fn sequencer_resets_seq_per_frame() {
        let mut sequencer = Sequencer::new(source());
        sequencer.begin_frame(1);
        assert_eq!(sequencer.next(10u32).key(), SeqKey::new(1, source(), 0));
        assert_eq!(sequencer.next(11u32).key(), SeqKey::new(1, source(), 1));
        sequencer.begin_frame(2);
        assert_eq!(sequencer.next(12u32).key(), SeqKey::new(2, source(), 0));
    }

    #[test]
    fn flush_sorts_and_keeps_immediate_separate() {
        let mut buffer = FlushBuffer::new();
        buffer.push(Sequenced::new(SeqKey::new(1, source(), 2), "c"));
        buffer.push(Sequenced::new(SeqKey::new(1, source(), 0), "a"));
        buffer.push(Sequenced::new(SeqKey::new(1, source(), 1), "b"));
        buffer.push_immediate("now");

        let batch = buffer.flush(FlushBoundary::FrameEnd { frame: 1 });
        let values: Vec<&str> = batch.ordered.iter().map(|item| *item.value()).collect();
        assert_eq!(values, vec!["a", "b", "c"]);
        assert_eq!(batch.immediate, vec!["now"]);
        assert!(buffer.is_empty());
    }
}
