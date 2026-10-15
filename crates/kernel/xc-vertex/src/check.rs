// check.rs
//
// debug 跨帧一致性校验（V5，§4.5）。
//
// 策略声明最大的风险是**说谎** —— 三角化器声明 `CacheStrategy::Static`，
// 实际每帧都在变。机制：debug 构建下，对声明为 Static 的格式，
// 在连续两帧比对数据哈希，不一致则 panic 并指明是哪个格式撒了谎。
//
// 成本控制（§4.5）：
//   - 仅在 `debug_assertions` 下启用检查；release 构建下所有方法均为 no-op
//     （类型始终存在，消费方代码无需 cfg 门控，release 零开销）。
//   - 仅对 `Static` 声明执行：`PerFrame` / `ViewDependent` / `MorphTarget` 本就预期变化，
//     提交给本校验器会被静默忽略。
//   - 大 buffer 可改用采样哈希（`submit_static_sampled` / [`hash_bytes_sampled`]）。

use std::collections::HashMap;

use super::cache::CacheStrategy;
use super::layout::VertexLayout;

// ─── 哈希工具（FNV-1a 64 位，无外部依赖） ─────────────────────────────────

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// 全量 FNV-1a 64 位哈希。
pub fn hash_bytes(data: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// 采样哈希：头尾各取最多 64 字节 + 长度混入。
///
/// 大数据量的退化方案（§4.5：若实测全量哈希开销过大）。
/// 注意：采样哈希区分度低于全量哈希，只在 buffer 很大时使用。
pub fn hash_bytes_sampled(data: &[u8]) -> u64 {
    const WINDOW: usize = 64;
    if data.len() <= WINDOW * 2 {
        return hash_bytes(data);
    }
    let mut h = FNV_OFFSET;
    fn feed(bytes: &[u8], h: &mut u64) {
        for &b in bytes {
            *h ^= b as u64;
            *h = h.wrapping_mul(FNV_PRIME);
        }
    }
    feed(&data[..WINDOW], &mut h);
    feed(&data[data.len() - WINDOW..], &mut h);
    feed(&data.len().to_le_bytes(), &mut h);
    h
}

// ─── 校验器 ───────────────────────────────────────────────────────────────

/// debug 跨帧一致性校验器（§4.5）。
///
/// 用法（消费方，通常在 RenderSubmit / 缓存层入口）：
///
/// ```ignore
/// let mut checker = StaticConsistencyChecker::new();
/// loop {
///     checker.begin_frame();
///     for mesh in static_meshes {
///         checker.submit_static(mesh.id, &mesh.layout, &mesh.vertex_bytes);
///     }
/// }
/// ```
///
/// 语义：
/// - `(layout, identity)` 连续两帧都被提交时，数据哈希必须一致，否则 panic；
/// - 某帧缺席（对象消失）不算撒谎；隔帧后以新数据重新出现也不算
///   （校验的是"连续两帧"，而非"历史出现过"）。
///
/// release 构建下所有方法为 no-op（`cfg!(debug_assertions)` 常量折叠）。
#[derive(Debug, Default)]
pub struct StaticConsistencyChecker {
    /// 上一帧快照：(layout, identity) -> data_hash。
    prev: HashMap<(VertexLayout, u64), u64>,
    /// 本帧记录。
    curr: HashMap<(VertexLayout, u64), u64>,
    frame: u64,
}

impl StaticConsistencyChecker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 每帧开始调用：把上一帧记录定为比较基准。
    pub fn begin_frame(&mut self) {
        if !cfg!(debug_assertions) {
            return;
        }
        let curr = std::mem::take(&mut self.curr);
        self.prev = curr;
        self.frame = self.frame.wrapping_add(1);
    }

    /// 提交一个 Static 对象的本帧数据（内部做全量 FNV-1a 哈希）。
    #[track_caller]
    pub fn submit_static(&mut self, identity: u64, layout: &VertexLayout, data: &[u8]) {
        self.submit_static_hash(identity, layout, hash_bytes(data));
    }

    /// 提交一个 Static 对象的本帧数据（采样哈希，大数据量退化方案）。
    #[track_caller]
    pub fn submit_static_sampled(&mut self, identity: u64, layout: &VertexLayout, data: &[u8]) {
        self.submit_static_hash(identity, layout, hash_bytes_sampled(data));
    }

    /// 提交一个 Static 对象的本帧数据哈希（调用方已自算哈希时使用）。
    #[track_caller]
    pub fn submit_static_hash(&mut self, identity: u64, layout: &VertexLayout, data_hash: u64) {
        if !cfg!(debug_assertions) {
            return;
        }
        // 仅对 Static 声明执行；其余策略本就预期变化
        if !matches!(layout.cache, CacheStrategy::Static) {
            return;
        }
        let key = (layout.clone(), identity);
        if let Some(&prev_hash) = self.prev.get(&key) {
            if prev_hash != data_hash {
                panic!(
                    "顶点格式契约校验失败：布局 {:?}（identity = {identity}）声明为 \
                     CacheStrategy::Static，但第 {} 帧与上一帧的数据哈希不一致 \
                     （{prev_hash:#018x} != {data_hash:#018x}）。\
                     数据每帧在变 —— 请改用 PerFrame/ViewDependent，\
                     或修正三角化器的 Static 声明。",
                    layout, self.frame,
                );
            }
        }
        self.curr.insert(key, data_hash);
    }

    /// 当前帧号（begin_frame 次数）。
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// 本帧已记录的 Static 对象数。
    pub fn tracked_len(&self) -> usize {
        self.curr.len()
    }
}

// ─── 单测（V5） ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AaModel, CacheStrategy};

    fn static_layout() -> VertexLayout {
        VertexLayout::standard(AaModel::None, CacheStrategy::Static)
    }

    fn per_frame_layout() -> VertexLayout {
        VertexLayout::standard(AaModel::None, CacheStrategy::PerFrame)
    }

    /// 撒谎的 Static 数据：第二帧改内容 → panic，且消息指明是哪个格式。
    #[test]
    #[should_panic(expected = "顶点格式契约校验失败")]
    fn lying_static_data_panics_on_second_frame() {
        let layout = static_layout();
        let mut c = StaticConsistencyChecker::new();
        c.submit_static(1, &layout, &[1, 2, 3, 4]);
        c.begin_frame();
        // 诚实数据通过
        c.submit_static(1, &layout, &[1, 2, 3, 4]);
        c.begin_frame();
        // 撒谎：同一 identity 的内容变了
        c.submit_static(1, &layout, &[9, 9, 9, 9]);
    }

    /// 诚实数据不 panic（多对象、多帧）。
    #[test]
    fn honest_static_data_never_panics() {
        let layout = static_layout();
        let mut c = StaticConsistencyChecker::new();
        for _ in 0..5 {
            c.begin_frame();
            c.submit_static(1, &layout, &[1, 2, 3]);
            c.submit_static(2, &layout, &[4, 5, 6]);
        }
        assert_eq!(c.tracked_len(), 2);
    }

    /// 同一布局、不同 identity、不同数据 —— 不算撒谎。
    #[test]
    fn different_identities_may_differ() {
        let layout = static_layout();
        let mut c = StaticConsistencyChecker::new();
        c.submit_static(1, &layout, &[1, 1, 1]);
        c.begin_frame();
        c.submit_static(1, &layout, &[1, 1, 1]);
        c.submit_static(2, &layout, &[2, 2, 2]); // 新对象，首帧出现
        c.begin_frame();
        c.submit_static(2, &layout, &[2, 2, 2]);
    }

    /// 对象缺席一帧后以新数据重新出现 —— 校验的是"连续两帧"，不算撒谎。
    #[test]
    fn absent_object_may_reappear_with_new_data() {
        let layout = static_layout();
        let mut c = StaticConsistencyChecker::new();
        c.submit_static(1, &layout, &[1]);
        c.begin_frame(); // 第 1 帧：缺席
        c.begin_frame(); // 第 2 帧：以新数据回归
        c.submit_static(1, &layout, &[2, 2]);
    }

    /// 非 Static 策略提交被忽略 —— 数据每帧变也不 panic。
    #[test]
    fn non_static_submissions_are_ignored() {
        let layout = per_frame_layout();
        let mut c = StaticConsistencyChecker::new();
        c.submit_static(1, &layout, &[1]);
        c.begin_frame();
        c.submit_static(1, &layout, &[2]);
        c.begin_frame();
        c.submit_static(1, &layout, &[3]);
        assert_eq!(c.tracked_len(), 0, "PerFrame 不应被跟踪");
    }

    /// 采样哈希：小 buffer 等价全量；大 buffer 对内容变化敏感。
    #[test]
    fn sampled_hash_behavior() {
        assert_eq!(hash_bytes_sampled(&[1, 2, 3]), hash_bytes(&[1, 2, 3]));

        let big = vec![7u8; 4096];
        let mut big2 = big.clone();
        assert_eq!(hash_bytes_sampled(&big), hash_bytes_sampled(&big2));
        // 头部与尾部不变、中段变化 —— 采样哈希不敏感（已知局限）
        // 头部或尾部变化 —— 采样哈希必须敏感
        big2[0] ^= 1;
        assert_ne!(hash_bytes_sampled(&big), hash_bytes_sampled(&big2));
        let mut big3 = big.clone();
        let last = big3.len() - 1;
        big3[last] ^= 1;
        assert_ne!(hash_bytes_sampled(&big), hash_bytes_sampled(&big3));

        // FNV-1a 已知向量
        assert_eq!(hash_bytes(b""), 0xcbf29ce484222325);
        assert_eq!(hash_bytes(b"a"), 0xaf63dc4c8601ec8c);
    }
}
