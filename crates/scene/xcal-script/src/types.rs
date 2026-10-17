// types.rs — 绑定层的类型词汇：**一处定义**（derive 同时生成 schema、文档名、转换）。
//
// 这些类型不是"为了脚本另写一套"：
// - 它们是 `SceneApi` 的形参类型（结构体 / 枚举），`#[derive(VmType)]` 生成
//   `TypeSpec` + 双向转换 + `IntoVmTypeName`；
// - 宿主侧 Service 把它们的值转成场景原语（`xc_object::Circle` / `StrokeSolid` /
//   `xc_time::Curve` …）。

use xcal_scene::{AnimValue, Curve as NativeCurve, Interp};
use xc_vm_macro::VmType;

/// 颜色（取代旧的 `rgba: u32` 打包）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, VmType)]
#[vm_type(name = "xcal.Color")]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// 二维点（数组元素 / 嵌套字段）。
#[derive(Clone, Copy, Debug, PartialEq, VmType)]
#[vm_type(name = "xcal.Vec2")]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// 圆组件。
#[derive(Clone, Copy, Debug, PartialEq, VmType)]
#[vm_type(name = "xcal.Circle")]
pub struct Circle {
    pub x: f32,
    pub y: f32,
    pub r: f32,
}

/// 描边组件（`color` 是结构体，不再打包）。
#[derive(Clone, Copy, Debug, PartialEq, VmType)]
#[vm_type(name = "xcal.Stroke")]
pub struct Stroke {
    pub width: f32,
    pub color: Color,
}

/// 2D 相机取景组件。
#[derive(Clone, Copy, Debug, PartialEq, VmType)]
#[vm_type(name = "xcal.Camera2D")]
pub struct Camera2D {
    pub center: Vec2,
    pub scale: f32,
}

/// 书写动画组件（扁平点表的替代：`Array<Vec2>`）。
#[derive(Clone, Debug, PartialEq, VmType)]
#[vm_type(name = "xcal.Reveal")]
pub struct Reveal {
    pub points: Vec<Vec2>,
    pub closed: bool,
}

/// 曲线：**一个枚举的五个变体**（取代 sin / phase / constant / ramp / ramp2 五个函数）。
#[derive(Clone, Copy, Debug, PartialEq, VmType)]
#[vm_type(name = "xcal.Curve")]
pub enum Curve {
    Const { value: f32 },
    Sin { base: f32, amp: f32, omega: f32 },
    Phase { omega: f32 },
    Ramp { from: f32, to: f32, dur: f32 },
    Ramp2 { from: Vec2, to: Vec2, dur: f32 },
}

impl Curve {
    /// 绑定层的曲线 → 场景层的曲线（装配期一次）。
    pub fn to_native(self) -> NativeCurve {
        match self {
            Curve::Const { value } => NativeCurve::Const(AnimValue::Scalar(value)),
            Curve::Sin { base, amp, omega } => NativeCurve::Sin { base, amp, omega },
            Curve::Phase { omega } => NativeCurve::Phase { omega },
            Curve::Ramp { from, to, dur } => NativeCurve::Keyframes {
                keys: vec![
                    (0.0, AnimValue::Scalar(from)),
                    (dur as f64, AnimValue::Scalar(to)),
                ],
                interpolation: Interp::Linear,
            },
            Curve::Ramp2 { from, to, dur } => NativeCurve::Keyframes {
                keys: vec![
                    (0.0, AnimValue::Vec2(xc_notion::Vec2::new(from.x, from.y))),
                    (dur as f64, AnimValue::Vec2(xc_notion::Vec2::new(to.x, to.y))),
                ],
                interpolation: Interp::Linear,
            },
        }
    }
}
