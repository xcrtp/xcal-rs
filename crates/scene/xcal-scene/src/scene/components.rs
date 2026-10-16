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

#[derive(Debug, Clone, Copy)]
pub struct ContentTextureItem;

/// **书写动画（reveal）对象**：持有一条描边路径 + 可见区间 `[start, end]`（归一化弧长）。
///
/// 与 [`MorphShape`] 同级、同一设计口径（几何静态、变量走 uniform）：
/// - `path` 帧间不变 ⇒ 后端按内容哈希缓存三角化与 GPU 缓冲（稳态零上传）；
/// - `start` / `end` 由 [`crate::animation::target::RevealRange`] 每帧写入
///   （`AnimValue::Scalar` → `end`；`AnimValue::Vec2` → `(start, end)`），
///   走 2D uniform 的 `params.z / params.w`。
///
/// 为什么是**独立组件**（而不是给 [`Path`](crate::scene::components) 加个区间字段）：
/// `(Path, StrokeSolid)` 会被 `SceneRecordSystem` 录进 `DrawQueue` **整条绘制**，
/// 而 reveal 对象必须**只**由 reveal 通路绘制（否则会叠加一条完整路径）。
/// 本组件不在 `record_scene` 的组件组合枚举里 ⇒ 天然互斥。
///
/// 默认 `start = 0` / `end = 1` = **完整可见**（与不带 reveal 的绘制等价）。
#[derive(Debug, Clone)]
pub struct Reveal {
    pub path: xc_notion::Path2D,
    /// 可见区间起点（归一化弧长 ∈ [0,1]），由 `RevealRange` 写入。
    pub start: f32,
    /// 可见区间终点（归一化弧长 ∈ [0,1]），由 `RevealRange` 写入。
    pub end: f32,
}

impl Reveal {
    /// 构造：`[0, 0]`（**尚未写出任何部分** —— 书写动画的自然起点）。
    pub fn new(path: xc_notion::Path2D) -> Self {
        Self {
            path,
            start: 0.0,
            end: 0.0,
        }
    }

    /// 构造：完整可见（`[0, 1]`）。
    pub fn visible(path: xc_notion::Path2D) -> Self {
        Self {
            path,
            start: 0.0,
            end: 1.0,
        }
    }
}

// M3-4：`Transform`（摆放组件）与 `Parent`（关系）已迁出本层 ——
// 前者归场景对象层 `xc-object`，后者统一用契约层
// `xc_scene_contract::Parent(pub ObjectId)`。原场景层的 `Parent` 组件把父对象
// 存成 ECS 实体句柄，是"场景层绑死具体 ECS"的最后一处组件形态，已删除。

macro_rules! define_comp {
    ($name:ident, $ty:ty) => {
        #[derive(
            derive_more::Deref, derive_more::DerefMut, derive_more::From, derive_more::Into,
        )]
        struct $name($ty);
    };
}

define_comp!(Position2, xc_notion::Vec2);
define_comp!(Position3, xc_notion::Vec3);
