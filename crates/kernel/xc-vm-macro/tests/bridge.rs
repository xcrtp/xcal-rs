//! Y2–Y5 验收：dyn 侧协商、Injector（含 requirements / optional）、
//! 端到端"VM 调宿主"、生命周期 capability。
//!
//! # 重入约束（用例演示）
//!
//! 本文件的 `plugin::tick` 是 **宿主调 VM → VM 回调宿主** 的完整链路，
//! 回调只读取 `host::now`（只读查询），不修改宿主状态。
//! 要改变场景状态时应由 VM 产出命令（SceneScript），不得直接调宿主改 ——
//! 这条约定写在 `xc-vm` 的 crate 文档里；类型层无法完全强制（回调仍是普通调用）。
#![allow(dead_code)]

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::sync::Arc;

use xc_vm::{
    FunctionId, Injector, Lifecycle, QualifiedName, RegistryVm, Service, ServiceRequirement, Vm,
    VmError, VmId, negotiate_dyn, validate_requirements,
};
use xc_vm_macro::capability;

// ─── 宿主侧：只读时钟 Service ────────────────────────────────────────────

struct HostVm {
    funcs: RegistryVm,
    now: Cell<f64>,
}

impl Vm for HostVm {
    fn id(&self) -> VmId {
        self.funcs.id()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn functions(&self) -> &[QualifiedName] {
        self.funcs.functions()
    }
    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)> {
        self.funcs.resolve(name)
    }
    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, VmError> {
        self.funcs.call(id, args)
    }
}

#[capability(namespace = "host")]
struct HostClock<'a> {
    now: fn(&'a dyn Vm) -> f64,
}

fn host(now: f64) -> Arc<HostVm> {
    let mut funcs = RegistryVm::new();
    funcs.register_fn0_named::<f64>("host::now", "f64", |vm: &dyn Vm| {
        vm.as_any()
            .downcast_ref::<HostVm>()
            .expect("调用者必须是 HostVm")
            .now
            .get()
    });
    Arc::new(HostVm {
        funcs,
        now: Cell::new(now),
    })
}

// ─── VM 侧：接受注入 + 回调宿主 ─────────────────────────────────────────

struct PluginVm {
    funcs: RegistryVm,
    host: RefCell<Option<Arc<dyn Vm>>>,
    last_seen: Cell<f64>,
}

impl Vm for PluginVm {
    fn id(&self) -> VmId {
        self.funcs.id()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn functions(&self) -> &[QualifiedName] {
        self.funcs.functions()
    }
    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)> {
        self.funcs.resolve(name)
    }
    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, VmError> {
        self.funcs.call(id, args)
    }
}

#[capability(namespace = "plugin")]
struct PluginApi<'a> {
    tick: fn(&'a dyn Vm, f32) -> f64,
    last_seen: fn(&'a dyn Vm) -> f64,
}

fn plugin() -> Arc<PluginVm> {
    let mut funcs = RegistryVm::new();

    // ── Injector：VM 自述需求 + 接收注入 ───────────────────────────────
    funcs.register_fn0_named::<Vec<ServiceRequirement>>(
        "xc_vm::injector::requirements",
        "ServiceRequirements",
        |_: &dyn Vm| {
            vec![ServiceRequirement::required(
                "host.clock",
                "host::now() -> f64",
            )]
        },
    );
    funcs.register_fn2_named::<String, Arc<dyn Vm>, Result<(), VmError>>(
        "xc_vm::injector::inject",
        "ServiceName",
        "ServiceVm",
        "InjectResult",
        |vm: &dyn Vm, service: String, service_vm: Arc<dyn Vm>| {
            let plugin = vm.as_any().downcast_ref::<PluginVm>().expect("PluginVm");
            if service != "host.clock" {
                return Err(VmError::CallFailed(format!("未知 Service：{service}")));
            }
            *plugin.host.borrow_mut() = Some(service_vm);
            Ok(())
        },
    );

    // ── VM 的业务函数：回调宿主（只读）────────────────────────────────
    funcs.register_fn1_named::<f32, f64>("plugin::tick", "f32", "f64", |vm: &dyn Vm, dt: f32| {
        let plugin = vm.as_any().downcast_ref::<PluginVm>().expect("PluginVm");
        let host = plugin
            .host
            .borrow()
            .clone()
            .expect("宿主能力尚未注入（宿主应先 validate + inject）");

        // Y2 的关键路径：`Arc<dyn Vm>` → `&dyn Vm` → 协商 → typed 句柄。
        let clock: HostClock<'_> =
            negotiate_dyn(&*host).expect("host 必须提供 HostClock（requirements 已声明）");
        let t = clock.now() + dt as f64; // 只读查询
        plugin.last_seen.set(t);
        t
    });
    funcs.register_fn0_named::<f64>("plugin::last_seen", "f64", |vm: &dyn Vm| {
        vm.as_any()
            .downcast_ref::<PluginVm>()
            .expect("PluginVm")
            .last_seen
            .get()
    });

    Arc::new(PluginVm {
        funcs,
        host: RefCell::new(None),
        last_seen: Cell::new(0.0),
    })
}

// ─── 生命周期 VM ────────────────────────────────────────────────────────

struct LifeVm {
    funcs: RegistryVm,
    alive: Cell<bool>,
    count: Cell<u32>,
}

impl Vm for LifeVm {
    fn id(&self) -> VmId {
        self.funcs.id()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn functions(&self) -> &[QualifiedName] {
        self.funcs.functions()
    }
    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)> {
        self.funcs.resolve(name)
    }
    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, VmError> {
        self.funcs.call(id, args)
    }
}

#[capability(namespace = "life")]
struct LifeApi<'a> {
    #[vm_type(ret = "Result<u32, VmError>")]
    bump: fn(&'a dyn Vm) -> Result<u32, VmError>,
}

fn life() -> LifeVm {
    let mut funcs = RegistryVm::new();
    funcs.register_fn0_named::<Result<(), VmError>>(
        "xc_vm::lifecycle::dispose",
        "LifecycleResult",
        |vm: &dyn Vm| {
            vm.as_any()
                .downcast_ref::<LifeVm>()
                .expect("LifeVm")
                .alive
                .set(false);
            Ok(())
        },
    );
    funcs.register_fn0_named::<Result<(), VmError>>(
        "xc_vm::lifecycle::reset",
        "LifecycleResult",
        |vm: &dyn Vm| {
            let state = vm.as_any().downcast_ref::<LifeVm>().expect("LifeVm");
            state.alive.set(true);
            state.count.set(0);
            Ok(())
        },
    );
    funcs.register_fn0_named::<Result<u32, VmError>>(
        "life::bump",
        "Result<u32, VmError>",
        |vm: &dyn Vm| {
            let state = vm.as_any().downcast_ref::<LifeVm>().expect("LifeVm");
            if !state.alive.get() {
                return Err(VmError::CallFailed("VM 已 dispose".to_string()));
            }
            let next = state.count.get() + 1;
            state.count.set(next);
            Ok(next)
        },
    );

    LifeVm {
        funcs,
        alive: Cell::new(true),
        count: Cell::new(0),
    }
}

// ─── Y3 沙箱：不暴露 Injector ⇒ None ────────────────────────────────────

#[test]
fn vm_without_injector_is_a_sandbox() {
    let plain = RegistryVm::new();
    assert!(
        plain.negotiate::<Injector<'_>>().is_none(),
        "不暴露 Injector ⇒ 无法注入（默认拒绝）"
    );

    let as_dyn: &dyn Vm = &plain;
    assert!(
        negotiate_dyn::<Injector<'_>>(as_dyn).is_none(),
        "trait object 路径同样拒绝"
    );
}

// ─── Y2：Arc<dyn Vm> 可协商 ────────────────────────────────────────────

#[test]
fn dyn_negotiation_works_on_arc() {
    let host = host(42.0);
    let as_dyn: Arc<dyn Vm> = host.clone();

    let via_dyn: HostClock<'_> = negotiate_dyn(&*as_dyn).expect("Arc<dyn Vm> → &dyn Vm 可协商");
    let via_concrete: HostClock<'_> = host.negotiate().expect("具体类型协商");

    assert_eq!(via_dyn.now(), 42.0);
    assert_eq!(via_dyn.now(), via_concrete.now(), "两条入口行为必须一致");
}

// ─── Y3/Y4：requirements + 校验 + 注入 + 端到端回调 ─────────────────────

#[test]
fn injection_end_to_end_vm_calls_host() {
    let host = host(100.0);
    let plugin = plugin();

    let injector: Injector<'_> = plugin.negotiate().expect("PluginVm 暴露 Injector");
    let requirements = injector.requirements();
    assert_eq!(requirements.len(), 1);
    assert_eq!(requirements[0].service, "host.clock");
    assert!(requirements[0].required);

    let services = vec![Service::new("host.clock", host.clone())];
    validate_requirements(&requirements, &services).expect("必需项已满足");
    injector.inject_service(&services[0]).expect("注入成功");

    let api: PluginApi<'_> = plugin.negotiate().expect("plugin API");
    // 宿主调 VM（tick）→ VM 回调宿主（host::now，只读）。
    assert_eq!(api.tick(0.5), 100.5);
    assert_eq!(api.last_seen(), 100.5);
    assert_eq!(api.tick(1.0), 101.0, "每次都读到宿主当前值");
}

#[test]
fn missing_required_is_an_explicit_error() {
    let plugin = plugin();
    let injector: Injector<'_> = plugin.negotiate().expect("暴露 Injector");
    let requirements = injector.requirements();

    // 一个 Service 都没提供。
    let err = validate_requirements(&requirements, &[]).expect_err("缺必需项必须失败");
    assert_eq!(err.missing.len(), 1);
    assert_eq!(err.missing[0].service, "host.clock");
    assert!(
        err.to_string().contains("host.clock"),
        "错误信息必须点名缺失项：{err}"
    );

    // Service 名对，但它没有声明的函数 ⇒ 仍算缺失。
    let empty: Arc<dyn Vm> = Arc::new(RegistryVm::new());
    let services = vec![Service::new("host.clock", empty)];
    assert!(
        validate_requirements(&requirements, &services).is_err(),
        "函数不在 Service 的清单里 ⇒ 不算满足"
    );
}

#[test]
fn optional_missing_does_not_fail() {
    let only_optional = vec![ServiceRequirement::optional(
        "host.debug",
        "host::debug() -> ()",
    )];
    validate_requirements(&only_optional, &[]).expect("optional 缺失不失败");

    let mixed = vec![
        ServiceRequirement::required("host.clock", "host::now() -> f64"),
        ServiceRequirement::optional("host.debug", "host::debug() -> ()"),
    ];
    let host = host(1.0);
    let services = vec![Service::new("host.clock", host)];
    validate_requirements(&mixed, &services).expect("只缺 optional ⇒ Ok");
}

// ─── Y5：生命周期 capability ───────────────────────────────────────────

#[test]
fn lifecycle_dispose_and_reset() {
    let vm = life();
    let api: LifeApi<'_> = vm.negotiate().expect("业务 capability");
    assert_eq!(api.bump().unwrap(), 1);

    let lifecycle: Lifecycle<'_> = vm.negotiate().expect("暴露生命周期");
    lifecycle.dispose().expect("dispose 成功");

    let err = api.bump().expect_err("dispose 后 VM 不可用");
    assert!(matches!(err, VmError::CallFailed(_)), "实际 {err:?}");

    lifecycle.reset().expect("reset 成功");
    assert_eq!(api.bump().unwrap(), 1, "reset 回到初始状态（计数清零）");
    assert_eq!(api.bump().unwrap(), 2);
}

#[test]
fn vm_without_lifecycle_returns_none() {
    let plain = RegistryVm::new();
    assert!(
        plain.negotiate::<Lifecycle<'_>>().is_none(),
        "未实现生命周期 ⇒ 协商 None（可缺省）"
    );
}
