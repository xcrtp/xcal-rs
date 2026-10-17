// timeline.rs — 顺序编排（P3）：把"先…再…"编译成 `ClockNode + Binding`。
//
// # 语义（§6.1）
//
// - `play(curve, target, dur)` —— 声明一段动画：在**顺序游标**处开始、时长 `dur`；
// - `parallel(curve, target)` —— 与**上一段** `play` **并行**的额外动作（同区间）；
// - `wait(d)` —— **不产生节点**，只推进游标；
// - 顺序游标累积，**自动算出**每个动画的起始时刻（不是手写 offset）。
//
// # 编译产物（§6.2，已按 `clock.rs` 源码公式逐行复现 `eval_local` 实测）
//
// ```text
// ClockNode { local_offset: 游标, span: Some((0.0, dur)), mode: PlayMode::Hold }
// ```
//
// 裸 offset（无 span/Hold）下等待期是**负时间**：`Sin`/`Phase`/`Orbit` 直接求值
// ⇒ 画面**倒着预演**；**`offset + span + Hold` 三件套**精确复现一段 `play` 的完整语义
// （延迟启动 + 两端冻结）。
//
// # 关键原则（§6.3）
//
// **求值层零改动** —— `ClockForest` / `eval_local` 一行都不动；本模块只决定
// "谁来填这些数字"。
//
// 它**不是**给 `ClockNode` 加 `wait_before` 字段 —— 那会得到"每个节点各自声明等多久"，
// 仍表达不了"A 在 B 之后"。

use std::sync::Arc;

use xc_scene_contract::ObjectId;
use xc_time::{ClockForest, ClockNode, Curve, PlayMode};

use crate::animation::binding::BindingTable;
use crate::animation::target::Target;

/// 一段已声明动画在**全局时间轴**上的区间（秒）。
///
/// 它是**算出来的**（`schedule()` 每次重算），不是声明时存下来的 ——
/// 因此改掉前一段的时长，后一段的 `start` 自动跟随。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub duration: f64,
}

impl Segment {
    /// 区间右端（`start + duration`）。
    pub fn end(&self) -> f64 {
        self.start + self.duration
    }
}

/// 段内一个动作的目标：**已解析** / **声明期待解析**（P6 场景脚本）。
///
/// `Ready` 是手写 `play` / `parallel` 走的那条（目标已构造好）；
/// `Pending` 是场景脚本（`SceneEncoder::animate`）走的那条 —— 声明期还没有
/// `ObjectId`，只记一个 `Handle` 下标；`commit` 时经 [`Timeline::compile_with`]
/// 的 `resolve` 求出真实 id，再调 `make(id)` 生成 target。
#[derive(Clone)]
enum ActionTarget {
    Ready(Arc<dyn Target>),
    /// `make` 是 `TargetCtor::make` 的**函数指针**（不捕获任何环境 ⇒ 天然 `Send + Sync`）；
    /// `handle` 是**本段脚本内**的句柄下标（`append` 时整体偏移）。
    Pending {
        make: fn(ObjectId) -> Arc<dyn Target>,
        handle: usize,
    },
}

/// 段内的一个动作：一条 `曲线 → 目标` 绑定。
#[derive(Clone)]
struct Action {
    curve: Curve,
    target: ActionTarget,
}

/// 编排步骤：`Wait` 只推进游标；`Play` 产出一个时间区间（区间内可含多个**并行**动作）。
#[derive(Clone)]
enum Step {
    Wait(f64),
    Play {
        duration: f64,
        actions: Vec<Action>,
    },
}

/// 顺序编排器：声明"先…再…"，编译成 `ClockNode + Binding`。
///
/// 目标/曲线以 `Arc`（曲线 `Clone`）持有，`compile` 可反复调用（不同森林/绑定表）。
pub struct Timeline {
    steps: Vec<Step>,
}

impl Default for Timeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Timeline {
    pub fn new() -> Self {
        Self { steps: Vec::new() }
    }

    /// 推进游标 `d` 秒（**不产生节点**）。
    pub fn wait(&mut self, d: f64) -> &mut Self {
        self.steps.push(Step::Wait(d));
        self
    }

    /// 声明一段动画：在游标处开始、时长 `dur`，随后游标前进 `dur`。
    ///
    /// `curve` 的定义域是**段内局部时间** `[0, dur]`：编译产物用 `Hold` 把段外
    /// 时间钳到两端，故曲线在段外不会被求值到。
    ///
    /// 段内还可以用 [`Timeline::parallel`] 追加与它**同区间**的其它动作
    /// （如"镜头推进"同时改 `scale` 与 `center`）。
    pub fn play(&mut self, curve: Curve, target: impl Target, dur: f64) -> &mut Self {
        self.play_action(curve, ActionTarget::Ready(Arc::new(target)), dur)
    }

    /// 声明一段动画，但目标**尚未解析**（场景脚本：`Handle` → `commit` 时求 id）。
    ///
    /// `make` 是 `TargetCtor::make` 的函数指针；`handle` 是脚本内句柄下标。
    /// 语义与 [`Timeline::play`] 逐位相同，差别只在"目标何时变成具体 trait object"。
    pub fn play_pending(
        &mut self,
        curve: Curve,
        make: fn(ObjectId) -> Arc<dyn Target>,
        handle: usize,
        dur: f64,
    ) -> &mut Self {
        self.play_action(curve, ActionTarget::Pending { make, handle }, dur)
    }

    fn play_action(&mut self, curve: Curve, target: ActionTarget, dur: f64) -> &mut Self {
        self.steps.push(Step::Play {
            duration: dur,
            actions: vec![Action { curve, target }],
        });
        self
    }

    /// 向**上一段** `play` 追加一个**同区间**动作（同一 `ClockNode`，同一 `span`）。
    ///
    /// 上一段不是 `play`（无可依附的区间）时本调用**不产生任何效果** ——
    /// 调用方须紧跟在一个 `play` 之后使用。
    pub fn parallel(&mut self, curve: Curve, target: impl Target) -> &mut Self {
        self.parallel_action(curve, ActionTarget::Ready(Arc::new(target)))
    }

    /// [`Timeline::parallel`] 的"待解析目标"版本（场景脚本用）。
    pub fn parallel_pending(
        &mut self,
        curve: Curve,
        make: fn(ObjectId) -> Arc<dyn Target>,
        handle: usize,
    ) -> &mut Self {
        self.parallel_action(curve, ActionTarget::Pending { make, handle })
    }

    fn parallel_action(&mut self, curve: Curve, target: ActionTarget) -> &mut Self {
        if let Some(Step::Play { actions, .. }) = self.steps.last_mut() {
            actions.push(Action { curve, target });
        }
        self
    }

    /// 每段 `play` 的时间区间（**由步骤顺序算出**；并行动作同属一个区间）。
    pub fn schedule(&self) -> Vec<Segment> {
        let mut cursor = 0.0f64;
        let mut out = Vec::new();
        for step in &self.steps {
            match step {
                Step::Wait(d) => cursor += d,
                Step::Play { duration, .. } => {
                    out.push(Segment {
                        start: cursor,
                        duration: *duration,
                    });
                    cursor += duration;
                }
            }
        }
        out
    }

    /// 改写第 `play_index` 段（0 基，**只数 `play`**，不含 `wait`）的时长。
    ///
    /// 越界则无操作。改完后 `schedule()` 里后续段的 `start` 自动跟随。
    pub fn set_duration(&mut self, play_index: usize, dur: f64) -> &mut Self {
        let mut seen = 0usize;
        for step in &mut self.steps {
            if let Step::Play { duration, .. } = step {
                if seen == play_index {
                    *duration = dur;
                    break;
                }
                seen += 1;
            }
        }
        self
    }

    /// 编译进时钟树 + 绑定表：每段 `play` 一个 `Hold` 节点；段内的并行动作
    /// **共享同一节点**（同一 `span` ⇒ 精确同区间）。
    ///
    /// `ClockNode::insert` 不可能失败（新 id 不在任何已有祖先链上，见 `clock.rs`），
    /// 失败（理论不可达）即跳过该段。
    ///
    /// 全部动作必须**已解析**（手写 `play` / `parallel`）；若含待解析动作，其目标按
    /// `DANGLING` 构造（表现为"静默不命中"）—— 场景脚本请走 [`Timeline::compile_with`]。
    pub fn compile(&self, forest: &mut ClockForest, bindings: &mut BindingTable) {
        self.compile_with(&|_| ObjectId::DANGLING, forest, bindings);
    }

    /// [`Timeline::compile`] 的"带句柄解析"版本：待解析动作经 `resolve(handle)` 求
    /// 真实 `ObjectId`，再 `make(id)` 生成 target（场景脚本 `commit` 的落点）。
    pub fn compile_with(
        &self,
        resolve: &impl Fn(usize) -> ObjectId,
        forest: &mut ClockForest,
        bindings: &mut BindingTable,
    ) {
        let mut cursor = 0.0f64;
        for step in &self.steps {
            match step {
                Step::Wait(d) => cursor += d,
                Step::Play { duration, actions } => {
                    let mut node = ClockNode::new(None);
                    node.local_offset = cursor;
                    node.span = Some((0.0, *duration));
                    node.mode = PlayMode::Hold;
                    if let Ok(id) = forest.insert(node) {
                        for action in actions {
                            let target: Arc<dyn Target> = match &action.target {
                                ActionTarget::Ready(t) => Arc::clone(t),
                                ActionTarget::Pending { make, handle } => make(resolve(*handle)),
                            };
                            bindings.bind_arc(id, action.curve.clone(), target);
                        }
                    }
                    cursor += duration;
                }
            }
        }
    }

    /// 编排总时长（所有 `wait` + 所有段时长之和）—— 合并（append）的时间游标依据。
    pub fn total_duration(&self) -> f64 {
        let mut cursor = 0.0f64;
        for step in &self.steps {
            match step {
                Step::Wait(d) => cursor += d,
                Step::Play { duration, .. } => cursor += duration,
            }
        }
        cursor
    }

    /// 把 `other` 的步骤**整体追加**到本编排之后（合并语义，§3.3）。
    ///
    /// - **时间游标**：`other` 的步骤直接续在本编排末尾 ⇒ `other` 的首段起点
    ///   自动落到本编排的 [`Timeline::total_duration`] 之后；
    /// - **句柄偏移**：`other` 的待解析动作里 `handle` 整体 `+ base`（`base` = 本编排
    ///   已占用的段内句柄数）。
    pub fn append_shifted(&mut self, other: &Timeline, base: usize) {
        for step in &other.steps {
            match step {
                Step::Wait(d) => self.steps.push(Step::Wait(*d)),
                Step::Play { duration, actions } => {
                    let actions = actions
                        .iter()
                        .map(|a| Action {
                            curve: a.curve.clone(),
                            target: match &a.target {
                                ActionTarget::Ready(t) => ActionTarget::Ready(Arc::clone(t)),
                                ActionTarget::Pending { make, handle } => ActionTarget::Pending {
                                    make: *make,
                                    handle: handle + base,
                                },
                            },
                        })
                        .collect();
                    self.steps.push(Step::Play {
                        duration: *duration,
                        actions,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::target::{CircleRadius, Phase as PhaseTarget};
    use xc_scene_contract::ObjectId;
    use xc_time::AnimValue;

    fn target() -> CircleRadius {
        CircleRadius {
            id: ObjectId::DANGLING,
        }
    }

    /// 三段编排：`play 2.0` / `wait 0.5` / `play 1.0` ⇒ 起始 0.0 与 2.5。
    fn three_segments() -> Timeline {
        let mut tl = Timeline::new();
        tl.play(Curve::Phase { omega: 0.5 }, target(), 2.0);
        tl.wait(0.5);
        tl.play(Curve::Phase { omega: 1.0 }, PhaseTarget { id: ObjectId::DANGLING }, 1.0);
        tl
    }

    /// **P3 验收（§6.4.1）**：把 A 的时长改掉后，B 的 offset **自动**跟着变
    /// —— 证明顺序是**算出来的**，不是手写的。
    #[test]
    fn later_segments_follow_earlier_duration_change() {
        let mut tl = three_segments();
        assert_eq!(
            tl.schedule(),
            vec![
                Segment {
                    start: 0.0,
                    duration: 2.0
                },
                Segment {
                    start: 2.5,
                    duration: 1.0
                },
            ]
        );

        // A 2.0 → 3.0：B 的起点 2.5 → 3.5（无需改任何 offset）。
        tl.set_duration(0, 3.0);
        assert_eq!(
            tl.schedule(),
            vec![
                Segment {
                    start: 0.0,
                    duration: 3.0
                },
                Segment {
                    start: 3.5,
                    duration: 1.0
                },
            ]
        );
        assert_eq!(tl.schedule()[1].end(), 4.5);
    }

    /// 编译产物：每段 `play` 一个 `Hold` 节点 + 一条绑定；`wait` 不产生节点。
    #[test]
    fn compile_emits_one_hold_node_per_play() {
        let tl = three_segments();
        let mut forest = ClockForest::new();
        let mut bindings = BindingTable::new();
        tl.compile(&mut forest, &mut bindings);

        assert_eq!(bindings.bindings.len(), 2, "两段 play ⇒ 两条绑定（wait 不产生）");

        // 第一段：offset 0、span (0,2)、Hold —— 段外钳两端。
        let n0 = bindings.bindings[0].clock;
        let node0 = forest.get(n0).unwrap();
        assert_eq!(node0.local_offset, 0.0);
        assert_eq!(node0.span, Some((0.0, 2.0)));
        assert_eq!(node0.mode, PlayMode::Hold);

        // 第二段：offset = 2.0 + 0.5（wait）= 2.5。
        let n1 = bindings.bindings[1].clock;
        let node1 = forest.get(n1).unwrap();
        assert_eq!(node1.local_offset, 2.5);
        assert_eq!(node1.span, Some((0.0, 1.0)));
    }

    /// **等待期不是负时间**：段前的局部时间钳到 `0`（初态冻结），段后钳到 `dur`
    /// （终态冻结），段内线性推进 —— 这正是 `Hold` + `span` 的效果。
    #[test]
    fn eval_local_freezes_before_and_after_each_segment() {
        let tl = three_segments();
        let mut forest = ClockForest::new();
        let mut bindings = BindingTable::new();
        tl.compile(&mut forest, &mut bindings);

        let n0 = bindings.bindings[0].clock;
        assert_eq!(forest.eval_local(n0, -1.0), 0.0, "段前钳初态");
        assert_eq!(forest.eval_local(n0, 1.0), 1.0, "段内线性");
        assert_eq!(forest.eval_local(n0, 100.0), 2.0, "段后钳终态");

        let n1 = bindings.bindings[1].clock;
        assert_eq!(forest.eval_local(n1, 0.0), 0.0, "第二段尚未开始 ⇒ 0，不是负时间");
        assert!((forest.eval_local(n1, 3.0) - 0.5).abs() < 1e-9);
        assert_eq!(forest.eval_local(n1, 9.0), 1.0);
    }

    /// 曲线在段内按局部时间求值：`Phase{omega:1/dur}` ⇒ `t` 从 0 走到 1。
    #[test]
    fn curve_samples_over_local_time_within_segment() {
        let mut tl = Timeline::new();
        tl.play(Curve::Phase { omega: 0.5 }, target(), 2.0);

        let mut forest = ClockForest::new();
        let mut bindings = BindingTable::new();
        tl.compile(&mut forest, &mut bindings);

        let b = &bindings.bindings[0];
        let sample = |t: f64| b.curve.sample(forest.eval_local(b.clock, t)).scalar();
        assert_eq!(sample(0.0), 0.0);
        assert!((sample(1.0) - 0.5).abs() < 1e-6);
        assert_eq!(sample(2.0), 1.0);
        // 段后仍钳终值 1.0（Hold），不会继续增长。
        assert_eq!(sample(5.0), 1.0);
        // 与 AnimValue 类型无关：本例是 Scalar。
        assert!(matches!(b.curve.sample(0.0), AnimValue::Scalar(_)));
    }

    /// `parallel`：同一区间内多个动作——共享**同一个** `ClockNode`（同一 span），
    /// 且**只推进一次游标**（后续段起点不受影响）。
    #[test]
    fn parallel_actions_share_one_interval_without_advancing_cursor() {
        let mut tl = Timeline::new();
        tl.play(Curve::Phase { omega: 1.0 }, target(), 1.0)
            .parallel(Curve::Phase { omega: 1.0 }, PhaseTarget { id: ObjectId::DANGLING })
            .parallel(Curve::Phase { omega: 1.0 }, target());
        tl.wait(0.5);
        tl.play(Curve::Phase { omega: 1.0 }, target(), 1.0);

        // 两个区间：0–1 与 1.5–2.5（中段并行动作不额外占时间）。
        assert_eq!(
            tl.schedule(),
            vec![
                Segment {
                    start: 0.0,
                    duration: 1.0
                },
                Segment {
                    start: 1.5,
                    duration: 1.0
                },
            ]
        );

        let mut forest = ClockForest::new();
        let mut bindings = BindingTable::new();
        tl.compile(&mut forest, &mut bindings);
        // 3 个并行动作 + 1 个后续动作 = 4 条绑定、2 个节点。
        assert_eq!(bindings.bindings.len(), 4);
        let a = bindings.bindings[0].clock;
        let b = bindings.bindings[1].clock;
        let c = bindings.bindings[2].clock;
        let d = bindings.bindings[3].clock;
        assert_eq!(a, b);
        assert_eq!(b, c, "并行动作共享同一节点");
        assert_ne!(c, d, "后续段用新节点");
    }
}
