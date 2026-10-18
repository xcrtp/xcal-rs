// composer.rs — **合成器对象化**（S3）。
//
// # 定位
//
// `Composer` 是**行为对象**：它本身不进 DrawQueue，也不是每帧遍历的节点；
// 它通过 S1 的生命周期事件**增量维护**自己的派生态（[`ComposerQueue`]）。
//
// ```text
// LifetimeEvents（S1） ──增量──▶ ComposerQueue（派生态）
//                                   ▲
// 全量重算（registration / 对拍）────┘
// ```
//
// # 为什么派生态不是场景状态
//
// `ComposerQueue` 由事件维护，事件不修改 `Composer` 配置本身（红线 §0.3）。
// `Composer` 的配置（layer / blend / opacity）是**场景状态**，只能经
// `Scene::set`（命令路径）修改；`ComposerSystem` 的 `Writes` 不含 `Composer`，
// 因而事件消费者在类型层面就无法改配置。
//
// # 初始回放
//
// 后注册的合成器收不到注册前的事件 ⇒ 首次看到 `Composer` 时做一次**全量快照**
// （`full_recompute`）。这与 `ReplaySystem` 的"按台账清账"是同一类边界处理：
// 注册/重播时用全量，稳态用增量。
//
// # 与最终 DrawQueue 的关系
//
// 当前 `DrawQueue` 是**屏幕空间**且随相机变化（分镜相机每帧在动），必须每帧重录。
// 本轮合成器维护的是**相机无关**的内容索引（`ComposerQueue`）：增量结果与全量
// 重算可逐项对拍。待相机无关批处理落地后，它就是 DrawQueue 的增量来源。

use std::collections::BTreeSet;

use xc_render_ir::{LayerBlend, LayerId};
use xc_scene_contract::{LifetimeTracked, ObjectId, SceneRead};
use xcal_schedule::context::ExecutorContext;
use xcal_schedule::system::System;

use crate::contract::WorldView;
use crate::lifetime::{LifetimeEvent, LifetimeEventKind, LifetimeEvents};

/// 合成器配置（**场景状态**；只能经 `Scene::set` / 命令路径修改）。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Composer {
    /// 目标图层。
    pub layer: LayerId,
    /// 与下层的混合方式。
    pub blend: LayerBlend,
    /// 图层不透明度（[0,1]，消费前钳制）。
    pub opacity: f32,
}

impl Default for Composer {
    fn default() -> Self {
        Self {
            layer: LayerId(0),
            blend: LayerBlend::Alpha,
            opacity: 1.0,
        }
    }
}

impl Composer {
    pub fn new(layer: LayerId, blend: LayerBlend, opacity: f32) -> Self {
        Self {
            layer,
            blend,
            opacity,
        }
    }

    /// 钳制后的不透明度（合成唯一取值口径）。
    pub fn clamped_opacity(&self) -> f32 {
        self.opacity.clamp(0.0, 1.0)
    }
}

/// 合成器的**派生态**：相机无关的内容索引（Resource）。
///
/// 稳态由生命周期事件增量维护；注册时由 [`full_recompute`] 全量填充。
/// `items` 是 `BTreeSet` ⇒ 天然按 `ObjectId` 排序，输出确定。
#[derive(Debug, Default)]
pub struct ComposerQueue {
    initialized: bool,
    items: BTreeSet<ObjectId>,
}

impl ComposerQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否已完成初始回放。
    pub fn initialized(&self) -> bool {
        self.initialized
    }

    /// 当前内容项（按 `ObjectId` 升序）。
    pub fn items(&self) -> Vec<ObjectId> {
        self.items.iter().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn contains(&self, id: ObjectId) -> bool {
        self.items.contains(&id)
    }
}

/// **全量重算**：当前场景里全部已追踪对象（排序后）。
///
/// 保留这条路径用于初始回放与"增量 == 全量"对拍。
pub fn full_recompute(scene: &impl SceneRead) -> Vec<ObjectId> {
    let mut ids = BTreeSet::new();
    scene.for_each::<LifetimeTracked>(|id, _| {
        ids.insert(id);
    });
    ids.into_iter().collect()
}

/// **合成器事件消费**（`FrameEnd`，注册在 `LifetimeFlushSystem` **之后**）。
pub struct ComposerSystem;

impl System for ComposerSystem {
    type Reads = (LifetimeEvents,);
    type Writes = (ComposerQueue,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        // 行为对象是否在场（位图查询，成本可忽略；不做每帧全量遍历）。
        let has_composer = ctx.world.query::<&Composer>().iter().next().is_some();

        let initialized = ctx
            .get::<ComposerQueue>()
            .map(|queue| queue.initialized)
            .unwrap_or(false);

        if !has_composer {
            // 合成器被销毁 ⇒ 派生态失效；下一次注册重新做全量回放。
            if initialized {
                if let Some(queue) = ctx.get_mut::<ComposerQueue>() {
                    queue.initialized = false;
                    queue.items.clear();
                }
            }
            return;
        }

        // 首次看到合成器：全量快照（后注册也能收到注册前的对象）。
        if !initialized {
            let snapshot = full_recompute(&WorldView::new(ctx.world));
            if let Some(queue) = ctx.get_mut::<ComposerQueue>() {
                queue.items = snapshot.into_iter().collect();
                queue.initialized = true;
            }
            return;
        }

        // 稳态：先取事件并按消费侧规则解析（world 只读借用收在块内），再写派生态。
        let events: Vec<LifetimeEvent> = ctx
            .get::<LifetimeEvents>()
            .map(|store| store.ordered().iter().map(|item| *item.value()).collect())
            .unwrap_or_default();

        let resolved: Vec<(LifetimeEventKind, ObjectId)> = {
            let view = WorldView::new(ctx.world);
            events
                .iter()
                .filter_map(|event| match event.kind {
                    // Spawn：必须通过 `resolve`（内部 `ObjectId::is_stale`）校验。
                    LifetimeEventKind::Spawned => event
                        .resolve(&view)
                        .map(|id| (LifetimeEventKind::Spawned, id)),
                    // Despawn：对象已不在，按事件 id 删除即可（不做悬垂解引用）。
                    LifetimeEventKind::Despawned => Some((LifetimeEventKind::Despawned, event.id)),
                })
                .collect()
        };

        if let Some(queue) = ctx.get_mut::<ComposerQueue>() {
            for (kind, id) in resolved {
                match kind {
                    LifetimeEventKind::Spawned => {
                        queue.items.insert(id);
                    }
                    LifetimeEventKind::Despawned => {
                        queue.items.remove(&id);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deferred::{DeferredApplySystem, DeferredCommands};
    use crate::lifetime::{LifetimeFlushSystem, LifetimeTracker};
    use crate::scene::Scene as SceneStruct;
    use xc_object::Circle;
    use xc_scene_contract::Scene as _;
    use xcal_schedule::resource::Resources;
    use xcal_schedule::scheduler::{Phase, Scheduler};

    use crate::lifetime::LifetimeEvents as Events;

    fn tracked_circle(x: f32) -> Vec<Box<dyn xc_scene_contract::Component>> {
        vec![
            Box::new(Circle::new((x, 0.0), 1.0)),
            Box::new(LifetimeTracked),
        ]
    }

    fn scheduler(world: hecs::World) -> Scheduler {
        let mut resources = Resources::new();
        resources.insert(LifetimeTracker::new());
        resources.insert(Events::new());
        resources.insert(ComposerQueue::new());
        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::Prepare, ComposerSystem);
        sched.add_system(Phase::FrameEnd, DeferredApplySystem);
        sched.add_system(Phase::FrameEnd, LifetimeFlushSystem);
        sched.init_resources();
        sched
    }

    fn queue(sched: &Scheduler) -> Vec<ObjectId> {
        sched.resources().get::<ComposerQueue>().unwrap().items()
    }

    fn full(sched: &Scheduler) -> Vec<ObjectId> {
        full_recompute(&WorldView::new(sched.world()))
    }

    /// 1. 后注册合成器能收到注册前已存在对象的初始回放。
    #[test]
    fn composer_registered_late_receives_initial_snapshot() {
        let mut scene = SceneStruct::new();
        let a = scene.spawn_dyn(tracked_circle(0.0)).unwrap();
        let b = scene.spawn_dyn(tracked_circle(1.0)).unwrap();
        let mut sched = scheduler(scene.into_world());

        // 两帧没有 Composer ⇒ 合成器未初始化。
        sched.run_frame(0.016);
        sched.run_frame(0.016);
        assert!(queue(&sched).is_empty(), "无 Composer ⇒ 派生态为空");
        assert!(
            !sched
                .resources()
                .get::<ComposerQueue>()
                .unwrap()
                .initialized()
        );

        // 后注册：下一帧必须全量回放（注册前已存在的 a/b 都要在）。
        sched.world_mut().spawn((Composer::default(),));
        sched.run_frame(0.016);
        eprintln!("[S3] initial snapshot = {:?}", queue(&sched));
        assert_eq!(
            queue(&sched),
            full(&sched),
            "初始回放必须覆盖全部已追踪对象"
        );
        assert_eq!(queue(&sched), vec![a, b]);
    }

    /// 2. 增量结果 == 全量重算结果（对拍）。
    #[test]
    fn incremental_matches_full_recompute() {
        let mut scene = SceneStruct::new();
        for i in 0..4 {
            scene.spawn_dyn(tracked_circle(i as f32)).unwrap();
        }
        scene
            .spawn_dyn(vec![Box::new(Composer::default())])
            .unwrap();
        let mut sched = scheduler(scene.into_world());

        sched.run_frame(0.016);
        assert_eq!(queue(&sched), full(&sched));

        // 增：延迟创建 2 个。
        {
            let commands = sched.resources_mut().get_mut::<DeferredCommands>().unwrap();
            commands.spawn(tracked_circle(10.0));
            commands.spawn(tracked_circle(11.0));
        }
        sched.run_frame(0.016);
        sched.run_frame(0.016);
        assert_eq!(queue(&sched), full(&sched), "增量 spawn 后必须与全量一致");

        // 减：延迟销毁第一个。
        let victim = queue(&sched)[0];
        sched
            .resources_mut()
            .get_mut::<DeferredCommands>()
            .unwrap()
            .despawn(victim);
        // 事件在帧末发出，合成器在下一帧 Prepare 消费 ⇒ 跑两帧。
        sched.run_frame(0.016);
        sched.run_frame(0.016);
        eprintln!(
            "[S3] incremental items = {}, full = {}",
            queue(&sched).len(),
            full(&sched).len()
        );
        assert_eq!(queue(&sched), full(&sched), "增量 despawn 后必须与全量一致");
    }

    /// 3. Spawn 一个内容对象 ⇒ 派生态增加对应项。
    #[test]
    fn spawn_adds_item_incrementally() {
        let mut scene = SceneStruct::new();
        scene
            .spawn_dyn(vec![Box::new(Composer::default())])
            .unwrap();
        let mut sched = scheduler(scene.into_world());
        sched.run_frame(0.016);
        assert!(queue(&sched).is_empty());

        {
            let commands = sched.resources_mut().get_mut::<DeferredCommands>().unwrap();
            commands.spawn(tracked_circle(0.0));
        }
        // 事件帧末发出、下一帧 Prepare 消费。
        sched.run_frame(0.016);
        sched.run_frame(0.016);
        let items = queue(&sched);
        assert_eq!(items.len(), 1, "增量 spawn 应新增 1 项");
        assert_eq!(items, full(&sched));
        assert!(
            sched
                .resources()
                .get::<ComposerQueue>()
                .unwrap()
                .contains(items[0]),
            "新增项必须可查"
        );
    }

    /// 4. Despawn ⇒ 派生态减少对应项，无残留。
    #[test]
    fn despawn_removes_item_incrementally() {
        let mut scene = SceneStruct::new();
        let id = scene.spawn_dyn(tracked_circle(0.0)).unwrap();
        scene
            .spawn_dyn(vec![Box::new(Composer::default())])
            .unwrap();
        let mut sched = scheduler(scene.into_world());
        sched.run_frame(0.016);
        assert_eq!(queue(&sched), vec![id]);

        sched
            .resources_mut()
            .get_mut::<DeferredCommands>()
            .unwrap()
            .despawn(id);
        // 事件帧末发出、下一帧 Prepare 消费。
        sched.run_frame(0.016);
        sched.run_frame(0.016);
        assert!(queue(&sched).is_empty(), "despawn 后不得残留");
        assert_eq!(queue(&sched), full(&sched));
    }

    /// 5. 事件不修改合成器配置（配置只能经 `Scene::set` 命令路径改）。
    #[test]
    fn events_do_not_modify_composer_config() {
        let mut scene = SceneStruct::new();
        let composer = scene
            .spawn_dyn(vec![Box::new(Composer::default())])
            .unwrap();
        scene.spawn_dyn(tracked_circle(0.0)).unwrap();
        let before = scene
            .with::<Composer, _>(composer, |c| *c)
            .expect("Composer 在");

        let world = scene.into_world();
        let mut sched = scheduler(world);
        sched.run_frame(0.016);
        {
            let commands = sched.resources_mut().get_mut::<DeferredCommands>().unwrap();
            commands.spawn(tracked_circle(1.0));
        }
        sched.run_frame(0.016);

        let after = *sched
            .world()
            .get::<&Composer>(crate::contract::entity_of(composer).unwrap())
            .unwrap();
        assert_eq!(before, after, "生命周期事件不得改写合成配置");
    }

    /// 6. 跑两遍，派生态逐项相同（确定性）。
    #[test]
    fn composed_queue_is_deterministic() {
        fn run() -> Vec<ObjectId> {
            let mut scene = SceneStruct::new();
            scene
                .spawn_dyn(vec![Box::new(Composer::default())])
                .unwrap();
            for i in [3.0, 1.0, 4.0, 0.0, 2.0] {
                scene.spawn_dyn(tracked_circle(i)).unwrap();
            }
            let mut sched = scheduler(scene.into_world());
            sched.run_frame(0.016);
            sched.run_frame(0.016);
            queue(&sched)
        }
        assert_eq!(run(), run(), "两遍派生态必须逐项相同");
    }
}
