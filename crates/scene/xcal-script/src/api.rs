// api.rs — **宿主暴露给脚本**的最小场景接口（L1）。
//
// # 为什么是 `#[capability]` struct
//
// 文档真源是**宿主侧 Service 的声明**：字段名 + 字段类型 + `///`。
// `#[capability]` 把它变成运行时协商出的 typed 句柄（`negotiate_dyn::<SceneApi>`），
// 同时由宏生成 `docs()` —— 写 Lua 的人不需要读 Rust 源码。
//
// # 参数个数为什么 ≤ 3
//
// `xc-vm` 的 `RegistryVm` 只提供 arity 0..=3 的 typed 注册（`ErasedCall` 同样
// 只覆盖到 3 参），4 参函数无法走 typed 快路径 ⇒ 协商会失败。故草案里的
// `animate(h, target, curve, duration)`（4 参）拆成：
//
// ```text
// clip(target, curve) -> 动作句柄   // 2 参
// animate(对象, 动作, 时长)         // 3 参
// parallel(对象, 动作)              // 2 参
// ```
//
// 组件 / 对象 / 曲线**全部走不透明句柄**（`xc_vm::Handle`）：新增一种组件只加一个
// 构造函数，不动 `spawn` 的签名。

use xc_vm::Handle;
use xc_vm_macro::capability;

/// 宿主暴露给脚本的场景装配接口。
///
/// 字段顺序 = `signatures()` / `docs()` 顺序 = 生成文档的顺序。
#[capability(
    namespace = "xcal.scene",
    version = "1",
    description = "脚本可见的场景装配接口（组件构造 / 对象 / 动画 / 曲线）"
)]
pub struct SceneApi<'a> {
    /// 构造圆组件：圆心 `(x, y)`、半径 `r`。
    pub circle: fn(&'a dyn Vm, f32, f32, f32) -> Handle,

    /// 构造描边样式组件：线宽 `width`、颜色 `rgba`（`0xRRGGBBAA` 打包）。
    pub stroke: fn(&'a dyn Vm, f32, u32) -> Handle,

    /// 构造 2D 相机取景组件：中心 `(cx, cy)`、缩放 `scale`（像素 / 世界单位）。
    pub camera2d: fn(&'a dyn Vm, f32, f32, f32) -> Handle,

    /// 构造书写动画组件：扁平点表 `[x0, y0, x1, y1, ...]` 与是否闭合。
    #[vm_type(param0 = "FloatList")]
    pub reveal: fn(&'a dyn Vm, Vec<f32>, bool) -> Handle,

    /// 创建对象：`components` 是组件句柄列表（按给定顺序装配），返回对象句柄。
    #[vm_type(param0 = "ComponentList")]
    pub spawn: fn(&'a dyn Vm, Vec<Handle>) -> Handle,

    /// 销毁对象（`commit` 时按对象句柄执行）。
    pub despawn: fn(&'a dyn Vm, Handle) -> (),

    /// 构造动画动作：target 名字（复用持久化的 `target_ctor_by_name` 表）+ 曲线句柄。
    pub clip: fn(&'a dyn Vm, String, Handle) -> Handle,

    /// 顺序播放一段动画（动作 + 时长，秒）。
    pub animate: fn(&'a dyn Vm, Handle, Handle, f64) -> (),

    /// 与**上一段**动画同区间的额外动作（按上一段的时长 / 时钟节点）。
    pub parallel: fn(&'a dyn Vm, Handle, Handle) -> (),

    /// 推进时序游标（不产生动画段）。
    pub wait: fn(&'a dyn Vm, f64) -> (),

    /// 构造 sin 曲线：`base + amp * sin(t * omega)`。
    pub sin: fn(&'a dyn Vm, f32, f32, f32) -> Handle,

    /// 构造相位曲线：`t * omega`（段内线性）。
    pub phase: fn(&'a dyn Vm, f32) -> Handle,

    /// 构造常量曲线。
    pub constant: fn(&'a dyn Vm, f32) -> Handle,

    /// 构造标量线性曲线：段内 `0 → from`、`dur → to`（`Keyframes`）。
    pub ramp: fn(&'a dyn Vm, f32, f32, f32) -> Handle,

    /// 构造 `Vec2` 线性曲线：扁平点表 `[from_x, from_y, to_x, to_y]` 与总时长 `dur`。
    #[vm_type(param0 = "FloatList")]
    pub ramp2: fn(&'a dyn Vm, Vec<f32>, f32) -> Handle,
}
