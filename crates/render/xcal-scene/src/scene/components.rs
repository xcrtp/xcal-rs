// components.rs
//
// 场景组件集合（文件模块）：marker / mesh / 3D 核心类型，
// 以及 Circle/Path/Line/PhaseAnim 等纯逻辑 2D 组件。
// 渲染相关样式组件（StrokeSolid/FillSolid）在 xcal-render 中。
//
// 注：旧动画组件（ClockState / LocalClock / CircleAnim / OrbitAnim）已删除，
// 动画统一由 Binding 表 + xc-time::Curve 纯函数驱动。

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

/// 3D 核心类型（PrimitiveTopology / MeshGeometry / Material3D /
/// Projection / Camera3D）。
/// 文件名以数字开头，无法直接作为 Rust 标识符，通过 `#[path]` 挂载为 `three_d`。
#[path = "components/3d.rs"]
pub mod three_d;

pub use marker::*;
pub use mesh::*;
pub use three_d::*;

use crate::types::Vec2;

#[derive(Debug, Clone, derive_builder::Builder, derive_new::new)]
pub struct Circle {
    #[new(into)]
    pub center: Vec2,
    pub radius: f32,
}

/// FunctionX2Y 相位滑动参数：`phase = t * omega`，
/// 由 `SceneRecordSystem` 采样时取 `x + phase` 实现曲线水平平移。
/// `omega` 为常量；`phase` 由 `BindingApplySystem` 每帧覆盖。
#[derive(Debug, Clone, derive_builder::Builder, derive_new::new)]
pub struct PhaseAnim {
    pub phase: f32,
    pub omega: f32,
}

/// 可变形形状：持有 `from` / `to` 与目标参数 `t`，由 [`crate::animation::MorphApplySystem`]
/// 每帧插值进同实体的 [`Path`]（render 层由 `record_scene` 消费 `(Path, StrokeSolid)`）。
///
/// 设计（S1 计划 §3.3）：**只写 `t`**（一个 f32），不每帧重建 `from`/`to` —— 为 S3
/// 上 GPU 预留（from/to 上传一次，t 每帧一个 float）。描边样式（`StrokeSolid`）
/// 由 xcal-render 提供，作为同一实体上的独立组件，本组件只管几何。
#[derive(Debug, Clone)]
pub struct MorphShape {
    pub from: xc_geometry::Path2D,
    pub to: xc_geometry::Path2D,
    /// 形变参数 ∈ [0, 1]，由 `BindingApplySystem`（`BindingTarget::MorphT`）写入。
    pub t: f32,
    /// 输出顶点数（恒定；与 from/to 复杂度无关）。
    pub samples: u32,
}

impl MorphShape {
    /// 用默认顶点数（[`xc_geometry::DEFAULT_MORPH_SAMPLES`]）构造，`t = 0`（= from）。
    pub fn new(from: xc_geometry::Path2D, to: xc_geometry::Path2D) -> Self {
        Self {
            from,
            to,
            t: 0.0,
            samples: xc_geometry::DEFAULT_MORPH_SAMPLES as u32,
        }
    }
}

/// 2D 路径组件：直接持有 [`xc_geometry::Path2D`]（命令序列，自带闭合语义，
/// `closed` 不再作为独立字段——避免两处真相）。
///
/// 坐标变换（屏幕 ↔ 逻辑）由 `xc_macro::Transform2DInto` 派生，落到
/// `xcal_scene::transform` 里 `Path2D` 的手动实现（逐命令映射，含椭圆弧解析改写）。
#[derive(Debug, Clone, derive_builder::Builder, derive_new::new, xc_macro::Transform2DInto)]
pub struct Path {
    pub path: xc_geometry::Path2D,
}

/// 线段图元：两个端点独立完成坐标转换（派生 [`xcal_scene::transform::Transform2DInto`]）。
#[derive(
    Debug, Clone, Copy, derive_builder::Builder, derive_new::new, xc_macro::Transform2DInto,
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

/// 标记组件：该实体参与内容纹理层渲染（S3-3）。
///
/// 只有带此标记的实体才会被 `ContentTextureRecordSystem` 录进内容纹理
/// （避免网格线 / 坐标轴等也进贴图，见计划 D5）。
#[derive(Debug, Clone, Copy)]
pub struct ContentTextureItem;

/// 3D 实体变换：平移 + 旋转 + 缩放（glam 原生类型）。
#[derive(Debug, Clone, derive_builder::Builder)]
pub struct Transform {
    pub position: glam::Vec3,
    pub rotation: glam::Quat,
    pub scale: glam::Vec3,
}

impl Transform {
    /// 组合模型矩阵：scale → rotation → translation（列主序，右乘约定）。
    pub fn matrix(&self) -> glam::Mat4 {
        glam::Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: glam::Vec3::ZERO,
            rotation: glam::Quat::IDENTITY,
            scale: glam::Vec3::ONE,
        }
    }
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
