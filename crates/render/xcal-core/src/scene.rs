// scene.rs
//
// 纯逻辑组件：marker, mesh, animation + Circle/Path/Line（不含 StrokeSolid/FillSolid，
// 这些放到 xcal-render） + FunctionX2Y/Sampler/Scene，以及 3D 类型与图元生成器。
//
// 模块组织（文件模块，见 scene/ 目录）：
// - components：所有 ECS 组件（含 components/3d.rs 的 Camera3D/Mesh3D 等，
//   以 `three_d` 模块名挂载；components/animation/orbit.rs 的 OrbitAnim）
// - primitives：内置图元生成器（primitives/cube.rs 的线框/实心立方体）

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

    pub fn render<T, F>(&mut self, f: F) -> T
    where
        F: FnOnce(&hecs::World, &ResourceMap) -> T,
    {
        f(&self.world, &self.resources)
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

    pub fn spawn(&mut self, bundle: impl hecs::DynamicBundle) -> hecs::Entity {
        self.world.spawn(bundle)
    }

    // ─── 访问底层 World / 所有权转移 ──────────────────────────────

    pub fn world(&self) -> &hecs::World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut hecs::World {
        &mut self.world
    }

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
    use super::components::*;
    use super::*;

    #[test]
    fn test_scene_new_is_empty() {
        let mut scene = Scene::new();
        let count = scene.render(|w, _| w.len());
        assert_eq!(count, 0);
    }

    #[test]
    fn test_scene_spawn() {
        let mut scene = Scene::new();
        scene.spawn((Circle::new((0.0, 0.0), 1.0),));
        let count = scene.render(|w, _| w.len());
        assert_eq!(count, 1);
    }

    #[test]
    fn test_scene_spawn_multiple() {
        let mut scene = Scene::new();
        scene.spawn((Circle::new((0.0, 0.0), 1.0),));
        scene.spawn((Circle::new((1.0, 0.0), 2.0),));
        let count = scene.render(|w, _| w.len());
        assert_eq!(count, 2);
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

    #[test]
    fn test_scene_render_returns_value() {
        let mut scene = Scene::new();
        scene.add_resource(7_i32);
        let result = scene.render(|w, r| {
            let count = w.len();
            let res = r
                .get(&std::any::TypeId::of::<i32>())
                .and_then(|b| b.downcast_ref::<i32>())
                .copied();
            (count, res)
        });
        assert_eq!(result.0, 0);
        assert_eq!(result.1, Some(7));
    }
}
