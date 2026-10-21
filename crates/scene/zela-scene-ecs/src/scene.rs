// scene.rs
//
// 纯逻辑组件：marker, mesh, animation + Circle/Path/Line（不含 StrokeSolid/FillSolid，
// 这些放到 zela-render） + Scene，以及 3D 类型与图元生成器。
// 连续数学对象（FunctionX2Y / Sampler / ParameterCurve / Interpolator）M1 起归位理念层 `ze-notion`。
//
// 模块组织（文件模块，见 scene/ 目录）：
// - components：所有 ECS 组件（含 components/3d.rs 的 Camera3D/MeshGeometry/
//   Material3D 等，以 `three_d` 模块名挂载；components/animation/orbit.rs 的 OrbitAnim）
// - primitives：内置图元生成器（primitives/cube.rs 的线框/实心立方体）

// M3-8（ECS 泄漏收敛）：
// - 原来的 `render<T, F>(f: FnOnce(&hecs::World, &ResourceMap) -> T)` **已删除** ——
//   它把 `&hecs::World` 经闭包交给任意调用方，是 hecs 泄漏里最重的一处，
//   也是 M3-7（`record_scene` 不认识 hecs）的前提。需要读场景时走契约层的
//   [`ze_scene_contract::SceneRead`]（`for_each` / `for_each_pair`），
//   需要"同时拿 world 与 resource"时走 `zela-schedule` 的
//   `ExecutorContext::with_world_and_resource_mut`。
// - 其余 4 个入口（`world` / `world_mut` / `into_world` / `spawn`）**保留**，
//   它们是**ECS 特定**的：替换实现（例如非 ECS 场景）时这几个方法必须**随之重写**，
//   不得被当作契约层能力使用。

use std::{any::TypeId, collections::HashMap};

type ResourceMap = HashMap<TypeId, Box<dyn std::any::Any>>;

// B3-1b：组件与网格生成器已下沉 `ze-scene-core`；这里保留原模块路径
// （`zela_scene_ecs::scene::components` / `zela_scene_ecs::scene::primitives`）。
pub mod components {
    pub use ze_scene_core::components::*;
}

pub mod primitives {
    pub use ze_scene_core::mesh_primitives::*;
}

pub mod resource {}

pub use components as comp;

use crate::contract::WorldScene;
use crate::deferred::DeferredCmd;
// 契约层写 trait 的方法（`spawn_dyn` / `despawn`）经此进入方法解析；
// 匿名导入避免与本模块的**结构体** `Scene` 同名冲突。
use ze_scene_contract::Scene as _;

pub struct Scene {
    world: hecs::World,
    resources: ResourceMap,
    /// **延迟命令队列**（P6/B3，§4.1）：装配期的 spawn / despawn 先入队，
    /// `apply_deferred()`（或 `into_world()`）时统一应用 —— 避免"边遍历边删"。
    ///
    /// ⚠️ 运行时（场景已 `into_world()` 交给调度器）的延迟命令走另一条路径：
    /// Resource `zela_scene_ecs::deferred::DeferredCommands` + `FrameEnd` 相位的
    /// `DeferredApplySystem`（本字段那时已随 `into_world` 消耗掉）。
    deferred: Vec<DeferredCmd>,
}

impl Scene {
    pub fn new() -> Self {
        Self {
            world: hecs::World::new(),
            resources: HashMap::new(),
            deferred: Vec::new(),
        }
    }

    /// 对象数（ECS 特定入口：替换实现时随之重写）。
    pub fn len(&self) -> usize {
        self.world.len() as usize
    }

    /// 是否为空（ECS 特定入口：替换实现时随之重写）。
    pub fn is_empty(&self) -> bool {
        self.world.is_empty()
    }

    pub fn add_resource<T: 'static>(&mut self, resource: T) {
        self.resources.insert(TypeId::of::<T>(), Box::new(resource));
    }
    pub fn borrow_resource<T: 'static>(&self) -> Option<&T> {
        self.resources
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }
    pub fn borrow_resource_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.resources
            .get_mut(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_mut::<T>())
    }

    /// ECS 特定入口（替换实现时随之重写）：spawn 一个 hecs bundle。
    ///
    /// 契约层的动态创建入口是 [`ze_scene_contract::Scene::spawn_dyn`]；
    /// 本方法只服务"启动时用元组一次性建场景"的 App 侧代码。
    pub fn spawn(&mut self, bundle: impl hecs::DynamicBundle) -> hecs::Entity {
        self.world.spawn(bundle)
    }

    // ─── 访问底层 World / 所有权转移（全部为 ECS 特定入口）──────────────

    /// ECS 特定入口（替换实现时随之重写）。
    pub fn world(&self) -> &hecs::World {
        &self.world
    }

    /// ECS 特定入口（替换实现时随之重写）。
    pub fn world_mut(&mut self) -> &mut hecs::World {
        &mut self.world
    }

    /// ECS 特定入口（替换实现时随之重写）：把场景所有权交给调度器。
    ///
    /// **B3**：交出前先 `apply_deferred()` —— 否则待应用的延迟命令会被**静默丢弃**
    /// （`deferred` 字段不属于 `hecs::World`）。
    pub fn into_world(mut self) -> hecs::World {
        self.apply_deferred();
        self.world
    }

    // ─── 延迟命令（B3，§4.1）──────────────────────────────────────────────

    /// 入队一条延迟命令（`commit` 风格的声明期创建 / 销毁）。
    pub fn defer(&mut self, cmd: DeferredCmd) {
        self.deferred.push(cmd);
    }

    /// 当前待应用的延迟命令数。
    pub fn deferred_len(&self) -> usize {
        self.deferred.len()
    }

    /// **统一应用**延迟命令（清空队列）。经契约层写路径 [`WorldScene`] 落地，
    /// 与运行时路径 [`crate::deferred::DeferredApplySystem`] 应用的是同一套操作。
    pub fn apply_deferred(&mut self) {
        let cmds = std::mem::take(&mut self.deferred);
        if cmds.is_empty() {
            return;
        }
        let mut ws = WorldScene::new(&mut self.world);
        for cmd in cmds {
            match cmd {
                DeferredCmd::Spawn { components } => {
                    ws.spawn_dyn(components);
                }
                DeferredCmd::Despawn { target } => {
                    ws.despawn(target);
                }
            }
        }
    }
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ze_scene_core::Circle;

    #[test]
    fn test_scene_new_is_empty() {
        let scene = Scene::new();
        assert_eq!(scene.len(), 0);
        assert!(scene.is_empty());
    }

    #[test]
    fn test_scene_spawn() {
        let mut scene = Scene::new();
        scene.spawn((Circle::new((0.0, 0.0), 1.0),));
        assert_eq!(scene.len(), 1);
    }

    #[test]
    fn test_scene_spawn_multiple() {
        let mut scene = Scene::new();
        scene.spawn((Circle::new((0.0, 0.0), 1.0),));
        scene.spawn((Circle::new((1.0, 0.0), 2.0),));
        assert_eq!(scene.len(), 2);
    }

    #[test]
    fn test_scene_add_and_borrow_resource() {
        let mut scene = Scene::new();
        scene.add_resource(42_i32);
        assert_eq!(scene.borrow_resource::<i32>(), Some(&42));
    }

    #[test]
    fn test_scene_borrow_resource_mut() {
        let mut scene = Scene::new();
        scene.add_resource(42_i32);
        if let Some(r) = scene.borrow_resource_mut::<i32>() {
            *r = 100;
        }
        assert_eq!(scene.borrow_resource::<i32>(), Some(&100));
    }

    #[test]
    fn test_scene_borrow_missing_resource() {
        let scene = Scene::new();
        assert!(scene.borrow_resource::<i32>().is_none());
    }

    #[test]
    fn test_scene_borrow_mut_missing_resource() {
        let mut scene = Scene::new();
        assert!(scene.borrow_resource_mut::<i32>().is_none());
    }

    #[test]
    fn test_scene_resource_type_isolation() {
        let mut scene = Scene::new();
        scene.add_resource(42_i32);
        scene.add_resource(2.5_f64);
        assert_eq!(scene.borrow_resource::<i32>(), Some(&42));
        assert_eq!(scene.borrow_resource::<f64>(), Some(&2.5));
    }

    /// M3-8 后不再有 `render(&hecs::World)` 闭包逃逸：读场景经契约层 / 本类型的
    /// 值访问器，读 Resource 走 `borrow_resource`。
    #[test]
    fn test_scene_len_and_resource_readback() {
        let mut scene = Scene::new();
        scene.add_resource(7_i32);
        assert_eq!(scene.len(), 0);
        assert_eq!(scene.borrow_resource::<i32>(), Some(&7));
    }
}
