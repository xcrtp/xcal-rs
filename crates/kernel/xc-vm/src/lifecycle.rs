//! 生命周期：**普通 capability**（不在 `Injector` 里），由宿主调用。
//!
//! 为什么是独立 capability：只有宿主知道何时销毁 / 重置 VM；把它塞进 `Injector`
//! 会让"接受注入"与"接受销毁"绑在一起（纯函数 VM 可能希望前者、不需要后者）。
//!
//! # 语义（宿主与 VM 的约定）
//!
//! - [`Lifecycle::dispose`]：VM 进入**终态**。宿主承诺之后不再调用它的函数；
//!   VM 可以自行拒绝后续调用（返回错误）。dispose 是幂等的还是报错由 VM 定义。
//! - [`Lifecycle::reset`]：VM 回到初始状态（供重播 / 复用；与"脚本可重放"的
//!   记账直接相关）。reset 后 VM **可继续使用**。
//!
//! 两者都由**宿主**调用，VM 不会自己调。

use crate::capability::{Capability, CapabilityMeta, FnSig, Resolved};
use crate::error::VmError;
use crate::vm::Vm;

/// `Lifecycle` 占用的保留 namespace。
pub const LIFECYCLE_NAMESPACE: &str = "xc_vm::lifecycle";

type LifecycleFn = fn(&dyn Vm) -> Result<(), VmError>;

/// 生命周期端点（普通 capability，保留 namespace [`LIFECYCLE_NAMESPACE`]）。
///
/// VM 侧注册：
///
/// ```text
/// xc_vm::lifecycle::dispose() -> LifecycleResult
/// xc_vm::lifecycle::reset()   -> LifecycleResult
/// ```
///
/// `LifecycleResult` = `Result<(), VmError>`。
pub struct Lifecycle<'a> {
    vm: &'a dyn Vm,
    dispose: LifecycleFn,
    reset: LifecycleFn,
}

impl<'a> Lifecycle<'a> {
    /// 让 VM 进入终态（宿主承诺之后不再调用它）。
    pub fn dispose(&self) -> Result<(), VmError> {
        (self.dispose)(self.vm)
    }

    /// 让 VM 回到初始状态（重播 / 复用）。
    pub fn reset(&self) -> Result<(), VmError> {
        (self.reset)(self.vm)
    }

    /// 句柄持有的目标 VM。
    pub fn vm(&self) -> &'a dyn Vm {
        self.vm
    }
}

impl<'a> Capability<'a> for Lifecycle<'a> {
    fn signatures() -> &'static [FnSig] {
        const SIGS: &[FnSig] = &[
            FnSig::new(LIFECYCLE_NAMESPACE, "dispose", &[], "LifecycleResult"),
            FnSig::new(LIFECYCLE_NAMESPACE, "reset", &[], "LifecycleResult"),
        ];
        SIGS
    }

    fn meta() -> CapabilityMeta {
        CapabilityMeta::new(
            Some("1"),
            Some("VM 生命周期（dispose / reset），由宿主调用"),
        )
    }

    fn from_negotiated(vm: &'a dyn Vm, resolved: &[Resolved<'_>]) -> Option<Self> {
        Some(Self {
            vm,
            dispose: *resolved.get(0)?.callable.downcast_ref::<LifecycleFn>()?,
            reset: *resolved.get(1)?.callable.downcast_ref::<LifecycleFn>()?,
        })
    }
}
