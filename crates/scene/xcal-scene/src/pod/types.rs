// types.rs — 绑定层的类型词汇：**一处定义**（derive 同时生成 schema、文档名、转换）。
//
// 这些类型不是"为了脚本另写一套"：
// - 它们是 `SceneApi` / `ContentApi` 的形参类型，`#[derive(PodType)]` 生成
//   `TypeSpec` + 双向转换 + `IntoPodTypeName`；
// - 宿主侧 Service 把它们的值转成场景原语（`xc_object` / `xc_time` …）。

use crate::{AnimValue, Curve as NativeCurve, Interp};
use xc_pod::Handle;
use xc_pod_macro::PodType;

/// 颜色（取代旧的 `rgba: u32` 打包）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PodType)]
#[pod_type(name = "xcal.Color")]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// 二维点（数组元素 / 嵌套字段）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.Vec2")]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// 圆组件。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.Circle")]
pub struct Circle {
    pub x: f32,
    pub y: f32,
    pub r: f32,
}

/// 描边组件（`color` 是结构体，不再打包）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.Stroke")]
pub struct Stroke {
    pub width: f32,
    pub color: Color,
}

/// 2D 相机取景组件。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.Camera2D")]
pub struct Camera2D {
    pub center: Vec2,
    pub scale: f32,
}

/// 书写动画组件（扁平点表的替代：`Array<Vec2>`）。
#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.Reveal")]
pub struct Reveal {
    pub points: Vec<Vec2>,
    pub closed: bool,
}

/// 线样式（**纯值**；与组件 `Stroke` 分开，避免"类型构造器返回句柄"的歧义）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.LineStyle")]
pub struct LineStyle {
    pub width: f32,
    pub color: Color,
}

/// 坐标网格组件（容器；几何由 `AxesRefreshSystem` 每帧生成）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.Axes2D")]
pub struct Axes2D {
    /// 刻度的屏幕空间最小间距（像素）。
    pub min_px: f32,
    /// 网格线样式。
    pub grid: LineStyle,
    /// 坐标轴样式。
    pub axis: LineStyle,
}

/// 箭头图元。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.Arrow")]
pub struct Arrow {
    pub from: Vec2,
    pub to: Vec2,
    pub head_length: f32,
    pub head_width: f32,
}

/// 实心填充样式。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.Fill")]
pub struct Fill {
    pub color: Color,
}

/// 逐对象摆放（当前只暴露位置；旋转 / 缩放沿用默认）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.Transform")]
pub struct Transform {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// 内容服务编译出的一条描边路径（**值**，不是组件）。
///
/// `path` 是内容服务路径表里的不透明句柄；`color` / `width` 由内容服务从
/// 公式 IR 的描边样式里解析（世界空间）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.Path")]
pub struct Path {
    pub path: Handle,
    pub color: Color,
    pub width: f32,
}

/// `Array<Path>` 的具名载体。
///
/// 为什么需要它：Rust 的 orphan 规则不允许在**外部 trait**（`IntoPodTypeName`）
/// 上为 `Vec<本地类型>` 实现 —— 即使 `Path` 是本地类型，`Vec` 仍是外部类型。
/// 本 newtype 的 schema 仍是 `Array<Struct(xcal.Path)>`（`PodType::SPEC` 转发给 `Vec<Path>`），
/// 所以绑定层看到的仍是数组，不是结构体。
#[derive(Clone, Debug, PartialEq)]
pub struct PathList(pub Vec<Path>);

impl xc_pod::PodType for PathList {
    const SPEC: &'static xc_pod::TypeSpec = <Vec<Path> as xc_pod::PodType>::SPEC;

    fn to_pod_value(&self) -> xc_pod::PodValue {
        <Vec<Path> as xc_pod::PodType>::to_pod_value(&self.0)
    }

    fn from_pod_value(value: &xc_pod::PodValue) -> Result<Self, xc_pod::PodValueError> {
        <Vec<Path> as xc_pod::PodType>::from_pod_value(value).map(Self)
    }
}

impl xc_pod::IntoPodTypeName for PathList {
    const NAME: xc_pod::PodTypeName = "Array<xcal.Path>";
}

/// 书写组件组：`Reveal` + `StrokeSolid`（一个句柄对应多个组件）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.RevealPath")]
pub struct RevealPath {
    pub path: Path,
}

/// 静态路径组件组：`Path` + `StrokeSolid`。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.PathShape")]
pub struct PathShape {
    pub path: Path,
}

/// 公式编译请求。
#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.Formula")]
pub struct Formula {
    pub source: String,
    pub font_size: f32,
    pub x: f32,
    pub y: f32,
}

/// 纯文本编译请求（与公式同形，源码按正文语义包装）。
#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.Text")]
pub struct Text {
    pub source: String,
    pub font_size: f32,
    pub x: f32,
    pub y: f32,
}

/// 曲线：**一个枚举的五个变体**（取代 sin / phase / constant / ramp / ramp2 五个函数）。
///
/// `dur` 用 `f64`：段内关键帧时间与 `Timeline` 的 `duration` 同精度，
/// 否则 `1/N` 这类时长在 f32 上会与手写分镜差一个 ulp。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "xcal.Curve")]
pub enum Curve {
    Const { value: f32 },
    Sin { base: f32, amp: f32, omega: f32 },
    Phase { omega: f32 },
    Ramp { from: f32, to: f32, dur: f64 },
    Ramp2 { from: Vec2, to: Vec2, dur: f64 },
}

impl Curve {
    /// 绑定层的曲线 → 场景层的曲线（装配期一次）。
    pub fn to_native(self) -> NativeCurve {
        match self {
            Curve::Const { value } => NativeCurve::Const(AnimValue::Scalar(value)),
            Curve::Sin { base, amp, omega } => NativeCurve::Sin { base, amp, omega },
            Curve::Phase { omega } => NativeCurve::Phase { omega },
            Curve::Ramp { from, to, dur } => NativeCurve::Keyframes {
                keys: vec![(0.0, AnimValue::Scalar(from)), (dur, AnimValue::Scalar(to))],
                interpolation: Interp::Linear,
            },
            Curve::Ramp2 { from, to, dur } => NativeCurve::Keyframes {
                keys: vec![
                    (0.0, AnimValue::Vec2(xc_notion::Vec2::new(from.x, from.y))),
                    (dur, AnimValue::Vec2(xc_notion::Vec2::new(to.x, to.y))),
                ],
                interpolation: Interp::Linear,
            },
        }
    }
}
