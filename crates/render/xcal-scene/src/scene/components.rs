// components.rs
//
// 场景组件集合（文件模块）：marker / mesh / 3D 核心类型，
// 以及 Circle/Path/Line/PhaseAnim 等纯逻辑 2D 组件。
// 渲染相关样式组件（StrokeSolid/FillSolid）在 xcal-render 中。
//
// 注：旧动画组件（ClockState / LocalClock / CircleAnim / OrbitAnim）已删除，
// 动画统一由 Binding 表 + xc-time::Curve 纯函数驱动。
//
// M1 T4：连续数学对象（FunctionX2Y / ParameterCurveT2XY / ParameterCurveT2XYZ /
// Sampler1D / Sampler2D / Interpolator / SyncSendFn）已归位理念层 `xc-notion`
// （func / curve / sample），此处**不再 re-export** —— 消费方直接改 import。
//
// M3-5：图元组件（Circle / Path / Line）已迁往**场景对象层** `xc-object`
// （newtype 封装理念层 `Circle2D` / `Path2D` / `Line2D`，消掉"字段两处定义"）。
// 本层（ECS 实现）只消费，不再定义；`Transform` / `Parent` / `MorphShape` 等
// 仍在下方（它们的归位见交付说明的延后清单）。

pub mod marker {
    pub trait Device {}
    pub struct WGPUDevice;
    impl Device for WGPUDevice {}
    pub struct CPUDevice;
    impl Device for CPUDevice {}
}

pub mod mesh {
    use xc_notion::Vec3;

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
    pub from: xc_notion::Path2D,
    pub to: xc_notion::Path2D,
    /// 形变参数 ∈ [0, 1]，由 `BindingApplySystem`（`BindingTarget::MorphT`）写入。
    pub t: f32,
    /// 输出顶点数（恒定；与 from/to 复杂度无关）。
    pub samples: u32,
}

impl MorphShape {
    /// 用默认顶点数（[`xc_notion::DEFAULT_MORPH_SAMPLES`]）构造，`t = 0`（= from）。
    pub fn new(from: xc_notion::Path2D, to: xc_notion::Path2D) -> Self {
        Self {
            from,
            to,
            t: 0.0,
            samples: xc_notion::DEFAULT_MORPH_SAMPLES as u32,
        }
    }
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
