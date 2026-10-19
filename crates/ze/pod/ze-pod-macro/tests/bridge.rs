//! Y2–Y5 验收：dyn 侧协商、Injector（含 requirements / optional）、
//! 端到端"Pod 调宿主"、生命周期 capability。
//!
//! # 重入约束（用例演示）
//!
//! 本文件的 `plugin::tick` 是 **宿主调 Pod → Pod 回调宿主** 的完整链路，
//! 回调只读取 `host::now`（只读查询），不修改宿主状态。
//! 要改变场景状态时应由 Pod 产出命令（SceneScript），不得直接调宿主改 ——
//! 这条约定写在 `ze-pod-core` 的 crate 文档里；类型层无法完全强制（回调仍是普通调用）。
#![allow(dead_code)]

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::sync::Arc;

use ze_pod_core::{
    FunctionId, Injector, Lifecycle, QualifiedName, RegistryPod, Service, ServiceRequirement, Pod,
    PodError, PodId, negotiate_dyn, validate_requirements,
};
use ze_pod_macro::capability;

// ─── 宿主侧：只读时钟 Service ────────────────────────────────────────────

struct HostPod {
    funcs: RegistryPod,
    now: Cell<f64>,
}

impl Pod for HostPod {
    fn id(&self) -> PodId {
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
    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
        self.funcs.call(id, args)
    }
}

#[capability(namespace = "host")]
struct HostClock<'a> {
    now: fn(&'a dyn Pod) -> f64,
}

fn host(now: f64) -> Arc<HostPod> {
    let mut funcs = RegistryPod::new();
    funcs.register_fn0_named::<f64>("host::now", "f64", |pod: &dyn Pod| {
        pod.as_any()
            .downcast_ref::<HostPod>()
            .expect("调用者必须是 HostPod")
            .now
            .get()
    });
    Arc::new(HostPod {
        funcs,
        now: Cell::new(now),
    })
}

// ─── Pod 侧：接受注入 + 回调宿主 ─────────────────────────────────────────

struct PluginPod {
    funcs: RegistryPod,
    host: RefCell<Option<Arc<dyn Pod>>>,
    last_seen: Cell<f64>,
}

impl Pod for PluginPod {
    fn id(&self) -> PodId {
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
    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
        self.funcs.call(id, args)
    }
}

#[capability(namespace = "plugin")]
struct PluginApi<'a> {
    /// 推进 `dt` 秒，返回当前时刻。
    tick: fn(&'a dyn Pod, f32) -> f64,
    /// 上一次看到的时刻。
    last_seen: fn(&'a dyn Pod) -> f64,
}

fn plugin() -> Arc<PluginPod> {
    let mut funcs = RegistryPod::new();

    // ── Injector：Pod 自述需求 + 接收注入 ───────────────────────────────
    funcs.register_fn0_named::<Vec<ServiceRequirement>>(
        "ze_pod_core::injector::requirements",
        "ServiceRequirements",
        |_: &dyn Pod| {
            vec![ServiceRequirement::required(
                "host.clock",
                "host::now() -> f64",
            )]
        },
    );
    funcs.register_fn2_named::<String, Arc<dyn Pod>, Result<(), PodError>>(
        "ze_pod_core::injector::inject",
        "ServiceName",
        "ServicePod",
        "InjectResult",
        |pod: &dyn Pod, service: String, service_pod: Arc<dyn Pod>| {
            let plugin = pod.as_any().downcast_ref::<PluginPod>().expect("PluginPod");
            if service != "host.clock" {
                return Err(PodError::CallFailed(format!("未知 Service：{service}")));
            }
            *plugin.host.borrow_mut() = Some(service_pod);
            Ok(())
        },
    );

    // ── Pod 的业务函数：回调宿主（只读）────────────────────────────────
    funcs.register_fn1_named::<f32, f64>("plugin::tick", "f32", "f64", |pod: &dyn Pod, dt: f32| {
        let plugin = pod.as_any().downcast_ref::<PluginPod>().expect("PluginPod");
        let host = plugin
            .host
            .borrow()
            .clone()
            .expect("宿主能力尚未注入（宿主应先 validate + inject）");

        // Y2 的关键路径：`Arc<dyn Pod>` → `&dyn Pod` → 协商 → typed 句柄。
        let clock: HostClock<'_> =
            negotiate_dyn(&*host).expect("host 必须提供 HostClock（requirements 已声明）");
        let t = clock.now() + dt as f64; // 只读查询
        plugin.last_seen.set(t);
        t
    });
    funcs.register_fn0_named::<f64>("plugin::last_seen", "f64", |pod: &dyn Pod| {
        pod.as_any()
            .downcast_ref::<PluginPod>()
            .expect("PluginPod")
            .last_seen
            .get()
    });

    Arc::new(PluginPod {
        funcs,
        host: RefCell::new(None),
        last_seen: Cell::new(0.0),
    })
}

// ─── 生命周期 Pod ────────────────────────────────────────────────────────

struct LifePod {
    funcs: RegistryPod,
    alive: Cell<bool>,
    count: Cell<u32>,
}

impl Pod for LifePod {
    fn id(&self) -> PodId {
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
    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
        self.funcs.call(id, args)
    }
}

#[capability(namespace = "life")]
struct LifeApi<'a> {
    #[pod_type(ret = "Result<u32, PodError>")]
    bump: fn(&'a dyn Pod) -> Result<u32, PodError>,
}

fn life() -> LifePod {
    let mut funcs = RegistryPod::new();
    funcs.register_fn0_named::<Result<(), PodError>>(
        "ze_pod_core::lifecycle::dispose",
        "LifecycleResult",
        |pod: &dyn Pod| {
            pod.as_any()
                .downcast_ref::<LifePod>()
                .expect("LifePod")
                .alive
                .set(false);
            Ok(())
        },
    );
    funcs.register_fn0_named::<Result<(), PodError>>(
        "ze_pod_core::lifecycle::reset",
        "LifecycleResult",
        |pod: &dyn Pod| {
            let state = pod.as_any().downcast_ref::<LifePod>().expect("LifePod");
            state.alive.set(true);
            state.count.set(0);
            Ok(())
        },
    );
    funcs.register_fn0_named::<Result<u32, PodError>>(
        "life::bump",
        "Result<u32, PodError>",
        |pod: &dyn Pod| {
            let state = pod.as_any().downcast_ref::<LifePod>().expect("LifePod");
            if !state.alive.get() {
                return Err(PodError::CallFailed("Pod 已 dispose".to_string()));
            }
            let next = state.count.get() + 1;
            state.count.set(next);
            Ok(next)
        },
    );

    LifePod {
        funcs,
        alive: Cell::new(true),
        count: Cell::new(0),
    }
}

// ─── Y3 沙箱：不暴露 Injector ⇒ None ────────────────────────────────────

#[test]
fn pod_without_injector_is_a_sandbox() {
    let plain = RegistryPod::new();
    assert!(
        plain.negotiate::<Injector<'_>>().is_none(),
        "不暴露 Injector ⇒ 无法注入（默认拒绝）"
    );

    let as_dyn: &dyn Pod = &plain;
    assert!(
        negotiate_dyn::<Injector<'_>>(as_dyn).is_none(),
        "trait object 路径同样拒绝"
    );
}

// ─── Y2：Arc<dyn Pod> 可协商 ────────────────────────────────────────────

#[test]
fn dyn_negotiation_works_on_arc() {
    let host = host(42.0);
    let as_dyn: Arc<dyn Pod> = host.clone();

    let via_dyn: HostClock<'_> = negotiate_dyn(&*as_dyn).expect("Arc<dyn Pod> → &dyn Pod 可协商");
    let via_concrete: HostClock<'_> = host.negotiate().expect("具体类型协商");

    assert_eq!(via_dyn.now(), 42.0);
    assert_eq!(via_dyn.now(), via_concrete.now(), "两条入口行为必须一致");
}

// ─── Y3/Y4：requirements + 校验 + 注入 + 端到端回调 ─────────────────────

#[test]
fn injection_end_to_end_pod_calls_host() {
    let host = host(100.0);
    let plugin = plugin();

    let injector: Injector<'_> = plugin.negotiate().expect("PluginPod 暴露 Injector");
    let requirements = injector.requirements();
    assert_eq!(requirements.len(), 1);
    assert_eq!(requirements[0].service, "host.clock");
    assert!(requirements[0].required);

    let services = vec![Service::new("host.clock", host.clone())];
    validate_requirements(&requirements, &services).expect("必需项已满足");
    injector.inject_service(&services[0]).expect("注入成功");

    let api: PluginApi<'_> = plugin.negotiate().expect("plugin API");
    // 宿主调 Pod（tick）→ Pod 回调宿主（host::now，只读）。
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
    let empty: Arc<dyn Pod> = Arc::new(RegistryPod::new());
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
    let pod = life();
    let api: LifeApi<'_> = pod.negotiate().expect("业务 capability");
    assert_eq!(api.bump().unwrap(), 1);

    let lifecycle: Lifecycle<'_> = pod.negotiate().expect("暴露生命周期");
    lifecycle.dispose().expect("dispose 成功");

    let err = api.bump().expect_err("dispose 后 Pod 不可用");
    assert!(matches!(err, PodError::CallFailed(_)), "实际 {err:?}");

    lifecycle.reset().expect("reset 成功");
    assert_eq!(api.bump().unwrap(), 1, "reset 回到初始状态（计数清零）");
    assert_eq!(api.bump().unwrap(), 2);
}

#[test]
fn pod_without_lifecycle_returns_none() {
    let plain = RegistryPod::new();
    assert!(
        plain.negotiate::<Lifecycle<'_>>().is_none(),
        "未实现生命周期 ⇒ 协商 None（可缺省）"
    );
}
