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

    #[derive(Debug, Clone, derive_builder::Builder, derive_new::new)]
    pub struct Path {
        #[new(into_iter = "Vec3")]
        pub vertices: Vec<Vec3>,
        pub closed: bool,
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
}
