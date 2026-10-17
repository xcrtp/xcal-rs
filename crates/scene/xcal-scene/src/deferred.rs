// deferred.rs — **延迟命令**（P6/B3）：Scene 内置的 spawn / despawn 队列 + 帧末应用。
//
// # 为什么需要延迟（不是过度设计，§4.1）
//
// despawn 不能发生在遍历中：录制路径（`record_scene` 一族）正用
// `for_each_pair_opt` **遍历 World**，同帧同步删除会破坏迭代。因此把"创建 / 销毁"
// 收集到一个**队列**里，帧末统一应用。
//
// # 两条路径（§4.1）
//
// 1. **装配期**：`Scene` 结构体自带 `defer()` / `apply_deferred()`（`scene.rs`），
//    适合"先声明、后统一应用"的脚本式装配；
// 2. **运行时**：场景已被 `into_world()` 交给调度器 ⇒ 延迟命令是一个 **Resource**
//    （[`DeferredCommands`]），由 **`FrameEnd` 相位的 [`DeferredApplySystem`]** 应用。
//    `Phase::FrameEnd` 此前为空（架构文档记"有意留空"）⇒ 正好可用，不改相位定义。
//
// # 相位顺序陷阱（§9.1 必须写明并覆盖）
//
// 三个录制 System 都在 `RenderCommit`；而延迟命令在 **`FrameEnd`** 应用
// ⇒ **本帧已录完 ⇒ 新对象下一帧才出现**。这是当前设计的**正确行为**（不是缺陷），
// 但必须被注释与用例锁定（见本文件测试 `deferred_spawn_appears_next_frame`）。
//
// # `hecs::CommandBuffer` / `ctx.commands` 零消费方不变（§9.2.4）
//
// 本轮走 Scene 自己的 [`DeferredCommands`] 队列，**不动调度层**的 `ctx.commands`
// ⇒ 不引入双轨。
//
// # §4.2 选型记账：出场 / 退场优先用视觉通道，不用真 spawn / despawn
//
// 真 spawn / despawn 是**一次性副作用**，会破坏"状态由 `t` 唯一决定"（重播问题）。
// 因此**出场 / 退场**应当优先用现成的视觉通道，而不是本模块的延迟命令：
//
// 1. **出场** = `Reveal`（`end` 0→1，笔尖写出）—— 已有；
// 2. **退场** = `Reveal` 反向（`start` 0→1 擦除，或 `end` 1→0 收回）—— 机制已有；
// 3. **淡入淡出** = `Opacity` + `OpacityValue` target —— 已有。
//
// 本轮的延迟命令（[`DeferredCmd`]）只保留给"**对象数量真的动态变化**"的场景，
// 并在重播时由 [`ReplaySystem`] 清账（使 `t` 重新唯一决定状态）。
//
// # `hecs::CommandBuffer` / `ctx.commands` 零消费方不变（§9.2.4）

use xc_scene_contract::{Components, ObjectId, Scene as SceneContract};
use xcal_schedule::context::ExecutorContext;
use xcal_schedule::system::System;

use crate::contract::WorldScene;
use crate::domain::{InputEvents, SceneEvent, ids};

/// 一条延迟命令。
///
/// `Components` 是契约层既有的类型擦除载荷（`Box<dyn Any + Send + Sync>`）⇒
/// 本枚举天然 `Send + Sync`，可作为 Resource 内的元素。
pub enum DeferredCmd {
    /// 延迟创建（载荷同 [`SceneContract::spawn_dyn`]）。
    Spawn { components: Components },
    /// 延迟销毁（目标是**已存在**对象的 `ObjectId`）。
    Despawn { target: ObjectId },
}

/// 运行时延迟命令队列（Resource）。
///
/// 由任意 System / 宿主 `defer`，由 [`DeferredApplySystem`]（FrameEnd）清空并应用。
/// 缺席（未插入）时系统 no-op ⇒ 与"可缺席资源"惯例一致。
#[derive(Default)]
pub struct DeferredCommands {
    cmds: Vec<DeferredCmd>,
}

impl DeferredCommands {
    pub fn new() -> Self {
        Self::default()
    }

    /// 入队一条命令。
    pub fn defer(&mut self, cmd: DeferredCmd) {
        self.cmds.push(cmd);
    }

    /// 便捷：延迟创建。
    pub fn spawn(&mut self, components: Components) {
        self.defer(DeferredCmd::Spawn { components });
    }

    /// 便捷：延迟销毁。
    pub fn despawn(&mut self, target: ObjectId) {
        self.defer(DeferredCmd::Despawn { target });
    }

    pub fn len(&self) -> usize {
        self.cmds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cmds.is_empty()
    }

    /// 取走全部命令（清空队列）。
    pub fn drain(&mut self) -> Vec<DeferredCmd> {
        std::mem::take(&mut self.cmds)
    }
}

/// **运行时创建的对象**台账（Resource）：供重播清理（B4）用。
///
/// 只登记**经延迟命令创建**的对象 —— 装配期（`Scene::spawn` / 脚本 `commit`）创建的对象
/// 由装配方自己负责，不属于"运行时创建"。
#[derive(Default)]
pub struct DynamicObjects {
    pub ids: Vec<ObjectId>,
}

impl DynamicObjects {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// **帧末应用延迟命令**（`FrameEnd` 相位）。
///
/// 顺序：弹出队列 → 在 `&mut World` 上应用（经契约层 [`WorldScene`]）→ 把新建对象
/// 登记进 [`DynamicObjects`]（供重播清理）。
pub struct DeferredApplySystem;

impl System for DeferredApplySystem {
    type Reads = ();
    type Writes = (DeferredCommands, DynamicObjects);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let cmds = match ctx.get_mut::<DeferredCommands>() {
            Some(q) => q.drain(),
            None => return,
        };
        if cmds.is_empty() {
            return;
        }

        let mut spawned: Vec<ObjectId> = Vec::new();
        {
            let mut scene = WorldScene::new(&mut *ctx.world);
            for cmd in cmds {
                match cmd {
                    DeferredCmd::Spawn { components } => {
                        if let Some(id) = scene.spawn_dyn(components) {
                            spawned.push(id);
                        }
                    }
                    DeferredCmd::Despawn { target } => {
                        scene.despawn(target);
                    }
                }
            }
        }

        if let Some(ledger) = ctx.get_mut::<DynamicObjects>() {
            ledger.ids.extend(spawned);
        }
    }
}

/// **重播清理**（`Prepare` 相位，B4）。
///
/// 「⟲ 重新播放」原本只 `seek(0)` —— 前提是"场景状态由 `t` 唯一决定"。运行时创建的
/// 对象破坏了这个前提（`t = 0` 也抹不掉它们）。本 System 在重播时把
/// [`DynamicObjects`] 台账里的对象**销毁并清账**，使状态重新由 `t` 唯一决定。
///
/// 「拨回 0 + 恢复播放」仍由 `TimeControlSystem`（LogicUpdate）负责 —— 两件事分开：
/// 时间归零是**时间**的事，对象清账是**场景**的事。
///
/// 相位选择：`Prepare`（在 `RenderCommit` 之前）⇒ 本帧就不会再录到被清掉的对象。
/// 该相位当前无其它 Resource 写者，无借用冲突。
pub struct ReplaySystem;

impl System for ReplaySystem {
    type Reads = (InputEvents,);
    type Writes = (DynamicObjects,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let replay = ctx
            .get::<InputEvents>()
            .map(|e| {
                e.iter()
                    .any(|ev| matches!(ev, SceneEvent::Button { id } if *id == ids::REPLAY))
            })
            .unwrap_or(false);
        if !replay {
            return;
        }

        let doomed = ctx
            .get_mut::<DynamicObjects>()
            .map(|ledger| std::mem::take(&mut ledger.ids))
            .unwrap_or_default();
        if doomed.is_empty() {
            return;
        }

        let mut scene = WorldScene::new(&mut *ctx.world);
        for id in doomed {
            scene.despawn(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::BindingApplySystem;
    use crate::scene::Scene as SceneStruct;
    use crate::TimeState;
    use xcal_schedule::resource::Resources;
    use xcal_schedule::scheduler::{Phase, Scheduler};
    use xc_object::Circle;

    fn circle(x: f32) -> Components {
        vec![Box::new(Circle::new((x, 0.0), 1.0))]
    }

    /// 计数探针：模拟"录制"（RenderCommit 阶段读 World 的对象数）。
    #[derive(Default)]
    struct Probe(usize);

    struct ProbeSystem;
    impl System for ProbeSystem {
        type Reads = ();
        type Writes = (Probe,);
        fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
            let n = ctx.world.query::<&Circle>().iter().count();
            if let Some(p) = ctx.get_mut::<Probe>() {
                p.0 = n;
            }
        }
    }

    /// **§9.2.1（headless 版）+ §9.2.3 相位顺序陷阱**：延迟创建的对象的
    /// **下一帧**才被录制（帧 N 为 0、帧 N+1 > 0）。
    #[test]
    fn deferred_spawn_appears_next_frame() {
        let mut res = Resources::new();
        res.insert(DeferredCommands::new());
        res.insert(DynamicObjects::new());
        res.insert(Probe::default());

        let mut sched = Scheduler::new(hecs::World::new(), res);
        // 录制探针在 RenderCommit；延迟命令在 FrameEnd 应用 ⇒ 相位陷阱复现。
        sched.add_system(Phase::RenderCommit, ProbeSystem);
        sched.add_system(Phase::FrameEnd, DeferredApplySystem);
        sched.init_resources();

        // 本帧入队一条延迟创建。
        sched
            .resources_mut()
            .get_mut::<DeferredCommands>()
            .unwrap()
            .spawn(circle(0.0));

        sched.run_frame(0.016);
        // 帧 N：录制发生在 FrameEnd 之前 ⇒ 探针看到 0（新对象尚未出现）。
        assert_eq!(
            sched.resources().get::<Probe>().unwrap().0,
            0,
            "延迟 spawn 的对象本帧不应被录制（相位陷阱）"
        );

        sched.run_frame(0.016);
        // 帧 N+1：上一帧末已创建 ⇒ 探针看到 1。
        assert_eq!(
            sched.resources().get::<Probe>().unwrap().0,
            1,
            "延迟 spawn 的对象下一帧应被录制"
        );
        assert_eq!(sched.resources().get::<DynamicObjects>().unwrap().len(), 1);
    }

    /// **§9.2.2**：延迟销毁后对象消失（下一帧探针为 0）。
    #[test]
    fn deferred_despawn_removes_object_next_frame() {
        let mut world = hecs::World::new();
        let id = {
            let mut ws = WorldScene::new(&mut world);
            ws.spawn_dyn(circle(0.0)).expect("Circle 已登记")
        };

        let mut res = Resources::new();
        res.insert(DeferredCommands::new());
        res.insert(DynamicObjects::new());
        res.insert(Probe::default());

        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::RenderCommit, ProbeSystem);
        sched.add_system(Phase::FrameEnd, DeferredApplySystem);
        sched.init_resources();

        // 帧 1：对象在（探针 1）。
        sched.run_frame(0.016);
        assert_eq!(sched.resources().get::<Probe>().unwrap().0, 1);

        // 入队延迟销毁。
        sched
            .resources_mut()
            .get_mut::<DeferredCommands>()
            .unwrap()
            .despawn(id);
        sched.run_frame(0.016);
        // 帧 2：录制时对象还在（FrameEnd 才删）⇒ 探针仍 1。
        assert_eq!(sched.resources().get::<Probe>().unwrap().0, 1);
        sched.run_frame(0.016);
        // 帧 3：已删除 ⇒ 探针 0。
        assert_eq!(sched.resources().get::<Probe>().unwrap().0, 0);
    }

    /// **B4 核心断言（§10.3.2）**：运行时创建的对象在重播后**不残留**。
    #[test]
    fn replay_removes_runtime_created_objects() {
        let replay_events = || {
            InputEvents::from_events(vec![SceneEvent::Button { id: ids::REPLAY }])
        };

        let mut res = Resources::new();
        res.insert(DeferredCommands::new());
        res.insert(DynamicObjects::new());
        res.insert(replay_events());

        let mut sched = Scheduler::new(hecs::World::new(), res);
        sched.add_system(Phase::FrameEnd, DeferredApplySystem);
        sched.add_system(Phase::Prepare, ReplaySystem);
        sched.init_resources();

        // 第一步：运行时创建两个对象（帧末应用）。
        {
            let q = sched.resources_mut().get_mut::<DeferredCommands>().unwrap();
            q.spawn(circle(0.0));
            q.spawn(circle(1.0));
        }
        sched.run_frame(0.016);
        assert_eq!(sched.world().len() as usize, 2, "运行时创建了两个对象");
        assert_eq!(sched.resources().get::<DynamicObjects>().unwrap().len(), 2);

        // 第二步：清掉 InputEvents 里的 REPLAY（模拟"只按一次"），换入重播事件。
        // 重播帧：ReplaySystem（Prepare）销毁台账里的对象。
        sched
            .resources_mut()
            .insert(InputEvents::from_events(vec![SceneEvent::Button {
                id: ids::REPLAY,
            }]));
        sched.run_frame(0.016);
        assert_eq!(sched.world().len() as usize, 0, "重播后运行时对象不残留");
        assert!(
            sched.resources().get::<DynamicObjects>().unwrap().is_empty(),
            "台账必须清空"
        );

        // 第三步（§10.3.3）：无运行时创建的帧，重播是无害 no-op。
        sched
            .resources_mut()
            .insert(InputEvents::from_events(vec![SceneEvent::Button {
                id: ids::REPLAY,
            }]));
        sched.run_frame(0.016);
        assert_eq!(sched.world().len() as usize, 0);
    }

    /// 无 `DeferredCommands` 资源时系统 no-op（不 panic）。
    #[test]
    fn missing_resource_is_noop() {
        let mut sched = Scheduler::new(hecs::World::new(), Resources::new());
        sched.add_system(Phase::FrameEnd, DeferredApplySystem);
        sched.init_resources();
        sched.run_frame(0.016);
    }

    /// **装配期路径**：`Scene::defer` + `apply_deferred`（`into_world` 前自动应用）。
    #[test]
    fn scene_struct_defer_applies_on_into_world() {
        let mut scene = SceneStruct::new();
        scene.defer(DeferredCmd::Spawn {
            components: circle(0.0),
        });
        assert_eq!(scene.deferred_len(), 1);
        let world = scene.into_world();
        assert_eq!(world.len() as usize, 1, "into_world 前应把延迟命令应用掉");
    }

    /// 装配期路径：手动 `apply_deferred`（幂等——应用后队列为空）。
    #[test]
    fn apply_deferred_is_idempotent() {
        let mut scene = SceneStruct::new();
        scene.defer(DeferredCmd::Spawn {
            components: circle(0.0),
        });
        scene.apply_deferred();
        assert_eq!(scene.len(), 1);
        assert_eq!(scene.deferred_len(), 0);
        scene.apply_deferred();
        assert_eq!(scene.len(), 1, "再次应用不得重复创建");
    }

    /// 与 B1/B2 的接线：延迟命令与脚本共存 —— 脚本 `commit` 的对象 + 延迟创建的对象
    /// 并存于同一 Scene（证明"不新建 Scene"）。
    #[test]
    fn script_and_deferred_coexist_in_one_scene() {
        use crate::scene_script::SceneEncoder;

        let mut scene = SceneStruct::new();
        let mut enc = SceneEncoder::new();
        enc.spawn(circle(0.0));
        let _recipe = enc.finish().commit(&mut scene).expect("ok");
        scene.defer(DeferredCmd::Spawn {
            components: circle(1.0),
        });
        scene.apply_deferred();
        assert_eq!(scene.len(), 2);

        // 绑定表可插入调度器（此处只证明类型接线成立）。
        let mut res = Resources::new();
        res.insert(TimeState::default());
        res.insert(_recipe.forest);
        res.insert(_recipe.bindings);
        let mut sched = Scheduler::new(scene.into_world(), res);
        sched.add_system(Phase::SceneSync, BindingApplySystem);
        sched.init_resources();
        sched.run_frame(0.016);
    }
}
