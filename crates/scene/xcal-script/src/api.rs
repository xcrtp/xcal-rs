// api.rs — **宿主暴露给脚本**的最小场景接口（L1 → V4 结构化）。
//
// # 从"函数集"升级为"类型集"
//
// 旧接口用 `f32` 拼装形状、用 `u32` 打包颜色、用四个函数表示同一种曲线；
// 现在参数是**具名结构体 / 枚举**（[`crate::types`]），由 `TypeSpec` 驱动
// 自动生成 Lua 绑定与文档（见 `binding.rs` / `export.rs`）。
//
// # `bindings` 开关
//
// `#[capability(bindings)]` 让宏额外生成 `calls()`（类型擦除调用 + 形参名 + TypeSpec）。
// 未开启时 `calls()` 为空 —— 既有 capability 的字段类型不必实现 `VmType`。

use xc_vm::Handle;
use xc_vm_macro::capability;

use crate::types::{Camera2D, Circle, Curve, Reveal, Stroke};

/// 宿主暴露给脚本的场景装配接口。
///
/// 组件构造 = **单个结构体参数** ⇒ 绑定层自动生成 `Circle { ... }` / `Stroke { ... }`
/// 这类**类型构造器**（不再是 15 个手写闭包）。
#[capability(
    namespace = "xcal.scene",
    version = "2",
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

    /// 创建对象：组件句柄列表（异构 ⇒ 保持 `Array<Opaque>`）。
    #[vm_type(param0 = "ComponentList")]
    pub spawn: fn(&'a dyn Vm, components: Vec<Handle>) -> Handle,

    /// 销毁对象（`commit` 时执行）。
    pub despawn: fn(&'a dyn Vm, object: Handle) -> (),

    /// 构造动画动作：target 名字 + **曲线枚举**。
    pub clip: fn(&'a dyn Vm, target: String, curve: Curve) -> Handle,

    /// 顺序播放一段动画（动作 + 时长，秒）。
    pub animate: fn(&'a dyn Vm, object: Handle, clip: Handle, duration: f64) -> (),

    /// 与**上一段**动画同区间的额外动作。
    pub parallel: fn(&'a dyn Vm, object: Handle, clip: Handle) -> (),

    /// 推进时序游标（不产生动画段）。
    pub wait: fn(&'a dyn Vm, seconds: f64) -> (),
}
