// types.rs — 绑定层的类型词汇：**一处定义**（derive 同时生成 schema、文档名、转换）。
//
// 这些类型不是"为了脚本另写一套"：
// - 它们是 `SceneApi` / `ContentApi` 的形参类型，`#[derive(PodType)]` 生成
//   `TypeSpec` + 双向转换 + `IntoPodTypeName`；
// - 宿主侧 Service 把它们的值转成场景原语（`ze_object` / `ze_time` …）。

use ze_scene_core::animation::target::TransformChannel as NativeTransformChannel;
use ze_time::{AnimValue, Curve as NativeCurve, Interp as NativeInterp};
use ze_pod_core::Handle;
use ze_pod_macro::PodType;

/// 颜色（取代旧的 `rgba: u32` 打包）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PodType)]
#[pod_type(name = "zela.Color")]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// 二维点（数组元素 / 嵌套字段）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Vec2")]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

// ─── 3D（B1-D 脚本层入口）──────────────────────────────────────────────

/// 三维点（3D pod）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Vec3")]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// 四维点（3D pod；旋转四元数 xyzw 等）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Vec4")]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

/// 3D 相机参数（B1-D）。`aspect` 缺省由渲染视口提供（每帧覆盖），
/// 故不暴露；近 / 远裁剪固定 0.1 / 100（与 demo3d 一致）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Camera3D")]
pub struct Camera3D {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fov_y: f32,
}

/// 立方体网格参数（B1-D）。**参数化构造器**：只传半边长，不传顶点数组 ——
/// 顶点由宿主侧 `wireframe_cube` / `solid_cube` 生成（B1-D 硬约束）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Cube")]
pub struct Cube {
    /// 半边长（模型空间中心在原点）。
    pub half_extent: f32,
    /// `false` ⇒ 线框（`wireframe_cube`，默认）；`true` ⇒ 实体（`solid_cube`）。
    pub solid: bool,
}

/// 3D 摆放（B1-D）。映射到原生 `Transform`（单一真源：位置 / 旋转 / 缩放
/// 都在摆放组件上）。**不扩展** 2D 的 `Transform { x, y, z }` —— 那会破坏
/// 冻结的 fourier / storyboard 脚本（Lua 绑定要求全字段）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Transform3D")]
pub struct Transform3D {
    pub position: Vec3,
    /// 旋转四元数（xyzw，glam 约定）；恒等旋转为 `(0, 0, 0, 1)`。
    pub rotation: Vec4,
    pub scale: Vec3,
}

/// 3D 着色变体（脚本枚举）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PodType)]
#[pod_type(name = "zela.ShaderVariant")]
pub enum ShaderVariant {
    Flat,
    Lambert,
    Pbr,
}

/// 3D 材质参数（B1-D；B1-C PBR 字段；B2-7 意图层字段）。
///
/// **只暴露意图**（D19）：`double_sided` / `transparent` 是作者语义，
/// 后端把它们翻译成 `cull_mode` / `blend`（脚本不得出现 GPU 机制名）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Material3D")]
pub struct Material3D {
    pub color: Color,
    pub variant: ShaderVariant,
    /// 金属度 [0,1]（仅 `variant = Pbr` 消费）。
    pub metallic: f32,
    /// 粗糙度 [0,1]（仅 `variant = Pbr` 消费；默认 0.5）。
    pub roughness: f32,
    /// 是否双面可见（B2-7 / D19；`false` = 封闭实体）。
    pub double_sided: bool,
    /// 是否按透明处理（B2-7 / D19；`false` = 不透明）。
    pub transparent: bool,
    /// 采样纹理（B3 Step 0）：`None` 或引用某 Surface 的内容纹理。
    pub texture: Texture,
}

/// 材质可引用的纹理（脚本不接触裸 u64 句柄）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PodType)]
#[pod_type(name = "zela.Texture")]
pub enum Texture {
    None,
    /// 引用某 Surface 的内容纹理（句柄由 `ObjectId → LayerId → TextureHandle` 派生）。
    Surface { surface: Handle },
}

/// Surface 的坐标映射语义。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PodType)]
#[pod_type(name = "zela.SurfaceMapping")]
pub enum SurfaceMapping {
    World,
    Screen,
}

/// Surface 的渲染策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PodType)]
#[pod_type(name = "zela.SurfaceRenderer")]
pub enum SurfaceRenderer {
    Renderer2D,
    Renderer3D,
}

/// 纹理采样过滤。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PodType)]
#[pod_type(name = "zela.TextureFilter")]
pub enum TextureFilter {
    Nearest,
    Linear,
}

/// Surface 声明（B3 Step 0）：一次构造 `Surface` + `SurfaceRenderer` + `SurfaceTexture`。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Surface")]
pub struct Surface {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
    pub mapping: SurfaceMapping,
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub renderer: SurfaceRenderer,
    pub texel_width: u32,
    pub texel_height: u32,
    pub filter: TextureFilter,
    pub mipmap: bool,
    /// `0` = bounds 1:1 铺满；`> 0` = `fit_bounds` 边距语义。
    pub fit_margin: f32,
    /// 是否把内容层合成到屏幕（能力矩阵作品需要它才能被像素对拍）。
    pub composite: bool,
}

/// 光源类型。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.LightKind")]
pub enum LightKind {
    /// 方向光：`direction` 是光传播方向（世界空间，从光源指向场景）。
    Directional { direction: Vec3 },
    /// 点光：`range` 是有效半径（世界单位）。**不带 position** —— 位置由
    /// 同一对象的 `Transform` 提供（B1 铁律 3：单一真源）。
    Point { range: f32 },
}

/// 光源参数（B1-D）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Light")]
pub struct Light {
    pub kind: LightKind,
    pub color: Color,
    pub intensity: f32,
}

/// 圆组件。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Circle")]
pub struct Circle {
    pub x: f32,
    pub y: f32,
    pub r: f32,
}

/// 描边组件（`color` 是结构体，不再打包）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Stroke")]
pub struct Stroke {
    pub width: f32,
    pub color: Color,
}

/// 2D 相机取景组件。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Camera2D")]
pub struct Camera2D {
    pub center: Vec2,
    pub scale: f32,
}

/// 书写动画组件（扁平点表的替代：`Array<Vec2>`）。
#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Reveal")]
pub struct Reveal {
    pub points: Vec<Vec2>,
    pub closed: bool,
}

/// **静态折线**（S2）：构造时即 `start=0, end=1` 的可见 `Reveal`。
///
/// 它复用 `Reveal` 的组件与录制路径（含 S2 的 Transform 折算），
/// 只是**初态可见**；仍可继续用 `Target.RevealRange` 做二次书写。
#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Polyline")]
pub struct Polyline {
    pub points: Vec<Vec2>,
    pub closed: bool,
}

/// 父对象引用（S5）：声明期用脚本句柄，`commit` 时解析成 `ObjectId`。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Parent")]
pub struct Parent {
    pub parent: Handle,
}

/// 对象名（便于排查 / 文本组父对象）。
#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Name")]
pub struct Name {
    pub value: String,
}

/// 对象级不透明度（S6）：`0` 全透明 … `1` 完全不透明。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Opacity")]
pub struct Opacity {
    pub value: f32,
}

/// 组标签（B7 Step 4）：**纯组合**，不含变换。
///
/// 与 `Parent` 的分工（这也是「补作品而不是删」的依据）：`Parent` 级联父子变换，
/// `Group` 只给对象打一个"同属一批"的标签，供整体查询 / 批量控制使用；
/// 二者不能互相替代，也**不支持嵌套**。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PodType)]
#[pod_type(name = "zela.Group")]
pub struct Group {
    pub id: u32,
}

/// 函数图像相位参数（B7 Step 4）：`Target.Phase` 每帧写 `phase`。
///
/// `phase` 的消费者只有函数图像对象（`x + phase` 的采样平移）；脚本层目前
/// 没有创建函数图像对象的入口 ⇒ 本组件可挂、可写，但**不改变像素**（GAPS W1-06）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.PhaseAnim")]
pub struct PhaseAnim {
    pub phase: f32,
    pub omega: f32,
}

/// 可变形形状（B7 Step 4）：`from` / `to` 两条折线 + 形变参数 `t`。
///
/// `Target.MorphT` 每帧写 `t` ∈ [0,1]；渲染侧对两条折线按弧长插值。
/// `samples = 0` 表示用默认顶点数（与 `MorphShape::new` 一致）。
#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Morph")]
pub struct Morph {
    pub from: Vec<Vec2>,
    pub to: Vec<Vec2>,
    pub closed: bool,
    pub t: f32,
    pub samples: u32,
}

/// Transform 的写入通道（S3；B7 起只有三条显式通道 —— `Auto` 已删除）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PodType)]
#[pod_type(name = "zela.TransformChannel")]
pub enum TransformChannel {
    Position,
    Rotation,
    Scale,
}

impl TransformChannel {
    pub fn to_native(self) -> NativeTransformChannel {
        match self {
            Self::Position => NativeTransformChannel::Position,
            Self::Rotation => NativeTransformChannel::Rotation,
            Self::Scale => NativeTransformChannel::Scale,
        }
    }
}

/// 线样式（**纯值**；与组件 `Stroke` 分开，避免"类型构造器返回句柄"的歧义）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.LineStyle")]
pub struct LineStyle {
    pub width: f32,
    pub color: Color,
}

/// 坐标网格组件（容器；几何由 `AxesRefreshSystem` 每帧生成）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Axes2D")]
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
#[pod_type(name = "zela.Arrow")]
pub struct Arrow {
    pub from: Vec2,
    pub to: Vec2,
    pub head_length: f32,
    pub head_width: f32,
}

/// 实心填充样式。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Fill")]
pub struct Fill {
    pub color: Color,
}

/// 逐对象摆放（当前只暴露位置；旋转 / 缩放沿用默认）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Transform")]
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
#[pod_type(name = "zela.Path")]
pub struct Path {
    pub path: Handle,
    pub color: Color,
    pub width: f32,
}

/// `Array<Path>` 的具名载体。
///
/// 为什么需要它：Rust 的 orphan 规则不允许在**外部 trait**（`IntoPodTypeName`）
/// 上为 `Vec<本地类型>` 实现 —— 即使 `Path` 是本地类型，`Vec` 仍是外部类型。
/// 本 newtype 的 schema 仍是 `Array<Struct(zela.Path)>`（`PodType::SPEC` 转发给 `Vec<Path>`），
/// 所以绑定层看到的仍是数组，不是结构体。
#[derive(Clone, Debug, PartialEq)]
pub struct PathList(pub Vec<Path>);

impl ze_pod_core::PodType for PathList {
    const SPEC: &'static ze_pod_core::TypeSpec = <Vec<Path> as ze_pod_core::PodType>::SPEC;

    fn to_pod_value(&self) -> ze_pod_core::PodValue {
        <Vec<Path> as ze_pod_core::PodType>::to_pod_value(&self.0)
    }

    fn from_pod_value(value: &ze_pod_core::PodValue) -> Result<Self, ze_pod_core::PodValueError> {
        <Vec<Path> as ze_pod_core::PodType>::from_pod_value(value).map(Self)
    }
}

impl ze_pod_core::IntoPodTypeName for PathList {
    const NAME: ze_pod_core::PodTypeName = "Array<zela.Path>";
}

/// 书写组件组：`Reveal` + `StrokeSolid`（一个句柄对应多个组件）。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.RevealPath")]
pub struct RevealPath {
    pub path: Path,
}

/// 静态路径组件组：`Path` + `StrokeSolid`。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.PathShape")]
pub struct PathShape {
    pub path: Path,
}

/// 公式编译请求。
#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Formula")]
pub struct Formula {
    pub source: String,
    pub font_size: f32,
    pub x: f32,
    pub y: f32,
}

/// 纯文本编译请求（与公式同形，源码按正文语义包装）。
#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Text")]
pub struct Text {
    pub source: String,
    pub font_size: f32,
    pub x: f32,
    pub y: f32,
}

/// 标量关键帧（S4）：`t` 是段内局部时间（秒），`value` 是标量。
#[derive(Clone, Copy, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.ScalarKey")]
pub struct ScalarKey {
    pub t: f64,
    pub value: f32,
}

/// 关键帧插值方式（S4）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PodType)]
#[pod_type(name = "zela.KeyframeInterp")]
pub enum KeyframeInterp {
    Linear,
    Step,
}

impl KeyframeInterp {
    pub fn to_native(self) -> NativeInterp {
        match self {
            Self::Linear => NativeInterp::Linear,
            Self::Step => NativeInterp::Step,
        }
    }
}

/// 曲线：脚本侧的统一枚举（取代 sin / phase / constant / ramp / ramp2 五个函数）。
///
/// `dur` 用 `f64`：段内关键帧时间与 `Timeline` 的 `duration` 同精度，
/// 否则 `1/N` 这类时长在 f32 上会与手写分镜差一个 ulp。
#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "zela.Curve")]
pub enum Curve {
    Const { value: f32 },
    Sin { base: f32, amp: f32, omega: f32 },
    Phase { omega: f32 },
    Ramp { from: f32, to: f32, dur: f64 },
    Ramp2 { from: Vec2, to: Vec2, dur: f64 },
    /// **2D 公转**（S4）：`r * (cos θ, sin θ)`，`θ = speed * t + initial_angle`。
    Orbit { radius: f32, speed: f32, initial_angle: f32 },
    /// **标量关键帧**（S4）：段内 `t` → 值，按 `interpolation` 插值。
    Keyframes { keys: Vec<ScalarKey>, interpolation: KeyframeInterp },
}

impl Curve {
    /// 绑定层的曲线 → 场景层的曲线（装配期一次）。
    pub fn to_native(&self) -> NativeCurve {
        match self {
            Curve::Const { value } => NativeCurve::Const(AnimValue::Scalar(*value)),
            Curve::Sin { base, amp, omega } => NativeCurve::Sin {
                base: *base,
                amp: *amp,
                omega: *omega,
            },
            Curve::Phase { omega } => NativeCurve::Phase { omega: *omega },
            Curve::Ramp { from, to, dur } => NativeCurve::Keyframes {
                keys: vec![
                    (0.0, AnimValue::Scalar(*from)),
                    (*dur, AnimValue::Scalar(*to)),
                ],
                interpolation: NativeInterp::Linear,
            },
            Curve::Ramp2 { from, to, dur } => NativeCurve::Keyframes {
                keys: vec![
                    (0.0, AnimValue::Vec2(ze_notion::Vec2::new(from.x, from.y))),
                    (*dur, AnimValue::Vec2(ze_notion::Vec2::new(to.x, to.y))),
                ],
                interpolation: NativeInterp::Linear,
            },
            Curve::Orbit {
                radius,
                speed,
                initial_angle,
            } => NativeCurve::Orbit2D {
                radius: *radius,
                speed: *speed,
                initial_angle: *initial_angle,
            },
            Curve::Keyframes { keys, interpolation } => NativeCurve::Keyframes {
                keys: keys
                    .iter()
                    .map(|key| (key.t, AnimValue::Scalar(key.value)))
                    .collect(),
                interpolation: interpolation.to_native(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_orbit_maps_to_2d_orbit() {
        let native = Curve::Orbit {
            radius: 2.0,
            speed: 1.0,
            initial_angle: 0.0,
        }
        .to_native();
        assert!(matches!(native, NativeCurve::Orbit2D { radius, .. } if radius == 2.0));
    }

    #[test]
    fn script_keyframes_map_to_scalar_keyframes() {
        let native = Curve::Keyframes {
            keys: vec![
                ScalarKey { t: 0.0, value: 0.0 },
                ScalarKey { t: 1.0, value: 2.0 },
            ],
            interpolation: KeyframeInterp::Step,
        }
        .to_native();
        match native {
            NativeCurve::Keyframes {
                keys,
                interpolation,
            } => {
                assert_eq!(keys.len(), 2);
                assert_eq!(keys[1].1.scalar(), 2.0);
                assert!(matches!(interpolation, NativeInterp::Step));
            }
            other => panic!("期望 Keyframes，实际 {other:?}"),
        }
    }
}
