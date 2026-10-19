// clock.rs — 纯数据时钟树
//
// 全部 eval_* 为纯函数，不持有/修改游标。
// 构造期环检测：parent 链成环立即返回 Err。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::TimeError;

/// 时钟节点的唯一标识符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClockId(pub u32);

/// 时钟回放模式（映射自旧 ClockState + Loop/PingPong）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayMode {
    /// 直接映射，不做任何包装（默认、最常见）。
    Step,
    /// 在 span 边界循环取模。
    Loop,
    /// 在 span 边界三角波反弹。
    PingPong,
    /// 冻结在 span 内：clamp 到 [lo, hi]（纯函数可表达；"暂停"由 Director.playing 负责）。
    Hold,
}

/// 单个时钟节点的纯数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockNode {
    pub parent: Option<ClockId>,
    pub local_offset: f64,
    pub rate: f64,
    /// 片段区间（可选）；None = 无限。
    pub span: Option<(f64, f64)>,
    pub mode: PlayMode,
}

impl ClockNode {
    pub fn new(parent: Option<ClockId>) -> Self {
        Self {
            parent,
            local_offset: 0.0,
            rate: 1.0,
            span: None,
            mode: PlayMode::Step,
        }
    }
}

/// 时钟森林（可多个根节点）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockForest {
    nodes: HashMap<ClockId, ClockNode>,
    next_id: u32,
}

impl Default for ClockForest {
    fn default() -> Self {
        Self::new()
    }
}

impl ClockForest {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            next_id: 0,
        }
    }

    /// 插入新节点并分配 id。
    ///
    /// parent 只能引用已存在的节点（新 id 不会出现在任何已有节点的祖先链上），
    /// 因此 insert 天然不可能成环；环检测由 [`ClockForest::set_parent`] 承担。
    pub fn insert(&mut self, node: ClockNode) -> Result<ClockId, TimeError> {
        let id = ClockId(self.next_id);
        self.next_id += 1;
        self.nodes.insert(id, node);
        Ok(id)
    }

    /// 重新挂载父节点（re-parent）。
    ///
    /// 沿 `new_parent` 的祖先链向上检测，若遇到 `id` 自身则成环，
    /// 返回 [`TimeError::CycleDetected`] 且林结构保持不变。
    pub fn set_parent(
        &mut self,
        id: ClockId,
        new_parent: Option<ClockId>,
    ) -> Result<(), TimeError> {
        if !self.nodes.contains_key(&id) {
            return Err(TimeError::ClockNotFound(id));
        }
        if let Some(p) = new_parent {
            if !self.nodes.contains_key(&p) {
                return Err(TimeError::ClockNotFound(p));
            }
            // 沿 new_parent 的祖先链向上走，遇到 id 即成环。
            let mut cur = p;
            loop {
                if cur == id {
                    return Err(TimeError::CycleDetected(id));
                }
                match self.nodes.get(&cur).and_then(|n| n.parent) {
                    Some(next) => cur = next,
                    None => break,
                }
            }
        }
        self.nodes.get_mut(&id).unwrap().parent = new_parent;
        Ok(())
    }

    pub fn get(&self, id: ClockId) -> Option<&ClockNode> {
        self.nodes.get(&id)
    }

    pub fn nodes(&self) -> impl Iterator<Item = (ClockId, &ClockNode)> {
        self.nodes.iter().map(|(&k, v)| (k, v))
    }

    /// 纯函数求值：给定全局 t，算出某时钟节点的本地时间。
    ///
    /// 递归：local(t) = map_into_span((parent_local(t) - offset) * rate, span, mode)
    pub fn eval_local(&self, id: ClockId, global_t: f64) -> f64 {
        self.eval_local_recursive(id, global_t, &mut std::collections::HashSet::new())
            .unwrap_or(global_t) // 节点不存在则 fallback 到全局时间
    }

    /// 该节点在**全局时间轴**上的起始偏移（沿祖先链累加 `local_offset`）。
    ///
    /// 用途：多段绑定按 `start` 升序应用（S1）。时间轴编译出的节点都是根节点
    /// （`parent = None`, `rate = 1`），此时它等于节点自己的 `local_offset`；
    /// 嵌套时钟只做偏移累加（`rate != 1` 的全局换算不在本方法口径内）。
    pub fn start_offset(&self, id: ClockId) -> f64 {
        self.start_offset_recursive(id, &mut std::collections::HashSet::new())
            .unwrap_or(0.0)
    }

    fn start_offset_recursive(
        &self,
        id: ClockId,
        seen: &mut std::collections::HashSet<ClockId>,
    ) -> Option<f64> {
        if !seen.insert(id) {
            return None;
        }
        let node = self.nodes.get(&id)?;
        let parent = match node.parent {
            Some(parent) => self.start_offset_recursive(parent, seen)?,
            None => 0.0,
        };
        Some(parent + node.local_offset)
    }

    /// **段前判定（S1）**：`global_t` 是否早于该节点所在段的起点。
    ///
    /// 语义：`local_offset` 是在**父级本地时间**里的偏移；根节点父级即全局时间。
    /// 只有带 `span` 的段节点才可能"尚未开始"；无 `span` 的常驻节点恒为 `false`。
    ///
    /// 为什么不在 `eval_local` 里改：`Hold` 的段前 clamp 是正确语义（段后冻结末值），
    /// 要改的是"绑定应用侧不要写"。本方法让应用侧可判定，而不动求值层。
    pub fn is_before_start(&self, id: ClockId, global_t: f64) -> bool {
        let Some(node) = self.nodes.get(&id) else {
            return false;
        };
        if node.span.is_none() {
            return false;
        }
        let parent_local = match node.parent {
            Some(parent) => self.eval_local(parent, global_t),
            None => global_t,
        };
        parent_local < node.local_offset
    }

    fn eval_local_recursive(
        &self,
        id: ClockId,
        global_t: f64,
        seen: &mut std::collections::HashSet<ClockId>,
    ) -> Option<f64> {
        if !seen.insert(id) {
            return None; // 环路保护
        }
        let node = self.nodes.get(&id)?;

        // 先得到父级本地时间（若 parent 存在，向上递归；否则 parent 本地时间 = 全局）
        let parent_local = match node.parent {
            Some(p) => self.eval_local_recursive(p, global_t, seen)?,
            None => global_t,
        };

        // 应用 offset + rate
        let raw = (parent_local - node.local_offset) * node.rate;

        // 应用 span + mode 映射
        let local = match (node.span, node.mode) {
            (None, _) => raw,
            (Some((_lo, _hi)), PlayMode::Step) => raw, // Step 模式下 span 无效果
            (Some((lo, hi)), PlayMode::Loop) => {
                let dur = (hi - lo).abs();
                if dur < 1e-9 {
                    raw
                } else {
                    let modded = ((raw - lo).rem_euclid(dur)) + lo;
                    modded
                }
            }
            (Some((lo, hi)), PlayMode::PingPong) => {
                let dur = (hi - lo).abs();
                if dur < 1e-9 {
                    raw
                } else {
                    // 三角波 tri(x): 0→1→0 over [0, 2*dur), then repeat
                    let x = (raw - lo).rem_euclid(dur * 2.0);
                    if x < dur {
                        lo + x
                    } else {
                        lo + (2.0 * dur - x)
                    }
                }
            }
            (Some((lo, hi)), PlayMode::Hold) => raw.clamp(lo.min(hi), lo.max(hi)),
        };

        Some(local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_parent_yields_global_t() {
        let mut forest = ClockForest::new();
        let _root = forest.insert(ClockNode::new(None)).unwrap();
        let child = forest.insert(ClockNode::new(Some(ClockId(0)))).unwrap();
        // child 的 parent = ClockId(0)（root），rate = 1, offset = 0
        let t = forest.eval_local(child, 12.5);
        assert!((t - 12.5).abs() < 1e-6);
    }

    #[test]
    fn nested_rate_offset_composes() {
        // root: rate=2, offset=0 → local = global * 2
        // child: parent=root, rate=0.5, offset=10 → local = (root_local - 10) * 0.5
        let mut forest = ClockForest::new();
        let mut root_node = ClockNode::new(None);
        root_node.rate = 2.0;
        let root = forest.insert(root_node).unwrap();

        let mut child_node = ClockNode::new(Some(root));
        child_node.rate = 0.5;
        child_node.local_offset = 10.0;
        let child = forest.insert(child_node).unwrap();

        let t = forest.eval_local(child, 20.0);
        // root_local = 20 * 2 = 40; child_local = (40 - 10) * 0.5 = 15
        assert!((t - 15.0).abs() < 1e-6);
    }

    #[test]
    fn loop_mode_wraps_continuously() {
        let mut forest = ClockForest::new();
        let mut node = ClockNode::new(None);
        node.span = Some((0.0, 1.0));
        node.mode = PlayMode::Loop;
        let id = forest.insert(node).unwrap();

        // 跨 3 个周期应该一致
        let a = forest.eval_local(id, 0.5);
        let b = forest.eval_local(id, 1.5);
        let c = forest.eval_local(id, 2.5);
        assert!((a - 0.5).abs() < 1e-6);
        assert!((b - 0.5).abs() < 1e-6);
        assert!((c - 0.5).abs() < 1e-6);

        // 周期边界连续
        let d = forest.eval_local(id, 0.999);
        let e = forest.eval_local(id, 1.001);
        assert!((d - 0.999).abs() < 1e-6);
        assert!((e - 0.001).abs() < 1e-6);
    }

    #[test]
    fn pingpong_mode_is_triangle_wave() {
        let mut forest = ClockForest::new();
        let mut node = ClockNode::new(None);
        node.span = Some((0.0, 1.0));
        node.mode = PlayMode::PingPong;
        let id = forest.insert(node).unwrap();

        // 上升段
        assert!((forest.eval_local(id, 0.25) - 0.25).abs() < 1e-6);
        // 反弹段（raw=1.25 → x=1.25, dur=1.0, x >= dur → result = 2-1.25 = 0.75）
        assert!((forest.eval_local(id, 1.25) - 0.75).abs() < 1e-6);
        // 反弹边界连续
        assert!((forest.eval_local(id, 0.99) - 0.99).abs() < 1e-6);
        assert!((forest.eval_local(id, 1.01) - 0.99).abs() < 1e-6);
    }

    #[test]
    fn hold_mode_clamps_to_span() {
        let mut forest = ClockForest::new();
        let mut node = ClockNode::new(None);
        node.span = Some((1.0, 3.0));
        node.mode = PlayMode::Hold;
        let id = forest.insert(node).unwrap();

        // span 外钳到边界，span 内原样通过
        assert!((forest.eval_local(id, 0.5) - 1.0).abs() < 1e-6);
        assert!((forest.eval_local(id, 2.0) - 2.0).abs() < 1e-6);
        assert!((forest.eval_local(id, 5.0) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn negative_rate_plays_backwards() {
        let mut forest = ClockForest::new();
        let mut node = ClockNode::new(None);
        node.rate = -1.0;
        let id = forest.insert(node).unwrap();

        assert!((forest.eval_local(id, 5.0) - (-5.0)).abs() < 1e-6);
        assert!((forest.eval_local(id, 10.0) - (-10.0)).abs() < 1e-6);
    }

    #[test]
    fn start_offset_and_before_start_for_timeline_nodes() {
        let mut forest = ClockForest::new();
        let mut first = ClockNode::new(None);
        first.span = Some((0.0, 1.0));
        first.mode = PlayMode::Hold;
        let first = forest.insert(first).unwrap();

        let mut second = ClockNode::new(None);
        second.local_offset = 2.5;
        second.span = Some((0.0, 1.0));
        second.mode = PlayMode::Hold;
        let second = forest.insert(second).unwrap();

        assert_eq!(forest.start_offset(first), 0.0);
        assert_eq!(forest.start_offset(second), 2.5);

        assert!(!forest.is_before_start(first, 0.0));
        assert!(forest.is_before_start(second, 2.499));
        assert!(!forest.is_before_start(second, 2.5));
        assert!(!forest.is_before_start(second, 9.0));
    }

    #[test]
    fn start_offset_sums_nested_offsets() {
        let mut forest = ClockForest::new();
        let mut root = ClockNode::new(None);
        root.local_offset = 1.5;
        let root = forest.insert(root).unwrap();
        let mut child = ClockNode::new(Some(root));
        child.local_offset = 2.0;
        let child = forest.insert(child).unwrap();

        assert_eq!(forest.start_offset(child), 3.5);
    }

    #[test]
    fn set_parent_rejects_cycle_and_preserves_forest() {
        // 构造 A→B（B.parent = A），再尝试 set_parent(A, Some(B)) 使 A→B→A 成环
        let mut forest = ClockForest::new();
        let a = forest.insert(ClockNode::new(None)).unwrap();
        let b = forest.insert(ClockNode::new(Some(a))).unwrap();

        assert_eq!(
            forest.set_parent(a, Some(b)),
            Err(TimeError::CycleDetected(a))
        );

        // 林结构未被破坏：A 仍为根，B 仍挂在 A 下，求值正常
        assert_eq!(forest.get(a).unwrap().parent, None);
        assert_eq!(forest.get(b).unwrap().parent, Some(a));
        assert!((forest.eval_local(b, 7.5) - 7.5).abs() < 1e-6);
    }

    #[test]
    fn set_parent_self_and_missing_are_rejected() {
        let mut forest = ClockForest::new();
        let a = forest.insert(ClockNode::new(None)).unwrap();

        // 自挂载成环
        assert_eq!(
            forest.set_parent(a, Some(a)),
            Err(TimeError::CycleDetected(a))
        );
        // 不存在的节点
        assert!(matches!(
            forest.set_parent(ClockId(999), None),
            Err(TimeError::ClockNotFound(_))
        ));
        assert!(matches!(
            forest.set_parent(a, Some(ClockId(999))),
            Err(TimeError::ClockNotFound(_))
        ));

        // 合法 re-parent 仍然可用
        let b = forest.insert(ClockNode::new(None)).unwrap();
        forest.set_parent(b, Some(a)).unwrap();
        assert_eq!(forest.get(b).unwrap().parent, Some(a));
        // 挂回根
        forest.set_parent(b, None).unwrap();
        assert_eq!(forest.get(b).unwrap().parent, None);
    }

    #[test]
    fn seek_determinism_global_t() {
        // 以任意 t 求值，结果只与 t 有关（无状态）
        let mut forest = ClockForest::new();
        let _root = forest.insert(ClockNode::new(None)).unwrap();

        let v1 = forest.eval_local(ClockId(0), 1.0);
        let v2 = forest.eval_local(ClockId(0), 5.0);
        let v3 = forest.eval_local(ClockId(0), 1.0);
        assert!(v1 == v3); // 同一 t → 同一结果
        assert!(v1 != v2); // 不同 t → 不同结果（无 rate/offset 时 t 直接传递）
    }

    #[test]
    fn serde_roundtrip_clock_forest() {
        let mut forest = ClockForest::new();
        let mut root = ClockNode::new(None);
        root.rate = 2.0;
        root.span = Some((0.0, 1.0));
        root.mode = PlayMode::Loop;
        forest.insert(root).unwrap();

        let json = serde_json::to_string(&forest).unwrap();
        let forest2: ClockForest = serde_json::from_str(&json).unwrap();

        // 两者在同一 t 的 eval 结果应完全相等
        for t in [0.0, 0.5, 1.5, 3.7, -2.0] {
            assert_eq!(
                forest.eval_local(ClockId(0), t),
                forest2.eval_local(ClockId(0), t)
            );
        }
    }
}
