//! 反向能力：宿主把能力注入 VM（`Injector` / `Service`）。
//!
//! # 形状
//!
//! ```text
//! 宿主 ──negotiate::<Injector>()──> VM
//!         │
//!         ├─ None  ⇒ 该 VM 不接受注入（天然沙箱）
//!         └─ Some(injector)
//!               ├─ injector.requirements()  ⇒ [(Service 名, 函数名, required|optional)]
//!               └─ injector.inject(...)      ⇒ 按 Service 单位注入（Arc<dyn Vm>）
//! ```
//!
//! `Injector` **不是特殊机制**：它是一个普通 capability，只是占用保留 namespace
//! [`INJECTOR_NAMESPACE`]。协商方式、全有或全无、快路径全部照旧。
//!
//! # 默认拒绝
//!
//! VM 不暴露 `Injector` ⇒ `negotiate::<Injector>()` 返回 `None` ⇒ 宿主无法注入。
//! 权限靠"能不能拿到句柄"表达，而不是运行时检查（capability-based security）。
//!
//! # 生命周期不在本模块
//!
//! dispose / reset 是独立的普通 capability —— 见 [`crate::Lifecycle`]。

use std::fmt;
use std::sync::Arc;

use crate::capability::{Capability, CapabilityMeta, FnSig, Resolved};
use crate::error::VmError;
use crate::qualified::QualifiedName;
use crate::vm::Vm;

/// `Injector` 占用的保留 namespace。
pub const INJECTOR_NAMESPACE: &str = "xc_vm::injector";

/// VM 自述的一条宿主能力需求。
///
/// `service` 是宿主侧 Service 的逻辑分组名（如 `"host.clock"`）；
/// `function` 是该 Service 需提供的**完整函数名**（如 `"host::now() -> f64"`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceRequirement {
    /// Service 名（逻辑分组）。
    pub service: String,
    /// 需要的完整函数名。
    pub function: String,
    /// `true` = 必需；`false` = 可选（缺了不影响加载）。
    pub required: bool,
}

impl ServiceRequirement {
    /// 逐字段构造。
    pub fn new(service: &str, function: &str, required: bool) -> Self {
        Self {
            service: service.to_string(),
            function: function.to_string(),
            required,
        }
    }

    /// 必需项。
    pub fn required(service: &str, function: &str) -> Self {
        Self::new(service, function, true)
    }

    /// 可选项。
    pub fn optional(service: &str, function: &str) -> Self {
        Self::new(service, function, false)
    }

    /// 该需求是否被 `vm` 满足（完整函数名必须出现在其函数清单里）。
    pub fn is_satisfied_by(&self, vm: &dyn Vm) -> bool {
        match QualifiedName::parse(&self.function) {
            Ok(wanted) => vm.functions().iter().any(|f| *f == wanted),
            // 非法完整函数名永远无法满足 —— 校验时按"缺失"处理，而不是 panic。
            Err(_) => false,
        }
    }
}

/// 宿主侧的一个能力单位（按 Service 构造/注入的最小粒度）。
///
/// 这是**具体类型**而不是"纯逻辑分组名"：注入时宿主必须同时给出 Service 名与
/// 提供它的 VM 实例（`Arc<dyn Vm>`），否则 `inject` 无法把能力交给目标 VM。
#[derive(Clone)]
pub struct Service {
    name: String,
    vm: Arc<dyn Vm>,
}

impl Service {
    /// 构造一个 Service。
    pub fn new(name: impl Into<String>, vm: Arc<dyn Vm>) -> Self {
        Self {
            name: name.into(),
            vm,
        }
    }

    /// Service 名（与 [`ServiceRequirement::service`] 对应）。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 提供该 Service 的 VM。
    pub fn vm(&self) -> &Arc<dyn Vm> {
        &self.vm
    }
}

/// 装配期校验失败：缺失的**必需**项列表。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingRequirements {
    /// 缺失的必需项（optional 缺失不会出现在这里）。
    pub missing: Vec<ServiceRequirement>,
}

impl fmt::Display for MissingRequirements {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "缺少 {} 个必需的宿主能力：", self.missing.len())?;
        for (i, req) in self.missing.iter().enumerate() {
            if i > 0 {
                write!(f, "，")?;
            }
            write!(f, "{} / {}", req.service, req.function)?;
        }
        Ok(())
    }
}

impl std::error::Error for MissingRequirements {}

/// 装配期校验：宿主提供的 `services` 是否满足 VM 的 `requirements`。
///
/// - 全部满足 ⇒ `Ok(())`
/// - 缺**必需**项 ⇒ `Err(MissingRequirements)`（明确列表，不是静默失败）
/// - 只缺 optional ⇒ `Ok(())`
pub fn validate_requirements(
    requirements: &[ServiceRequirement],
    services: &[Service],
) -> Result<(), MissingRequirements> {
    let missing: Vec<ServiceRequirement> = requirements
        .iter()
        .filter(|req| req.required)
        .filter(|req| {
            let satisfied = services
                .iter()
                .find(|service| service.name == req.service)
                .is_some_and(|service| req.is_satisfied_by(service.vm.as_ref()));
            !satisfied
        })
        .cloned()
        .collect();

    if missing.is_empty() {
        Ok(())
    } else {
        Err(MissingRequirements { missing })
    }
}

type RequirementsFn = fn(&dyn Vm) -> Vec<ServiceRequirement>;
type InjectFn = fn(&dyn Vm, String, Arc<dyn Vm>) -> Result<(), VmError>;

/// 注入端点（普通 capability，保留 namespace [`INJECTOR_NAMESPACE`]）。
///
/// VM 侧注册两个函数即可接受注入：
///
/// ```text
/// xc_vm::injector::requirements() -> ServiceRequirements
/// xc_vm::injector::inject(ServiceName, ServiceVm) -> InjectResult
/// ```
///
/// `ServiceName` = `String`，`ServiceVm` = `Arc<dyn Vm>`，
/// `InjectResult` = `Result<(), VmError>`。
pub struct Injector<'a> {
    vm: &'a dyn Vm,
    requirements: RequirementsFn,
    inject: InjectFn,
}

impl<'a> Injector<'a> {
    /// VM 自述的宿主能力需求（装配期判定"该不该加载"）。
    pub fn requirements(&self) -> Vec<ServiceRequirement> {
        (self.requirements)(self.vm)
    }

    /// 按 Service 名注入一个宿主能力 VM。
    ///
    /// 宿主应先调用 [`validate_requirements`]；本方法只负责把实例交给 VM。
    pub fn inject(&self, service: &str, vm: Arc<dyn Vm>) -> Result<(), VmError> {
        (self.inject)(self.vm, service.to_string(), vm)
    }

    /// 注入一个 [`Service`]（`name` 与 `vm` 一起给出）。
    pub fn inject_service(&self, service: &Service) -> Result<(), VmError> {
        self.inject(service.name(), Arc::clone(&service.vm))
    }

    /// 句柄持有的目标 VM。
    pub fn vm(&self) -> &'a dyn Vm {
        self.vm
    }
}

impl<'a> Capability<'a> for Injector<'a> {
    fn signatures() -> &'static [FnSig] {
        const SIGS: &[FnSig] = &[
            FnSig::new(
                INJECTOR_NAMESPACE,
                "requirements",
                &[],
                "ServiceRequirements",
            ),
            FnSig::new(
                INJECTOR_NAMESPACE,
                "inject",
                &["ServiceName", "ServiceVm"],
                "InjectResult",
            ),
        ];
        SIGS
    }

    fn meta() -> CapabilityMeta {
        CapabilityMeta::new(
            Some("1"),
            Some("宿主能力注入端点（默认拒绝：不暴露即沙箱）"),
        )
    }

    fn from_negotiated(vm: &'a dyn Vm, resolved: &[Resolved<'_>]) -> Option<Self> {
        Some(Self {
            vm,
            requirements: *resolved.get(0)?.callable.downcast_ref::<RequirementsFn>()?,
            inject: *resolved.get(1)?.callable.downcast_ref::<InjectFn>()?,
        })
    }
}
