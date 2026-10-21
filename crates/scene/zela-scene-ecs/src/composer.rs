// composer.rs — ECS 侧（B3-1b 后）
//
// `Composer` / `ComposerQueue` / `full_recompute` 已下沉 `ze-scene-core`；
// 本层只留事件消费 System（它需要 `ExecutorContext` 与 hecs 世界）。
// 路径兼容：`zela_scene::composer::{Composer, ComposerQueue, full_recompute}` 转发自核心层。

use ze_scene_contract::ObjectId;
use zela_schedule::context::ExecutorContext;
use zela_schedule::system::System;

use crate::contract::WorldView;
use crate::lifetime::{LifetimeEvent, LifetimeEventKind, LifetimeEvents};

pub use ze_scene_core::composer::{Composer, ComposerQueue, full_recompute};

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
            .map(|queue| queue.initialized())
            .unwrap_or(false);

        if !has_composer {
            // 合成器被销毁 ⇒ 派生态失效；下一次注册重新做全量回放。
            if initialized {
                if let Some(queue) = ctx.get_mut::<ComposerQueue>() {
                    queue.invalidate();
                }
            }
            return;
        }

        // 首次看到合成器：全量快照（后注册也能收到注册前的对象）。
        if !initialized {
            let snapshot = full_recompute(&WorldView::new(ctx.world));
            if let Some(queue) = ctx.get_mut::<ComposerQueue>() {
                queue.initialize_with(snapshot);
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
                        queue.insert(id);
                    }
                    LifetimeEventKind::Despawned => {
                        queue.remove(id);
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
    use ze_object::Circle;
    use ze_scene_contract::LifetimeTracked;
    use ze_scene_contract::Scene as _;
    use ze_scene_contract::SceneRead as _;
        use zela_schedule::resource::Resources;
    use zela_schedule::scheduler::{Phase, Scheduler};

    use crate::lifetime::LifetimeEvents as Events;

    fn tracked_circle(x: f32) -> Vec<Box<dyn ze_scene_contract::Component>> {
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
