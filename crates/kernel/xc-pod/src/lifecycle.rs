//! 生命周期：**普通 capability**（不在 `Injector` 里），由宿主调用。
//!
//! 为什么是独立 capability：只有宿主知道何时销毁 / 重置 Pod；把它塞进 `Injector`
//! 会让"接受注入"与"接受销毁"绑在一起（纯函数 Pod 可能希望前者、不需要后者）。
//!
//! # 语义（宿主与 Pod 的约定）
//!
//! - [`Lifecycle::dispose`]：Pod 进入**终态**。宿主承诺之后不再调用它的函数；
//!   Pod 可以自行拒绝后续调用（返回错误）。dispose 是幂等的还是报错由 Pod 定义。
//! - [`Lifecycle::reset`]：Pod 回到初始状态（供重播 / 复用；与"脚本可重放"的
//!   记账直接相关）。reset 后 Pod **可继续使用**。
//!
//! 两者都由**宿主**调用，Pod 不会自己调。

use crate::capability::{Capability, CapabilityMeta, FnSig, Resolved};
use crate::error::PodError;
use crate::pod::Pod;

/// `Lifecycle` 占用的保留 namespace。
pub const LIFECYCLE_NAMESPACE: &str = "xc_pod::lifecycle";

type LifecycleFn = fn(&dyn Pod) -> Result<(), PodError>;

/// 生命周期端点（普通 capability，保留 namespace [`LIFECYCLE_NAMESPACE`]）。
///
/// Pod 侧注册：
///
/// ```text
/// xc_pod::lifecycle::dispose() -> LifecycleResult
/// xc_pod::lifecycle::reset()   -> LifecycleResult
/// ```
///
/// `LifecycleResult` = `Result<(), PodError>`。
pub struct Lifecycle<'a> {
    pod: &'a dyn Pod,
    dispose: LifecycleFn,
    reset: LifecycleFn,
}

impl<'a> Lifecycle<'a> {
    /// 让 Pod 进入终态（宿主承诺之后不再调用它）。
    pub fn dispose(&self) -> Result<(), PodError> {
        (self.dispose)(self.pod)
    }

    /// 让 Pod 回到初始状态（重播 / 复用）。
    pub fn reset(&self) -> Result<(), PodError> {
        (self.reset)(self.pod)
    }

    /// 句柄持有的目标 Pod。
    pub fn pod(&self) -> &'a dyn Pod {
        self.pod
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
            Some("Pod 生命周期（dispose / reset），由宿主调用"),
        )
    }

    fn from_negotiated(pod: &'a dyn Pod, resolved: &[Resolved<'_>]) -> Option<Self> {
        Some(Self {
            pod,
            dispose: *resolved.get(0)?.callable.downcast_ref::<LifecycleFn>()?,
            reset: *resolved.get(1)?.callable.downcast_ref::<LifecycleFn>()?,
        })
    }
}
