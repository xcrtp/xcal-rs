// xc-pod: Pod 统一抽象（零依赖 kernel crate）。
//
// # 它是什么 / 不是什么
//
// **A Pod is not a virtual machine, and not a scripting-language runtime.**
//
// A Pod is an **opaque state machine plus a capability set**: its internal state is
// hidden, and callers can only use the function set obtained through capability
// negotiation. Large values cross the boundary as opaque handles.
//
// **Pod 不是虚拟机，也不是脚本语言运行时。**
// 它是「不透明状态机 + 能力集合」的抽象：内部状态不可见，只能通过协商得到的
// 函数集合访问；大类型跨边界只传句柄。Lua / Python 解释器、节点图求值器、
// 闭包表、未来的 HTTP 对端，都只是它的具体实现。
//
// 它不是"一门脚本语言的运行时"，而是把 **黑箱状态机 + 它的 capability + 它的函数集合**
// 统一到一套抽象下：
//
//   - 节点编辑器（图求值）= 一个 Pod
//   - Python / Rhai / Rune 脚本 = 一个 Pod
//   - 进程间通信对端 = 一个 Pod
//   - 闭包注册表 = 一个内置的小 Pod（[`RegistryPod`]）
//
// # 两级协商
//
// | 级 | 何时 | 开销 |
// |---|---|---|
// | 协商（[`Pod::negotiate`]） | 装配期 / 首次取用 | 一次（按完整函数名查表） |
// | 调用（capability 方法） | 每帧 | **一次间接寻址**（fn 指针调用） |
//
// capability 是**运行时协商出的接口**：宿主声明"需要这组函数"，Pod 回答"我有/没有"，
// 成功则得到实例化的 struct 句柄（`#[capability]` 宏生成）。
//
// # 数据边界两级
//
// - 小类型（`f32` / `i32` / `bool`…）按值传递 —— 快路径；
// - 大类型（路径 / 网格 / 纹理…）走不透明句柄（[`Handle`]），数据留在 Pod 内部。
//
// # 借用方案（§0.2.1 第 3 条）
//
// 快慢两路径都用 `&self`；有状态 Pod 自行用内部可变性（`RefCell` / `Cell` / 锁）。
// 状态归 Pod 自己管，外部不应看到"它在变"。
//
// # 双向对接（宿主 ⇄ Pod）
//
// 宿主 → Pod：[`Pod::negotiate`] / [`negotiate_dyn`] 拿到 capability 句柄并调用。
//
// Pod → 宿主：宿主把能力以 [`Service`]（`Arc<dyn Pod>`）为单位，经 [`Injector`]
// capability 注入 Pod。**默认拒绝**：Pod 不暴露 `Injector` ⇒ 协商返回 `None` ⇒ 天然沙箱。
// 生命周期（[`Lifecycle`]）是独立的普通 capability，由宿主调用。
//
// ## 重入约束（必须遵守）
//
// 双向对接后会出现：**宿主调 Pod → Pod 回调宿主**。约定：
//
// > **回调只做只读查询；要改变场景状态，Pod 产出命令（SceneScript），不直接调宿主改。**
//
// 理由：导出 / 重播时宿主侧能力可能不存在（headless 渲染没有 UI、没有实时时间）。
// 这条约束无法在类型层完全强制（回调仍是普通函数调用），故以文档 + 用例
// （只读查询）为准。
//
// # 零依赖
//
// 本 crate 只使用 std，且必须能编译到 `wasm32-unknown-unknown`。
// `#[capability]` 宏在独立的 `xc-pod-macro`（proc-macro 必须独立 crate）。
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
mod pod;

pub use broker::{AssemblyPlan, AssemblyReport, Broker, BrokerError, NodeSpec, PodNode};
pub use capability::{Capability, CapabilityMeta, FnDoc, FnSig, Resolved};
pub use error::PodError;
pub use handle::{Handle, HandleTable, PodId};
pub use inject::{
    INJECTOR_NAMESPACE, Injector, MissingRequirements, Service, ServiceRequirement,
    validate_requirements,
};
pub use lifecycle::{LIFECYCLE_NAMESPACE, Lifecycle};
pub use qualified::QualifiedName;
pub use registry::{ErasedCall, RegistryPod};
pub use type_name::{IntoPodTypeName, PodTypeName};
pub use typespec::{
    ErasedFn, Field, FnIo, HANDLE_SPEC, ParamSpec, ScalarKind, TypeSpec, UNIT_SPEC, Variant,
};
pub use value::{
    ScalarValue, PodType, PodValue, PodValueError, describe, expect_enum, expect_struct, field,
};
pub use pod::{FunctionId, Pod, negotiate_dyn};
