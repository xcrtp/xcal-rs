// scene.rs

use std::{any::TypeId, collections::HashMap};

type ResourceMap = HashMap<TypeId, Box<dyn std::any::Any>>;

pub mod components {
    use std::sync::Arc;

    pub mod marker {
        pub trait Device {}
        pub struct WGPUDevice;
        impl Device for WGPUDevice {}
        pub struct CPUDevice;
        impl Device for CPUDevice {}
    }
    pub mod mesh {
        use crate::types::Vec3;

        pub struct TriangleMesh {
            pub vertices: Vec<Vec3>,
            pub indices: Vec<u32>,
        }
        pub struct QuadMesh {
            pub vertices: Vec<Vec3>,
            pub indices: Vec<u32>,
        }
        #[derive(Debug, Clone)]
        pub struct PathMesh {
            pub vertices: Vec<Vec3>,
            pub indices: Vec<u32>,
        }
    }
    pub mod render {
        use crate::types::Color32;

        #[derive(Debug, Clone, derive_new::new)]
        pub struct StrokeSolid {
            pub width: f32,
            #[new(into)]
            pub color: Color32,
        }
        #[derive(Debug, Clone, derive_new::new)]
        pub struct FillSolid {
            #[new(into)]
            pub color: Color32,
        }

        pub enum FillStyle {
            Solid(FillSolid),
        }
        pub enum StrokeStyle {
            Solid(StrokeSolid),
        }
    }

    pub mod animation {

        #[derive(Clone, Debug)]
        pub enum ClockState {
            Step,
            Suspend,
            Loop,
        }

        #[derive(Debug, Clone, derive_builder::Builder, derive_new::new)]
        pub struct LocalClock {
            pub parent: Option<hecs::Entity>,
            pub parent_offset: f32,
            pub current: f32,
            pub state: ClockState,
        }
    }

    pub use animation::*;
    pub use marker::*;
    pub use mesh::*;
    pub use render::*;

    use crate::types::{Vec2, Vec3};

    #[derive(Debug, Clone, derive_builder::Builder, derive_new::new)]
    pub struct Circle {
        #[new(into)]
        pub center: Vec2,
        pub radius: f32,
    }

    #[derive(
        Debug,
        Clone,
        derive_builder::Builder,
        derive_new::new,
        xcal_macro::Transform2DInto,
    )]
    pub struct Path {
        #[new(into_iter = "Vec3")]
        #[transform(iter)]
        pub vertices: Vec<Vec3>,
        #[transform(skip)]
        pub closed: bool,
    }

    /// 线段图元：两个端点独立完成坐标转换（派生 [`xcal::render::Transform2DInto`]）。
    #[derive(
        Debug,
        Clone,
        Copy,
        derive_builder::Builder,
        derive_new::new,
        xcal_macro::Transform2DInto,
    )]
    pub struct Line {
        #[new(into)]
        pub a: Vec2,
        #[new(into)]
        pub b: Vec2,
    }

    pub trait SyncSendFn<X, Y>: Sync + Send + Fn(X) -> Y {}
    impl<X, Y, T: Send + Sync + Fn(X) -> Y> SyncSendFn<X, Y> for T {}

    #[derive(Clone)]
    pub struct FunctionX2Y<X = f32, Y = f32> {
        pub f: Arc<dyn SyncSendFn<X, Y>>,
    }
    macro_rules! impl_rc_new {
        ($ty:ident) => {
            impl<X, Y> $ty<X, Y> {
                pub fn new(f: impl SyncSendFn<X, Y> + 'static) -> Self {
                    Self {
                        f: Arc::new(f) as Arc<dyn SyncSendFn<X, Y>>,
                    }
                }
            }
        };
    }
    impl_rc_new!(FunctionX2Y);

    #[derive(Clone)]
    pub struct ParameterCurveT2XY<T = f32, XY = (f32, f32)> {
        pub f: Arc<dyn Fn(T) -> XY>,
    }
    impl_rc_new!(ParameterCurveT2XY);

    #[derive(Clone)]
    pub struct ParameterCurveT2XYZ<T, XYZ = (f32, f32, f32)> {
        pub f: Arc<dyn Fn(T) -> XYZ>,
    }
    impl_rc_new!(ParameterCurveT2XYZ);

    #[derive(Clone, derive_builder::Builder, derive_new::new)]
    pub struct Sampler1D<T = f32> {
        pub start: T,
        pub end: T,
        pub step: T,
    }
    #[derive(Clone, derive_builder::Builder, derive_new::new)]
    pub struct Sampler2D<T = f32> {
        pub u_start: T,
        pub u_end: T,
        pub u_step: T,
        pub v_start: T,
        pub v_end: T,
        pub v_step: T,
    }

    #[derive(Clone, derive_builder::Builder, derive_new::new)]
    pub struct Interpolator<T = f32> {
        pub f: Arc<dyn Fn(T) -> T>,
    }

    #[derive(Debug, Clone, derive_builder::Builder)]
    pub struct Parent {
        pub parent: hecs::Entity,
    }
    #[derive(Debug, Clone, derive_builder::Builder)]
    pub struct Transform {
        pub position: glam::Vec3,
        pub rotation: glam::Quat,
        pub scale: glam::Vec3,
    }

    macro_rules! define_comp {
        ($name:ident, $ty:ty) => {
            #[derive(
                derive_more::Deref, derive_more::DerefMut, derive_more::From, derive_more::Into,
            )]
            struct $name($ty);
        };
    }

    define_comp!(Position2, glam::Vec2);
    define_comp!(Position3, glam::Vec3);
}

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

    /// 只读访问底层 World。
    pub fn world(&self) -> &hecs::World {
        &self.world
    }

    /// 可变访问底层 World。
    pub fn world_mut(&mut self) -> &mut hecs::World {
        &mut self.world
    }

    /// 消费 Scene，取出内部的 World 用于交给 Scheduler 等外部持有者。
    pub fn into_world(self) -> hecs::World {
        self.world
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::components::*;

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
        scene.add_resource(3.14_f64);
        assert_eq!(scene.borrow_resource::<i32>(), Some(&42));
        assert_eq!(scene.borrow_resource::<f64>(), Some(&3.14));
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

    #[test]
    fn test_scene_query_components() {
        let mut scene = Scene::new();
        scene.spawn((
            Circle::new((0.0, 0.0), 1.0),
            StrokeSolid::new(2.0, (100, 200, 50, 255)),
        ));
        scene.spawn((Circle::new((3.0, 4.0), 2.0),));

        let circle_count = scene.render(|w, _| w.query::<&Circle>().iter().count());
        assert_eq!(circle_count, 2);

        let stroke_count = scene
            .render(|w, _| w.query::<(&Circle, &StrokeSolid)>().iter().count());
        assert_eq!(stroke_count, 1);
    }
}
