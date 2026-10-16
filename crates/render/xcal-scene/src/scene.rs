// scene.rs
//
// 纯逻辑组件：marker, mesh, animation + Circle/Path/Line（不含 StrokeSolid/FillSolid，
// 这些放到 xcal-render） + Scene，以及 3D 类型与图元生成器。
// 连续数学对象（FunctionX2Y / Sampler / ParameterCurve / Interpolator）M1 起归位理念层 `xc-notion`。
//
// 模块组织（文件模块，见 scene/ 目录）：
// - components：所有 ECS 组件（含 components/3d.rs 的 Camera3D/MeshGeometry/
//   Material3D 等，以 `three_d` 模块名挂载；components/animation/orbit.rs 的 OrbitAnim）
// - primitives：内置图元生成器（primitives/cube.rs 的线框/实心立方体）

// M3-8（ECS 泄漏收敛）：
// - 原来的 `render<T, F>(f: FnOnce(&hecs::World, &ResourceMap) -> T)` **已删除** ——
//   它把 `&hecs::World` 经闭包交给任意调用方，是 hecs 泄漏里最重的一处，
//   也是 M3-7（`record_scene` 不认识 hecs）的前提。需要读场景时走契约层的
//   [`xc_scene_contract::SceneRead`]（`for_each` / `for_each_pair`），
//   需要"同时拿 world 与 resource"时走 `xcal-schedule` 的
//   `ExecutorContext::with_world_and_resource_mut`。
// - 其余 4 个入口（`world` / `world_mut` / `into_world` / `spawn`）**保留**，
//   它们是**ECS 特定**的：替换实现（例如非 ECS 场景）时这几个方法必须**随之重写**，
//   不得被当作契约层能力使用。

use std::{any::TypeId, collections::HashMap};

type ResourceMap = HashMap<TypeId, Box<dyn std::any::Any>>;

/// ECS 组件集合（marker / mesh / 2D 图元 / 3D 类型）。
pub mod components;

/// 内置图元生成器（立方体等）。
pub mod primitives;

pub mod resource {}

pub use components as comp;

pub struct Scene {
    world: hecs::World,
    resources: ResourceMap,
}

impl Scene {
    pub fn new() -> Self {
        Self {
            world: hecs::World::new(),
            resources: HashMap::new(),
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
    /// 契约层的动态创建入口是 [`xc_scene_contract::Scene::spawn_dyn`]；
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
    pub fn into_world(self) -> hecs::World {
        self.world
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
    use xc_object::Circle;

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
