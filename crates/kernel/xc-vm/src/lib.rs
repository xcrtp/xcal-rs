// xc-vm: VM 统一抽象（零依赖 kernel crate）。
//
// 目标不是"引入一门脚本语言"，而是把 **黑箱状态机 + 它的 capability + 它的函数集合**
// 统一到一套抽象下：
//
//   - 节点编辑器（图求值）= 一个 VM
//   - Python / Rhai / Rune 脚本 = 一个 VM
//   - 进程间通信对端 = 一个 VM
//   - 闭包注册表 = 一个内置的小 VM（[`RegistryVm`]）
//
// # 两级协商
//
// | 级 | 何时 | 开销 |
// |---|---|---|
// | 协商（[`Vm::negotiate`]） | 装配期 / 首次取用 | 一次（按完整函数名查表） |
// | 调用（capability 方法） | 每帧 | **一次间接寻址**（fn 指针调用） |
//
// capability 是**运行时协商出的接口**：宿主声明"需要这组函数"，VM 回答"我有/没有"，
// 成功则得到实例化的 struct 句柄（`#[capability]` 宏生成）。
//
// # 数据边界两级
//
// - 小类型（`f32` / `i32` / `bool`…）按值传递 —— 快路径；
// - 大类型（路径 / 网格 / 纹理…）走不透明句柄（[`Handle`]），数据留在 VM 内部。
//
// # 借用方案（§0.2.1 第 3 条）
//
// 快慢两路径都用 `&self`；有状态 VM 自行用内部可变性（`RefCell` / `Cell` / 锁）。
// 状态归 VM 自己管，外部不应看到"它在变"。
//
// # 双向对接（宿主 ⇄ VM）
//
// 宿主 → VM：[`Vm::negotiate`] / [`negotiate_dyn`] 拿到 capability 句柄并调用。
//
// VM → 宿主：宿主把能力以 [`Service`]（`Arc<dyn Vm>`）为单位，经 [`Injector`]
// capability 注入 VM。**默认拒绝**：VM 不暴露 `Injector` ⇒ 协商返回 `None` ⇒ 天然沙箱。
// 生命周期（[`Lifecycle`]）是独立的普通 capability，由宿主调用。
//
// ## 重入约束（必须遵守）
//
// 双向对接后会出现：**宿主调 VM → VM 回调宿主**。约定：
//
// > **回调只做只读查询；要改变场景状态，VM 产出命令（SceneScript），不直接调宿主改。**
//
// 理由：导出 / 重播时宿主侧能力可能不存在（headless 渲染没有 UI、没有实时时间）。
// 这条约束无法在类型层完全强制（回调仍是普通函数调用），故以文档 + 用例
// （只读查询）为准。
//
// # 零依赖
//
// 本 crate 只使用 std，且必须能编译到 `wasm32-unknown-unknown`。
// `#[capability]` 宏在独立的 `xc-vm-macro`（proc-macro 必须独立 crate）。
#![forbid(unsafe_code)]

mod broker;
mod capability;
mod error;
mod handle;
mod inject;
mod lifecycle;
mod qualified;
mod registry;
mod type_name;
mod typespec;
mod value;
mod vm;

pub use broker::{AssemblyPlan, AssemblyReport, Broker, BrokerError, NodeSpec, VmNode};
pub use capability::{Capability, CapabilityMeta, FnDoc, FnSig, Resolved};
pub use error::VmError;
pub use handle::{Handle, HandleTable, VmId};
pub use inject::{
    INJECTOR_NAMESPACE, Injector, MissingRequirements, Service, ServiceRequirement,
    validate_requirements,
};
pub use lifecycle::{LIFECYCLE_NAMESPACE, Lifecycle};
pub use qualified::QualifiedName;
pub use registry::{ErasedCall, RegistryVm};
pub use type_name::{IntoVmTypeName, VmTypeName};
pub use typespec::{
    ErasedFn, Field, FnIo, HANDLE_SPEC, ParamSpec, ScalarKind, TypeSpec, UNIT_SPEC, Variant,
};
pub use value::{
    ScalarValue, VmType, VmValue, VmValueError, describe, expect_enum, expect_struct, field,
};
pub use vm::{FunctionId, Vm, negotiate_dyn};
