// lifetime.rs — **生命周期事件**（S1）。
//
// # 设计
//
// - opt-in：只有挂 [`LifetimeTracked`] 的实体产生事件；
// - 发出点：`FrameEnd`（**在 `DeferredApplySystem` 之后**）做**集合差**，
//   而不是在 spawn 入队时埋点（否则接收方会看到尚未生效的 ID）；
// - 差集顺带吃掉同帧生灭的瞬态实体（不会产生 Spawn + Despawn 对）；
// - 事件顺序：按 `ObjectId` 显式排序后发出（容器遍历顺序不定 ⇒ 必须写死）；
// - 排序机制复用 pod 层的 `SeqKey` / `FlushBoundary` 语义（frame 用场景帧号）。
//
// # 为什么是差集而不是埋点
//
// 场景有两条创建路径：立即 `spawn_dyn` 与延迟 `DeferredCommands`。后者在
// `FrameEnd` 才生效。若在入队时发事件，接收方拿到的 ID 还不能被 `resolve`。
// 统一在生效之后做差集，两条路径自动归一。
//
// # Group 规则
//
// Group 只给**容器自身**打标记 ⇒ 子实体不产生事件。Group 被销毁时只发 1 条
// Despawn 事件；子实体按"隐式失效"处理（消费方不得持有未标记子实体的引用）。
// 这与当前场景的父子关系实现一致：`Parent`/`Children` 不强制级联删除。

use std::collections::BTreeSet;

use ze_pod::{FlushBatch, FlushBoundary, FlushBuffer, PodHandle, PodId, Sequenced, Sequencer};
use ze_scene_contract::{LifetimeTracked, ObjectId, SceneRead};
use zela_schedule::context::ExecutorContext;
use zela_schedule::system::System;

use crate::contract::object_id_of;

/// 场景内部生命周期事件的逻辑源（PodHandle 的 `PodId::next()` 从 1 开始，
/// 0 保留给"场景自身"）。
const LIFETIME_SOURCE: PodHandle = PodHandle::new(PodId::new(0));

/// 生命周期事件类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LifetimeEventKind {
    /// 对象已生效（本帧末仍在场景中，且上一帧末不存在）。
    Spawned,
    /// 对象已销毁（上一帧末存在，本帧末不存在）。
    Despawned,
}

/// 一条生命周期事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LifetimeEvent {
    pub kind: LifetimeEventKind,
    pub id: ObjectId,
}

impl LifetimeEvent {
    pub const fn new(kind: LifetimeEventKind, id: ObjectId) -> Self {
        Self { kind, id }
    }

    /// **消费侧强制校验**：事件目标是否仍指向活对象。
    ///
    /// - Spawn 事件在生效后消费 ⇒ 通常有效；
    /// - Despawn 事件在销毁后消费 ⇒ 必然失效（`None`）；
    /// - 槽位被复用 ⇒ `ObjectId::is_stale` 判定为失效。
    ///
    /// 返回当前场景里同 index 的活对象 ID（有效时），否则 `None`。
    pub fn resolve(&self, scene: &impl SceneRead) -> Option<ObjectId> {
        let mut current = None;
        scene.for_each::<LifetimeTracked>(|id, _| {
            if current.is_none() && id.index == self.id.index {
                current = Some(id);
            }
        });
        match current {
            // 同 index 同代 ⇒ 仍指向活对象；不同代 ⇒ 槽位已复用，事件失效。
            Some(current) if !self.id.is_stale(current) => Some(current),
            _ => None,
        }
    }

    /// 事件目标是否已失效（便捷包装，内部走 [`Self::resolve`]）。
    pub fn is_stale(&self, scene: &impl SceneRead) -> bool {
        self.resolve(scene).is_none()
    }
}

/// 生命周期事件批次（Resource）。
///
/// 每帧 `FrameEnd` 覆盖一次；`ordered` 已按 `SeqKey`（frame, source, seq）排序，
/// 而 `seq` 按 `ObjectId` 升序分配 ⇒ **事件序列按 ObjectId 升序**。
#[derive(Debug, Default)]
pub struct LifetimeEvents {
    last: Option<FlushBatch<LifetimeEvent>>,
}

impl LifetimeEvents {
    pub fn new() -> Self {
        Self::default()
    }

    /// 最近一帧的批次（含边界；无事件时也是 `Some` 空批次）。
    pub fn last(&self) -> Option<&FlushBatch<LifetimeEvent>> {
        self.last.as_ref()
    }

    /// 最近一帧的有序事件（无批次 ⇒ 空切片）。
    pub fn ordered(&self) -> &[Sequenced<LifetimeEvent>] {
        self.last
            .as_ref()
            .map(|batch| batch.ordered.as_slice())
            .unwrap_or(&[])
    }
}

/// 差集状态（Resource）：上一帧末的已追踪对象集合 + 场景帧号。
#[derive(Debug, Default)]
pub struct LifetimeTracker {
    previous: BTreeSet<ObjectId>,
    frame: u64,
}

impl LifetimeTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 已发出的场景帧数（第一帧为 1）。
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// 上一帧末的已追踪对象数。
    pub fn tracked_len(&self) -> usize {
        self.previous.len()
    }
}

/// **生命周期 flush**（`FrameEnd`，必须注册在 `DeferredApplySystem` **之后**）。
///
/// 求 `current - previous` 与 `previous - current`，按 `ObjectId` 升序发出一个批次。
pub struct LifetimeFlushSystem;

impl System for LifetimeFlushSystem {
    type Reads = ();
    type Writes = (LifetimeTracker, LifetimeEvents);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        // 当前帧末的已追踪集合（BTreeSet ⇒ 已按 ObjectId 排序）。
        // 先把 world 的只读借用收在块内，再取资源可变借用。
        let current: BTreeSet<ObjectId> = {
            let mut set = BTreeSet::new();
            for (entity, _) in ctx.world.query::<(hecs::Entity, &LifetimeTracked)>().iter() {
                set.insert(object_id_of(entity));
            }
            set
        };

        let Some(tracker) = ctx.get_mut::<LifetimeTracker>() else {
            return;
        };
        tracker.frame += 1;
        let frame = tracker.frame;

        // 取走上一帧集合；差集算完后写回当前集合（空集也必须写回）。
        let previous = std::mem::take(&mut tracker.previous);
        let mut ordered: Vec<LifetimeEvent> = Vec::new();
        // 两个集合各自有序 ⇒ 归并即可得到按 ObjectId 升序的事件序列。
        let mut a = current.difference(&previous).peekable();
        let mut b = previous.difference(&current).peekable();
        loop {
            match (a.peek(), b.peek()) {
                (Some(&spawned), Some(&despawned)) => {
                    if spawned <= despawned {
                        ordered.push(LifetimeEvent::new(LifetimeEventKind::Spawned, *spawned));
                        a.next();
                    } else {
                        ordered.push(LifetimeEvent::new(LifetimeEventKind::Despawned, *despawned));
                        b.next();
                    }
                }
                (Some(&spawned), None) => {
                    ordered.push(LifetimeEvent::new(LifetimeEventKind::Spawned, *spawned));
                    a.next();
                }
                (None, Some(&despawned)) => {
                    ordered.push(LifetimeEvent::new(LifetimeEventKind::Despawned, *despawned));
                    b.next();
                }
                (None, None) => break,
            }
        }

        // 复用 pod 层排序语义：SeqKey = (frame, source, seq)，seq 按事件顺序分配。
        let mut sequencer = Sequencer::new(LIFETIME_SOURCE);
        sequencer.begin_frame(frame);
        let mut buffer = FlushBuffer::new();
        for event in ordered {
            buffer.push(sequencer.next(event));
        }
        let batch = buffer.flush(FlushBoundary::FrameEnd { frame });

        if let Some(events) = ctx.get_mut::<LifetimeEvents>() {
            events.last = Some(batch);
        }
        if let Some(tracker) = ctx.get_mut::<LifetimeTracker>() {
            tracker.previous = current;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deferred::{DeferredApplySystem, DeferredCommands};
    use crate::scene::Scene as SceneStruct;
    use ze_object::Circle;
    use ze_scene_contract::Scene as _;
    use zela_schedule::resource::Resources;
    use zela_schedule::scheduler::{Phase, Scheduler};

    fn marked_circle(x: f32) -> Vec<Box<dyn ze_scene_contract::Component>> {
        vec![
            Box::new(Circle::new((x, 0.0), 1.0)),
            Box::new(LifetimeTracked),
        ]
    }

    fn unmarked_circle(x: f32) -> Vec<Box<dyn ze_scene_contract::Component>> {
        vec![Box::new(Circle::new((x, 0.0), 1.0))]
    }

    fn scheduler(world: hecs::World) -> Scheduler {
        let mut resources = Resources::new();
        resources.insert(LifetimeTracker::new());
        resources.insert(LifetimeEvents::new());
        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::FrameEnd, DeferredApplySystem);
        sched.add_system(Phase::FrameEnd, LifetimeFlushSystem);
        sched.init_resources();
        sched
    }

    fn events(sched: &Scheduler) -> Vec<LifetimeEvent> {
        sched
            .resources()
            .get::<LifetimeEvents>()
            .unwrap()
            .ordered()
            .iter()
            .map(|item| *item.value())
            .collect()
    }

    /// 1. 未打标记的实体不产生任何事件（opt-in 零成本）。
    #[test]
    fn unmarked_entities_emit_no_events() {
        let mut scene = SceneStruct::new();
        for i in 0..5 {
            scene.spawn_dyn(unmarked_circle(i as f32)).unwrap();
        }
        let mut sched = scheduler(scene.into_world());
        sched.run_frame(0.016);
        assert!(events(&sched).is_empty(), "未打标记 ⇒ 无事件");
    }

    /// 2. Group 打标记 ⇒ 刷新时恰好 1 条事件，不是 39 条。
    #[test]
    fn marked_group_emits_exactly_one_event() {
        let mut scene = SceneStruct::new();
        let group = scene.spawn_dyn(marked_circle(0.0)).unwrap();
        for i in 0..39 {
            scene.spawn_dyn(unmarked_circle(i as f32 + 1.0)).unwrap();
        }
        let mut sched = scheduler(scene.into_world());

        sched.run_frame(0.016);
        let first = events(&sched);
        eprintln!("[S1] marked group events = {}", first.len());
        assert_eq!(
            first,
            vec![LifetimeEvent::new(LifetimeEventKind::Spawned, group)],
            "Group 自身 1 条，39 个子实体不产生事件"
        );

        // 模拟 refresh：新增 39 个子实体（仍未打标记）⇒ 不产生新事件。
        {
            let mut ws = crate::contract::WorldScene::new(&mut *sched.world_mut());
            for i in 0..39 {
                ws.spawn_dyn(unmarked_circle(i as f32 + 100.0)).unwrap();
            }
        }
        sched.run_frame(0.016);
        assert!(events(&sched).is_empty(), "子实体 churn 不产生事件");
    }

    /// 3. 瞬态实体（同帧生灭）不产生事件对。
    #[test]
    fn transient_entity_produces_no_event_pair() {
        let mut scene = SceneStruct::new();
        let id = scene.spawn_dyn(marked_circle(0.0)).unwrap();
        scene.despawn(id);
        let mut sched = scheduler(scene.into_world());
        sched.run_frame(0.016);
        assert!(events(&sched).is_empty(), "同帧生灭 ⇒ 无 Spawn/Despawn 对");
    }

    /// 4. 事件序列按 `ObjectId` 升序（打乱插入顺序后仍成立）。
    #[test]
    fn events_are_ordered_by_object_id() {
        let mut scene = SceneStruct::new();
        let mut ids = Vec::new();
        for i in [3.0, 1.0, 4.0, 0.0, 2.0] {
            ids.push(scene.spawn_dyn(marked_circle(i)).unwrap());
        }
        ids.sort();
        let mut sched = scheduler(scene.into_world());
        sched.run_frame(0.016);
        let got: Vec<ObjectId> = events(&sched).into_iter().map(|e| e.id).collect();
        eprintln!("[S1] ordered ids = {got:?}");
        assert_eq!(got, ids, "事件必须按 ObjectId 升序");
    }

    /// 5. 延迟 spawn 的对象，事件在其**生效之后**才发出。
    #[test]
    fn deferred_spawn_event_is_emitted_after_effect() {
        let mut sched = scheduler(hecs::World::new());
        sched
            .resources_mut()
            .get_mut::<DeferredCommands>()
            .unwrap()
            .spawn(marked_circle(0.0));

        sched.run_frame(0.016);
        let got = events(&sched);
        assert_eq!(got.len(), 1, "延迟 spawn 的生效帧末应发出事件");
        assert_eq!(got[0].kind, LifetimeEventKind::Spawned);
        let world = crate::contract::WorldView::new(sched.world());
        assert!(got[0].resolve(&world).is_some(), "事件发出时对象必须已存在");
    }

    /// 6. 消费已销毁对象 ⇒ `is_stale` 判定为 true。
    #[test]
    fn despawned_event_is_stale() {
        let mut scene = SceneStruct::new();
        let id = scene.spawn_dyn(marked_circle(0.0)).unwrap();
        let mut sched = scheduler(scene.into_world());
        sched.run_frame(0.016);
        assert_eq!(events(&sched)[0].kind, LifetimeEventKind::Spawned);

        sched
            .resources_mut()
            .get_mut::<DeferredCommands>()
            .unwrap()
            .despawn(id);
        sched.run_frame(0.016);

        let got = events(&sched);
        assert_eq!(got[0].kind, LifetimeEventKind::Despawned);
        let world = crate::contract::WorldView::new(sched.world());
        assert!(got[0].is_stale(&world), "销毁后的 id 必须判定为失效");
        assert!(got[0].resolve(&world).is_none());
    }

    /// 7. Group 销毁 ⇒ 1 条事件；未标记子实体不产生事件（隐式失效规则）。
    #[test]
    fn group_despawn_emits_single_event() {
        let mut scene = SceneStruct::new();
        let group = scene.spawn_dyn(marked_circle(0.0)).unwrap();
        for i in 0..3 {
            scene.spawn_dyn(unmarked_circle(i as f32 + 1.0)).unwrap();
        }
        let mut sched = scheduler(scene.into_world());
        sched.run_frame(0.016);
        assert_eq!(events(&sched).len(), 1);

        sched
            .resources_mut()
            .get_mut::<DeferredCommands>()
            .unwrap()
            .despawn(group);
        sched.run_frame(0.016);
        let got = events(&sched);
        assert_eq!(
            got,
            vec![LifetimeEvent::new(LifetimeEventKind::Despawned, group)],
            "Group 销毁只发 1 条；子实体不产生事件（隐式失效）"
        );
    }

    /// 8. 跑两遍，事件序列逐条相同（确定性）。
    #[test]
    fn event_sequence_is_deterministic() {
        fn run() -> Vec<LifetimeEvent> {
            let mut scene = SceneStruct::new();
            for i in [2.0, 0.0, 3.0, 1.0] {
                scene.spawn_dyn(marked_circle(i)).unwrap();
            }
            let mut sched = scheduler(scene.into_world());
            sched.run_frame(0.016);
            sched.run_frame(0.016);
            events(&sched)
        }
        assert_eq!(run(), run(), "两遍事件序列必须逐条相同");
    }
}
