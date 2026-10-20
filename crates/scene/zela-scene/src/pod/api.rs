// api.rs — 宿主暴露给脚本的两组能力（**Injector 的两次真实使用**）。
//
// ```text
// scene   ← SceneApi   ：场景装配（组件 / 对象 / 动画）
// content ← ContentApi ：内容编译（公式 / 文字 → 描边路径数组）
// ```
//
// 两组能力都由 `#[capability(bindings)]` 生成 `calls()`；Lua 侧的 `scene.*` /
// `content.*` 表与 `Circle { ... }` / `Curve.Phase { ... }` 构造器由
// `binding.rs` 按 `TypeSpec` **自动生成**。

use crate::animation::TargetKind;
use crate::pod::types::{
    Arrow, Axes2D, Camera2D, Camera3D, Circle, Cube, Curve, Fill, Formula, Light, Material3D, Name,
    Opacity, Parent, PathList, PathShape, Polyline, Reveal, RevealPath, Stroke, Text, Transform,
    Transform3D, TransformChannel,
};
use ze_pod_core::Handle;
use ze_pod_macro::capability;

/// 宿主暴露给脚本的场景装配接口。
#[capability(
    namespace = "zela.scene",
    version = "5",
    description = "脚本可见的场景装配接口（结构化组件 / 对象 / 动画）",
    bindings
)]
pub struct SceneApi<'a> {
    /// 构造圆组件。
    pub circle: fn(&'a dyn Pod, shape: Circle) -> Handle,

    /// 构造描边组件（颜色是结构体，不再是打包的 `u32`）。
    pub stroke: fn(&'a dyn Pod, style: Stroke) -> Handle,

    /// 构造 2D 相机取景组件。
    pub camera2d: fn(&'a dyn Pod, camera: Camera2D) -> Handle,

    /// 构造 3D 相机组件（B1-D）。
    pub camera3d: fn(&'a dyn Pod, camera: Camera3D) -> Handle,

    /// 构造立方体网格组件（B1-D，**参数化**：只传半边长，顶点由宿主生成）。
    pub cube: fn(&'a dyn Pod, cube: Cube) -> Handle,

    /// 构造 3D 材质组件（B1-D；`metallic` / `roughness` 仅 Pbr 变体消费）。
    pub material3d: fn(&'a dyn Pod, material: Material3D) -> Handle,

    /// 构造光源组件（B1-D；**不带 position** —— 位置由同对象的 `Transform` 提供）。
    pub light: fn(&'a dyn Pod, light: Light) -> Handle,

    /// 构造 3D 摆放组件（B1-D；位置 / 四元数旋转 / 缩放，映射到原生 `Transform`）。
    pub transform3d: fn(&'a dyn Pod, transform: Transform3D) -> Handle,

    /// 构造书写动画组件（点表是 `Array<Vec2>`）。
    pub reveal: fn(&'a dyn Pod, reveal: Reveal) -> Handle,

    /// 构造**静态折线**（S2：可见初态，仍可继续被 RevealRange 书写）。
    pub polyline: fn(&'a dyn Pod, polyline: Polyline) -> Handle,

    /// 父对象引用组件（S5：声明期句柄，commit 时解析）。
    pub parent: fn(&'a dyn Pod, parent: Parent) -> Handle,

    /// 对象名组件。
    pub name: fn(&'a dyn Pod, name: Name) -> Handle,

    /// 对象级不透明度组件（S6）。
    pub opacity: fn(&'a dyn Pod, opacity: Opacity) -> Handle,

    /// 构造坐标网格组件（容器；几何由 `AxesRefreshSystem` 每帧生成）。
    pub axes2d: fn(&'a dyn Pod, axes: Axes2D) -> Handle,

    /// 构造箭头图元。
    pub arrow: fn(&'a dyn Pod, arrow: Arrow) -> Handle,

    /// 构造实心填充样式。
    pub fill: fn(&'a dyn Pod, fill: Fill) -> Handle,

    /// 构造逐对象摆放组件。
    pub transform: fn(&'a dyn Pod, transform: Transform) -> Handle,

    /// 由内容路径构造书写对象（`Reveal` + `StrokeSolid` 组件组）。
    pub reveal_path: fn(&'a dyn Pod, shape: RevealPath) -> Handle,

    /// 由内容路径构造静态对象（`Path` + `StrokeSolid` 组件组）。
    pub path_shape: fn(&'a dyn Pod, shape: PathShape) -> Handle,

    /// 创建对象：组件句柄列表（异构 ⇒ 保持 `Array<Opaque>`）。
    pub spawn: fn(&'a dyn Pod, components: Vec<Handle>) -> Handle,

    /// 销毁对象（`commit` 时执行）。
    pub despawn: fn(&'a dyn Pod, object: Handle) -> (),

    /// 构造动画动作：**目标枚举** + **曲线枚举**（拼错在绑定层即报错）。
    pub clip: fn(&'a dyn Pod, target: TargetKind, curve: Curve) -> Handle,

    /// 构造 **Transform 通道**动作（S3：Position / Rotation / Scale）。
    ///
    /// 与 `clip(Target.TransformValue, ...)` 的区别：显式指定写哪一列，
    /// 因此 `Position` 与 `Rotation` 可以在 `parallel` 里并存。
    pub clip_transform: fn(&'a dyn Pod, channel: TransformChannel, curve: Curve) -> Handle,

    /// 顺序播放一段动画（动作 + 时长，秒）。
    pub animate: fn(&'a dyn Pod, object: Handle, clip: Handle, duration: f64) -> (),

    /// 与**上一段**动画同区间的额外动作。
    pub parallel: fn(&'a dyn Pod, object: Handle, clip: Handle) -> (),

    /// 推进时序游标（不产生动画段）。
    pub wait: fn(&'a dyn Pod, seconds: f64) -> (),
}

/// 宿主暴露给脚本的内容编译接口。
///
/// 返回 `Array<Path>`（**按阅读顺序**）：脚本据此决定 spawn 几个对象；
/// "阅读顺序" 归宿主（内容服务）决定，不在脚本里复制排序语义。
#[capability(
    namespace = "zela.content",
    version = "1",
    description = "脚本可见的内容编译接口（公式 / 文字 → 描边路径数组）",
    bindings
)]
pub struct ContentApi<'a> {
    /// 编译公式源码（Typst）为描边路径。
    pub formula: fn(&'a dyn Pod, spec: Formula) -> PathList,

    /// 编译纯文本为描边路径（正文外观）。
    pub text: fn(&'a dyn Pod, spec: Text) -> PathList,
}
