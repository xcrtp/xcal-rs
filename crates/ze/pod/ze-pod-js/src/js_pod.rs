// js_pod.rs — 第二个**非原生 Pod**（JS，解释器是纯 Rust 的 boa）。
//
// # 两个方向
//
// ```text
// 脚本 → 宿主：装配期。两个 Service（scene / content）经 Injector 注入；
//              JS 侧 `scene.*` / `content.*` 与类型构造器由 `calls()` 自动生成。
// 宿主 → 脚本：Pod::call("zela.script::run", [source])（erased 慢路径）。
// ```
//
// # 为什么是 boa（B3-P §一）
//
// `deno_core` 依赖 V8（C++），编不到 `wasm32-unknown-unknown` ⇒ 无法满足
// "native 与 wasm 跑同一个 JS 引擎实现"。boa 是纯 Rust，两边同一个实现。
//
// # 线程安全徽章
//
// `boa_engine::Context` 是 `!Send + !Sync`（实测见 `tests/send_probe.rs`）⇒
// `JsPod` 与 `LuaPod` 一样登记为**非 sync** 节点（`PodNode::new`）。

use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use boa_engine::{Context, JsString, Source};
use ze_pod_core::{Capability, FunctionId, Pod, PodError, PodId, QualifiedName, ServiceRequirement};

use crate::binding::HostState;

/// `Injector` 的两个函数（与 `ze_pod_core::Injector::signatures()` 逐字一致）。
pub const REQUIREMENTS_PATH: &str = "ze_pod_core::injector::requirements";
pub const INJECT_PATH: &str = "ze_pod_core::injector::inject";
/// 宿主 → 脚本的唯一入口（erased）。
pub const RUN_PATH: &str = "zela.script::run";
/// `run` 的完整函数名（含参数 / 返回类型；`Broker::run_with` 用）。
pub const RUN_ENTRY: &str = "zela.script::run(String) -> Any";
/// 入口 Service 的约定名（解释器与"场景录制器"之间的协议，不是 crate 依赖）。
pub const ENTRY_SERVICE: &str = "scene";
/// 入口 Service 的内部交接函数（不是脚本可见 API）。
pub const RESET_PATH: &str = "zela.scene::__reset";
pub const FINISH_PATH: &str = "zela.scene::__finish";

type RequirementsFn = fn(&dyn Pod) -> Vec<ServiceRequirement>;
type InjectFn = fn(&dyn Pod, String, Arc<dyn Pod>) -> Result<(), PodError>;
type RunFn = fn(&dyn Pod, String) -> Result<Box<dyn Any>, PodError>;

/// Pod 的可变状态（`Pod` 只有 `&self`，故用内部可变性）。
struct ScriptState {
    context: Context,
    /// 场景装配 Service（`run` 的内部交接需要它）。
    scene: Option<Arc<dyn Pod>>,
}

/// 一个已注册能力的注入端点。
struct BinderEntry {
    /// 协商 + 建语言侧命名空间。
    bind: fn(&mut Context, &str, Arc<dyn Pod>) -> Result<(), PodError>,
    /// 自述需求（`required` 由注册时决定）。
    requirements: Vec<ServiceRequirement>,
}

/// JS 脚本 Pod。
///
/// 字段结构与 `LuaPod` 一一对应（`ScriptState` + `BinderEntry`），只有解释器类型不同。
pub struct JsPod {
    id: PodId,
    names: Vec<QualifiedName>,
    state: RefCell<ScriptState>,
    /// Service 名 → 能力绑定器（**不写死任何 domain 类型**）。
    binders: RefCell<BTreeMap<String, BinderEntry>>,
    requirements_fn: RequirementsFn,
    inject_fn: InjectFn,
    run_fn: RunFn,
}

impl Default for JsPod {
    fn default() -> Self {
        Self::new()
    }
}

impl JsPod {
    /// 新建脚本 Pod（尚未注入任何宿主能力）。
    pub fn new() -> Self {
        let names = vec![
            QualifiedName::from_path(REQUIREMENTS_PATH, vec![], "ServiceRequirements"),
            QualifiedName::from_path(
                INJECT_PATH,
                vec!["ServiceName".to_string(), "ServicePod".to_string()],
                "InjectResult",
            ),
            QualifiedName::from_path(RUN_PATH, vec!["String".to_string()], "Any"),
        ];
        let mut context = Context::default();
        // 宿主侧状态（Service 表 + 令牌表）必须在使用前就位。
        context.insert_data(HostState::new());
        Self {
            id: PodId::next(),
            names,
            state: RefCell::new(ScriptState {
                context,
                scene: None,
            }),
            binders: RefCell::new(BTreeMap::new()),
            requirements_fn: requirements_impl,
            inject_fn: inject_impl,
            run_fn: run_impl,
        }
    }

    /// 注册一个能力的绑定器（必须在 Broker 装配前调用）。
    ///
    /// `required = false` ⇒ 该 Service 是 **optional**：没有提供者也能装配，
    /// 只是 JS 全局对象上不会出现对应的命名空间。
    pub fn register_capability<C: Capability<'static>>(&self, service: &str, required: bool) {
        let requirements = C::signatures()
            .iter()
            .map(|sig| ServiceRequirement::new(service, &sig.qualified().to_string(), required))
            .collect();
        self.binders.borrow_mut().insert(
            service.to_string(),
            BinderEntry {
                bind: bind_capability::<C>,
                requirements,
            },
        );
    }

    /// 当前 JS 全局对象上某个命名空间注册的函数名（排序后）。
    ///
    /// 一致性护栏用例使用：**实际注册名** vs `SceneApi::signatures()` 的名字集合。
    pub fn registered_global_names(&self, global: &str) -> Vec<String> {
        let mut state = self.state.borrow_mut();
        let Ok(value) = state
            .context
            .global_object()
            .get(JsString::from(global), &mut state.context)
        else {
            return Vec::new();
        };
        let Some(object) = value.as_object() else {
            return Vec::new();
        };
        let Ok(keys) = object.own_property_keys(&mut state.context) else {
            return Vec::new();
        };
        let mut names: Vec<String> = keys
            .iter()
            .filter_map(|key| match key {
                boa_engine::property::PropertyKey::String(text) => {
                    Some(text.to_std_string_escaped())
                }
                boa_engine::property::PropertyKey::Index(index) => Some(index.get().to_string()),
                boa_engine::property::PropertyKey::Symbol(_) => None,
            })
            .collect();
        names.sort();
        names
    }

    /// 宿主 → 脚本：执行一段 JS 源码，取回入口 Service 的产物 `T`。
    pub fn run<T: 'static>(&self, source: &str) -> Result<T, PodError> {
        let id = self
            .function_id(&QualifiedName::from_path(
                RUN_PATH,
                vec!["String".to_string()],
                "Any",
            ))
            .ok_or_else(|| PodError::UnknownFunction(RUN_PATH.to_string()))?;
        let owned = source.to_string();
        let arg: &dyn Any = &owned;
        let out = self.call(id, &[arg])?;
        out.downcast::<T>().map(|boxed| *boxed).map_err(|_| {
            PodError::CallFailed(format!("run 的返回值不是 `{}`", std::any::type_name::<T>()))
        })
    }
}

impl Pod for JsPod {
    fn id(&self) -> PodId {
        self.id
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn functions(&self) -> &[QualifiedName] {
        &self.names
    }

    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)> {
        if *name == self.names[0] {
            Some(&self.requirements_fn)
        } else if *name == self.names[1] {
            Some(&self.inject_fn)
        } else if *name == self.names[2] {
            Some(&self.run_fn)
        } else {
            None
        }
    }

    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
        match id.get() {
            0 => Ok(Box::new((self.requirements_fn)(self))),
            1 => {
                let service = arg::<String>(args, 0)?.clone();
                let pod = Arc::clone(arg::<Arc<dyn Pod>>(args, 1)?);
                (self.inject_fn)(self, service, pod)?;
                Ok(Box::new(()))
            }
            2 => {
                let source = arg::<String>(args, 0)?.clone();
                (self.run_fn)(self, source)
            }
            _ => Err(PodError::UnknownFunction(format!("函数编号 {id}"))),
        }
    }
}

fn arg<'a, T: 'static>(args: &'a [&'a dyn Any], index: usize) -> Result<&'a T, PodError> {
    args.get(index)
        .and_then(|a| a.downcast_ref::<T>())
        .ok_or_else(|| PodError::CallFailed(format!("第 {index} 个实参类型不匹配")))
}

fn js_pod(pod: &dyn Pod) -> &JsPod {
    pod.as_any()
        .downcast_ref::<JsPod>()
        .expect("JsPod 的函数只能由 JsPod 自己调用")
}

// ─── Injector：Pod 自述需求 + 接收注入 ───────────────────────────────────

/// 一个能力的语言侧绑定器（泛型在注册时单态化，运行时只留 fn 指针）。
fn bind_capability<C: Capability<'static>>(
    ctx: &mut Context,
    global: &str,
    pod: Arc<dyn Pod>,
) -> Result<(), PodError> {
    // Broker 已在装配期做函数级校验；这里保留手动注入的最后一道闸。
    let missing: Vec<&str> = C::signatures()
        .iter()
        .filter(|sig| !pod.functions().contains(&sig.qualified()))
        .map(|sig| sig.name)
        .collect();
    if !missing.is_empty() {
        return Err(PodError::CallFailed(format!(
            "`{global}` 缺少函数：{}",
            missing.join(", ")
        )));
    }
    crate::binding::bind::<C>(ctx, global, pod)
}

fn requirements_impl(pod: &dyn Pod) -> Vec<ServiceRequirement> {
    let this = js_pod(pod);
    this.binders
        .borrow()
        .values()
        .flat_map(|entry| entry.requirements.iter().cloned())
        .collect()
}

fn inject_impl(pod: &dyn Pod, service: String, service_pod: Arc<dyn Pod>) -> Result<(), PodError> {
    let this = js_pod(pod);
    let binder = this.binders.borrow().get(&service).map(|entry| entry.bind);
    let Some(binder) = binder else {
        return Err(PodError::CallFailed(format!(
            "未知 Service `{service}`（能力未注册；先调用 `JsPod::register_capability`）"
        )));
    };
    let mut state = this.state.borrow_mut();
    binder(&mut state.context, &service, Arc::clone(&service_pod))?;
    if service == ENTRY_SERVICE {
        state.scene = Some(service_pod);
    }
    Ok(())
}

// ─── 宿主 → 脚本：run ──────────────────────────────────────────────────

fn run_impl(pod: &dyn Pod, source: String) -> Result<Box<dyn Any>, PodError> {
    let this = js_pod(pod);
    let mut state = this.state.borrow_mut();
    let scene = state.scene.clone().ok_or_else(|| {
        PodError::CallFailed("脚本 Pod 尚未注入入口服务（先经 Injector 注入 scene）".to_string())
    })?;

    scene.call(function_id(&scene, RESET_PATH, "()")?, &[])?;

    state
        .context
        .eval(Source::from_bytes(&source))
        .map_err(|e| PodError::CallFailed(format!("JS 执行失败：{e}")))?;
    // Promise 任务（若有）在装配期一次跑完，避免残留任务影响确定性。
    state
        .context
        .run_jobs()
        .map_err(|e| PodError::CallFailed(format!("JS 任务执行失败：{e}")))?;

    // 入口服务把产物装箱返回：`Result<Box<dyn Any>, String>`。
    let out = scene.call(
        function_id(&scene, FINISH_PATH, "Result<Box<dyn Any>, String>")?,
        &[],
    )?;
    match out.downcast::<Result<Box<dyn Any>, String>>() {
        Ok(result) => match *result {
            Ok(value) => Ok(value),
            Err(message) => Err(PodError::CallFailed(message)),
        },
        Err(_) => Err(PodError::CallFailed(
            "入口服务没有返回 Result<Box<dyn Any>, String>".to_string(),
        )),
    }
}

fn function_id(pod: &Arc<dyn Pod>, path: &str, ret: &str) -> Result<FunctionId, PodError> {
    pod.function_id(&QualifiedName::from_path(path, vec![], ret))
        .ok_or_else(|| PodError::UnknownFunction(path.to_string()))
}
