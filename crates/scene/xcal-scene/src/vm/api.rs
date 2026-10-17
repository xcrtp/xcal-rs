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
use crate::vm::types::{
    Arrow, Axes2D, Camera2D, Circle, Curve, Fill, Formula, PathList, PathShape, Reveal, RevealPath,
    Stroke, Text, Transform,
};
use xc_vm::Handle;
use xc_vm_macro::capability;

/// 宿主暴露给脚本的场景装配接口。
#[capability(
    namespace = "xcal.scene",
    version = "3",
    description = "脚本可见的场景装配接口（结构化组件 / 对象 / 动画）",
    bindings
)]
pub struct SceneApi<'a> {
    /// 构造圆组件。
    pub circle: fn(&'a dyn Vm, shape: Circle) -> Handle,

    /// 构造描边组件（颜色是结构体，不再是打包的 `u32`）。
    pub stroke: fn(&'a dyn Vm, style: Stroke) -> Handle,

    /// 构造 2D 相机取景组件。
    pub camera2d: fn(&'a dyn Vm, camera: Camera2D) -> Handle,

    /// 构造书写动画组件（点表是 `Array<Vec2>`）。
    pub reveal: fn(&'a dyn Vm, reveal: Reveal) -> Handle,

    /// 构造坐标网格组件（容器；几何由 `AxesRefreshSystem` 每帧生成）。
    pub axes2d: fn(&'a dyn Vm, axes: Axes2D) -> Handle,

    /// 构造箭头图元。
    pub arrow: fn(&'a dyn Vm, arrow: Arrow) -> Handle,

    /// 构造实心填充样式。
    pub fill: fn(&'a dyn Vm, fill: Fill) -> Handle,

    /// 构造逐对象摆放组件。
    pub transform: fn(&'a dyn Vm, transform: Transform) -> Handle,

    /// 由内容路径构造书写对象（`Reveal` + `StrokeSolid` 组件组）。
    pub reveal_path: fn(&'a dyn Vm, shape: RevealPath) -> Handle,

    /// 由内容路径构造静态对象（`Path` + `StrokeSolid` 组件组）。
    pub path_shape: fn(&'a dyn Vm, shape: PathShape) -> Handle,

    /// 创建对象：组件句柄列表（异构 ⇒ 保持 `Array<Opaque>`）。
    pub spawn: fn(&'a dyn Vm, components: Vec<Handle>) -> Handle,

    /// 销毁对象（`commit` 时执行）。
    pub despawn: fn(&'a dyn Vm, object: Handle) -> (),

    /// 构造动画动作：**目标枚举** + **曲线枚举**（拼错在绑定层即报错）。
    pub clip: fn(&'a dyn Vm, target: TargetKind, curve: Curve) -> Handle,

    /// 顺序播放一段动画（动作 + 时长，秒）。
    pub animate: fn(&'a dyn Vm, object: Handle, clip: Handle, duration: f64) -> (),

    /// 与**上一段**动画同区间的额外动作。
    pub parallel: fn(&'a dyn Vm, object: Handle, clip: Handle) -> (),

    /// 推进时序游标（不产生动画段）。
    pub wait: fn(&'a dyn Vm, seconds: f64) -> (),
}

/// 宿主暴露给脚本的内容编译接口。
///
/// 返回 `Array<Path>`（**按阅读顺序**）：脚本据此决定 spawn 几个对象；
/// "阅读顺序" 归宿主（内容服务）决定，不在脚本里复制排序语义。
#[capability(
    namespace = "xcal.content",
    version = "1",
    description = "脚本可见的内容编译接口（公式 / 文字 → 描边路径数组）",
    bindings
)]
pub struct ContentApi<'a> {
    /// 编译公式源码（Typst）为描边路径。
    pub formula: fn(&'a dyn Vm, spec: Formula) -> PathList,

    /// 编译纯文本为描边路径（正文外观）。
    pub text: fn(&'a dyn Vm, spec: Text) -> PathList,
}
