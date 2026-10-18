// imp.rs — pyo3 实现（仅 `feature = "python"` 编译）。
//
// # 线程模型（P1：SyncPod 徽章）
//
// `PyCFunction::new_closure` 要求闭包 `Send + Sync`。服务以 `Arc<dyn SyncPod>`
// **直接捕获进闭包**（`SyncPod: Pod + Send + Sync`）⇒ 不再需要线程本地槽位表，
// 也不再需要 `sys.modules` 之外的全局锁来保护槽位。
//
// 代价：提供方必须登记 `SyncPod` 徽章（`PodNode::new_sync`）；未登记 ⇒ Broker
// 给出明确的 `NotSyncPod` 错误，而不是静默降级。

use std::any::Any;
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::ffi::CString;
use std::sync::{Arc, Mutex, MutexGuard};
use std::sync::atomic::{AtomicU64, Ordering};

use pyo3::exceptions::{PyRuntimeError, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyCFunction, PyDict, PyList, PyModule, PyTuple};
use xc_pod::{
    Capability, ErasedFn, Field, FunctionId, QualifiedName, RegistryPod, ScalarKind, ScalarValue,
    ServiceRequirement, SyncPod, TypeSpec, Pod, PodError, PodId,
};
use xc_pod_binding::{BindingPlan, ValueCodec, invoke, short_name};

use crate::{ENTRY_SERVICE, FINISH_PATH, FloatList, PythonApi, RESET_PATH, RUN_PATH};

/// Python 模块名必须**每个 PyPod 唯一**：`PyImport_ExecCodeModuleEx` 会复用
/// `sys.modules` 里的同名模块，多实例会共享 globals。
static NEXT_MODULE: AtomicU64 = AtomicU64::new(1);
/// `sys.modules` 里的 `xcal.*` 别名是进程级的；脚本执行期间必须串行，
/// 否则两个 PyPod 会互相覆盖。锁在 GIL 内获取 ⇒ 不会与解释器锁形成死锁。
static PY_RUN_LOCK: Mutex<()> = Mutex::new(());

thread_local! {
    /// 当前线程的 `py_run` 嵌套深度（**只用于重入探测**，不替代进程级锁）。
    ///
    /// 核实结论：当前没有任何 host capability 会回调 `PyPod::run` ——
    /// `scene` / `content` 都是纯 Rust Service，`python.linspace` / `make_dot`
    /// 只执行 Python 函数、不进入 `run`。因此本应在同一线程上恒为 0。
    /// 这里把"应该不会"变成可验证：真出现重入时，debug 构建直接 panic。
    static RUN_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// `py_run` 的执行守卫：进程级锁 + 同线程重入探测。
struct RunGuard {
    _lock: MutexGuard<'static, ()>,
}

impl RunGuard {
    fn acquire() -> Result<Self, PodError> {
        let depth = RUN_DEPTH.with(Cell::get);
        if depth > 0 {
            debug_assert!(
                false,
                "py_run 重入：同一线程在脚本执行期间再次进入 run。\
                 当前 host capability 不应回调 PyPod::run；若新增了这种回调路径，\
                 需要把本守卫改成可重入或显式拒绝。"
            );
            return Err(PodError::CallFailed(format!(
                "py_run 重入（当前线程嵌套深度 {depth}）：脚本执行期间不得再次进入 run"
            )));
        }

        let lock = PY_RUN_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        RUN_DEPTH.with(|depth| depth.set(1));
        Ok(Self { _lock: lock })
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        RUN_DEPTH.with(|depth| depth.set(0));
    }
}

// ─── Python 侧的值载体 ─────────────────────────────────────────────────

/// 结构化值（`Color` / `Curve.Phase` …）。
#[pyclass]
#[derive(Clone)]
pub struct PyPodValue {
    value: xc_pod::PodValue,
}

/// 不透明句柄（组件 / 对象 / 曲线 / 动作）。
#[pyclass]
#[derive(Clone, Copy)]
pub struct PyHandle {
    handle: xc_pod::Handle,
}

/// Python 值 ↔ `PodValue`（**B4 的 ValueCodec**；通用驱动 `invoke` 复用）。
struct PyCodec<'py> {
    py: Python<'py>,
}

impl<'py> ValueCodec for PyCodec<'py> {
    type Value = Bound<'py, PyAny>;
    type Error = PyErr;

    fn to_pod(
        &self,
        value: &Bound<'py, PyAny>,
        spec: &'static TypeSpec,
    ) -> PyResult<xc_pod::PodValue> {
        let bound = value;
        if let Ok(wrapped) = bound.extract::<PyRef<PyPodValue>>() {
            return Ok(wrapped.value.clone());
        }
        if let Ok(handle) = bound.extract::<PyRef<PyHandle>>() {
            return Ok(xc_pod::PodValue::Opaque(handle.handle));
        }
        match spec {
            TypeSpec::Scalar(kind) => Ok(xc_pod::PodValue::Scalar(match kind {
                ScalarKind::F32 => ScalarValue::F32(bound.extract::<f32>()?),
                ScalarKind::F64 => ScalarValue::F64(bound.extract::<f64>()?),
                ScalarKind::I32 => ScalarValue::I32(bound.extract::<i32>()?),
                ScalarKind::U32 => ScalarValue::U32(bound.extract::<u32>()?),
                ScalarKind::U64 => ScalarValue::U64(bound.extract::<u64>()?),
                ScalarKind::U8 => ScalarValue::U8(bound.extract::<u8>()?),
                ScalarKind::Bool => ScalarValue::Bool(bound.extract::<bool>()?),
                ScalarKind::String => ScalarValue::String(bound.extract::<String>()?),
            })),
            TypeSpec::Opaque(_) => Err(PyErr::new::<PyTypeError, _>("期望 xcal 句柄（PyHandle）")),
            TypeSpec::Array { elem } => {
                let list = bound.cast::<PyList>()?;
                let mut values = Vec::with_capacity(list.len());
                for item in list.iter() {
                    values.push(self.to_pod(&item, elem)?);
                }
                Ok(xc_pod::PodValue::Array(values))
            }
            TypeSpec::Struct { name, fields } => {
                let dict = bound.cast::<PyDict>()?;
                let mut values = Vec::with_capacity(fields.len());
                for field in *fields {
                    let item = dict.get_item(field.name)?.ok_or_else(|| {
                        PyErr::new::<PyTypeError, _>(format!("`{name}` 缺少字段 `{}`", field.name))
                    })?;
                    values.push((field.name, self.to_pod(&item, field.ty)?));
                }
                Ok(xc_pod::PodValue::Struct(name, values))
            }
            TypeSpec::Enum { name, .. } => Err(PyErr::new::<PyTypeError, _>(format!(
                "枚举 `{name}` 请用构造器创建"
            ))),
        }
    }

    fn from_pod(&self, value: &xc_pod::PodValue) -> PyResult<Bound<'py, PyAny>> {
        let py = self.py;
        match value {
            xc_pod::PodValue::Scalar(scalar) => Ok(match scalar {
                ScalarValue::F32(v) => v.into_pyobject(py)?.into_any(),
                ScalarValue::F64(v) => v.into_pyobject(py)?.into_any(),
                ScalarValue::I32(v) => v.into_pyobject(py)?.into_any(),
                ScalarValue::U32(v) => v.into_pyobject(py)?.into_any(),
                ScalarValue::U64(v) => v.into_pyobject(py)?.into_any(),
                ScalarValue::U8(v) => v.into_pyobject(py)?.into_any(),
                ScalarValue::Bool(v) => pyo3::types::PyBool::new(py, *v).to_owned().into_any(),
                ScalarValue::String(v) => v.into_pyobject(py)?.into_any(),
            }),
            xc_pod::PodValue::Opaque(handle) => Ok(Py::new(py, PyHandle { handle: *handle })?
                .into_bound(py)
                .into_any()),
            xc_pod::PodValue::Struct("()", _) => Ok(py.None().into_bound(py)),
            xc_pod::PodValue::Struct(..) | xc_pod::PodValue::Enum(..) => Ok(Py::new(
                py,
                PyPodValue {
                    value: value.clone(),
                },
            )?
            .into_bound(py)
            .into_any()),
            xc_pod::PodValue::Array(items) => {
                let list = PyList::empty(py);
                for item in items {
                    list.append(self.from_pod(item)?)?;
                }
                Ok(list.into_any())
            }
        }
    }

    fn error(&self, message: String) -> PyErr {
        PyErr::new::<PyRuntimeError, _>(message)
    }
}

// ─── 能力绑定（通用驱动）──────────────────────────────────────────────

fn make_function<'py>(
    py: Python<'py>,
    item: ErasedFn,
    service: Arc<dyn SyncPod>,
) -> PyResult<Bound<'py, PyCFunction>> {
    PyCFunction::new_closure(
        py,
        None,
        None,
        move |args: &Bound<'_, PyTuple>, _kwargs: Option<&Bound<'_, PyDict>>| {
            let py = args.py();
            let values: Vec<Bound<'_, PyAny>> = args.iter().collect();
            let codec = PyCodec { py };
            invoke(&codec, service.as_pod(), item, &values).map(|value| value.unbind())
        },
    )
}

fn make_ctor<'py>(
    py: Python<'py>,
    fields: &'static [Field],
    type_name: &'static str,
    variant: Option<&'static str>,
    ctor: Option<ErasedFn>,
    service: Arc<dyn SyncPod>,
) -> PyResult<Bound<'py, PyCFunction>> {
    PyCFunction::new_closure(
        py,
        None,
        None,
        move |args: &Bound<'_, PyTuple>, kwargs: Option<&Bound<'_, PyDict>>| {
            let py = args.py();
            let codec = PyCodec { py };
            let mut values = Vec::with_capacity(fields.len());
            for (index, field) in fields.iter().enumerate() {
                let item: Bound<'_, PyAny> = match kwargs {
                    Some(dict) => dict.get_item(field.name)?.ok_or_else(|| {
                        PyErr::new::<PyTypeError, _>(format!(
                            "`{type_name}` 缺少字段 `{}`",
                            field.name
                        ))
                    })?,
                    None => args.get_item(index)?,
                };
                values.push((field.name, codec.to_pod(&item, field.ty)?));
            }
            let value = match variant {
                Some(variant) => xc_pod::PodValue::Enum(type_name, variant, values),
                None => xc_pod::PodValue::Struct(type_name, values),
            };
            match ctor {
                Some(ctor) => {
                    let out = (ctor.call)(service.as_pod(), &[value])
                        .map_err(|error| PyErr::new::<PyRuntimeError, _>(error.to_string()))?;
                    codec.from_pod(&out).map(|value| value.unbind())
                }
                None => Ok(Py::new(py, PyPodValue { value })?
                    .into_bound(py)
                    .into_any()
                    .unbind()),
            }
        },
    )
}

/// 一个已注册能力的绑定器（泛型在注册时单态化，运行时只留 fn 指针）。
type BindFn = for<'py> fn(Python<'py>, &PyPod, &str, Arc<dyn SyncPod>) -> PyResult<()>;

/// 确保包链存在（如 `xcal`），返回最末一级包模块。
fn ensure_package(py: Python<'_>, this: &PyPod, path: &str) -> PyResult<Py<PyModule>> {
    let existing = this
        .packages
        .lock()
        .expect("Python 包表锁中毒")
        .get(path)
        .map(|module| module.clone_ref(py));
    if let Some(module) = existing {
        return Ok(module);
    }

    let module = PyModule::new(py, path)?;
    module.setattr("__path__", PyList::empty(py))?;
    if let Some((parent_path, name)) = path.rsplit_once('.') {
        let parent = ensure_package(py, this, parent_path)?;
        parent.bind(py).setattr(name, &module)?;
    }
    let module = module.unbind();
    this.packages
        .lock()
        .expect("Python 包表锁中毒")
        .insert(path.to_string(), module.clone_ref(py));
    Ok(module)
}

/// 把一个 capability 绑定为 `xcal.scene` 这样的真实模块，并挂到包树上。
fn bind_capability<C: Capability<'static>>(
    py: Python<'_>,
    this: &PyPod,
    service_name: &str,
    service: Arc<dyn SyncPod>,
) -> PyResult<()> {
    let namespace = C::signatures()
        .first()
        .map(|signature| signature.namespace)
        .filter(|namespace| !namespace.is_empty())
        .unwrap_or(service_name);

    let service_module = PyModule::new(py, namespace)?;
    let plan = BindingPlan::new(C::calls());
    for item in plan.calls {
        let function = make_function(py, *item, Arc::clone(&service))?;
        service_module.setattr(item.signature.name, function)?;
    }

    // `xcal.scene`：挂到父包上；同时保留根解释器模块上的短名（旧脚本兼容）。
    if let Some((parent_path, name)) = namespace.rsplit_once('.') {
        let parent = ensure_package(py, this, parent_path)?;
        parent.bind(py).setattr(name, &service_module)?;
    }
    this.module.bind(py).setattr(service_name, &service_module)?;

    register_types(py, this, &service_module, &plan, service)?;
    this.modules
        .lock()
        .expect("Python 模块表锁中毒")
        .insert(namespace.to_string(), service_module.unbind());
    Ok(())
}

/// 自动注册所有可达类型的构造器（组件 / 值 / 枚举）。
///
/// 类型按 `TypeSpec` 的全名归位：`xcal.Circle` ⇒ 根模块 + `xcal` 包 + 当前 capability 模块。
fn register_types(
    py: Python<'_>,
    this: &PyPod,
    service_module: &Bound<'_, PyModule>,
    plan: &BindingPlan,
    service: Arc<dyn SyncPod>,
) -> PyResult<()> {
    for (&full_name, &spec) in &plan.types {
        let short = short_name(full_name);
        let home = full_name
            .rsplit_once('.')
            .map(|(parent, _)| parent)
            .unwrap_or("");
        let home_module = if home.is_empty() {
            None
        } else {
            Some(ensure_package(py, this, home)?)
        };

        match spec {
            TypeSpec::Struct { name, fields } => {
                let ctor = plan.component_ctor(name);
                let function =
                    make_ctor(py, fields, name, None, ctor, Arc::clone(&service))?;
                this.module.bind(py).setattr(short, &function)?;
                service_module.setattr(short, &function)?;
                if let Some(home_module) = &home_module {
                    home_module.bind(py).setattr(short, &function)?;
                }
            }
            TypeSpec::Enum { name, variants } => {
                let enum_module = PyModule::new(py, short)?;
                for variant in *variants {
                    if variant.fields.is_empty() {
                        enum_module.setattr(
                            variant.name,
                            Py::new(
                                py,
                                PyPodValue {
                                    value: xc_pod::PodValue::Enum(name, variant.name, Vec::new()),
                                },
                            )?
                            .into_any(),
                        )?;
                    } else {
                        let function = make_ctor(
                            py,
                            variant.fields,
                            name,
                            Some(variant.name),
                            None,
                            Arc::clone(&service),
                        )?;
                        enum_module.setattr(variant.name, function)?;
                    }
                }
                this.module.bind(py).setattr(short, &enum_module)?;
                service_module.setattr(short, &enum_module)?;
                if let Some(home_module) = &home_module {
                    home_module.bind(py).setattr(short, &enum_module)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

// ─── PyPod ─────────────────────────────────────────────────────────────

const PY_SOURCE: &str = r#"
def linspace(a, b, n):
    n = int(n)
    if n <= 1:
        return [float(a)] * max(n, 0)
    step = (float(b) - float(a)) / (n - 1)
    return [float(a) + step * i for i in range(n)]

def make_dot(x, y, r):
    shape = Circle(x=x, y=y, r=r)
    return scene.spawn([shape])
"#;

/// Python Pod：实现 `Pod` + `Injector` + `SyncInjector` + `PythonApi`。
///
/// 所有可变状态都是 `Mutex`（不是 `RefCell`）⇒ 本类型登记 `SyncPod` 徽章，
/// 注入的服务可以被 Python 闭包直接捕获（闭包要求 `Send + Sync`）。
pub struct PyPod {
    id: PodId,
    registry: RegistryPod,
    module: Py<PyModule>,
    injected: Mutex<Vec<String>>,
    /// Service 名 → 能力绑定器（**不写死任何 domain 类型**）。
    binders: Mutex<BTreeMap<String, BinderEntry>>,
    /// 已注入 Service 的线程安全视图（`Drop` 时随 `Arc` 一起释放）。
    slots: Mutex<HashMap<String, Arc<dyn SyncPod>>>,
    /// capability 包树（`xcal` 等）；脚本执行前安装到 `sys.modules`。
    packages: Mutex<BTreeMap<String, Py<PyModule>>>,
    /// capability 模块（`xcal.scene` 等）；脚本可用 `import xcal.scene`。
    modules: Mutex<BTreeMap<String, Py<PyModule>>>,
}

/// 一个已注册能力的注入端点。
struct BinderEntry {
    /// 协商 + 建 Python 侧模块。
    bind: BindFn,
    /// 自述需求（`required` 由注册时决定）。
    requirements: Vec<ServiceRequirement>,
}

impl PyPod {
    /// 新建（需要 Python 解释器；失败 ⇒ `PodError`）。
    pub fn new() -> Result<Self, PodError> {
        let module = Python::attach(|py| -> PyResult<Py<PyModule>> {
            let code = CString::new(PY_SOURCE).expect("内置 Python 源码无 NUL");
            let file = CString::new("xc_pod_py.py").expect("文件名无 NUL");
            let instance = NEXT_MODULE.fetch_add(1, Ordering::Relaxed);
            let name = CString::new(format!("xc_pod_py_{instance}")).expect("模块名无 NUL");
            PyModule::from_code(py, &code, &file, &name).map(|module| module.unbind())
        })
        .map_err(|error| PodError::CallFailed(format!("Python 初始化失败：{error}")))?;

        let mut registry = RegistryPod::new();
        registry.register_fn0_named::<Vec<ServiceRequirement>>(
            "xc_pod::injector::requirements",
            "ServiceRequirements",
            py_requirements,
        );
        registry.register_fn2_named::<String, Arc<dyn Pod>, Result<(), PodError>>(
            "xc_pod::injector::inject",
            "ServiceName",
            "ServicePod",
            "InjectResult",
            py_inject,
        );
        // P1：线程安全注入端点（Broker 在提供方登记 SyncPod 时优先走这条）。
        registry.register_fn2_named::<String, Arc<dyn SyncPod>, Result<(), PodError>>(
            "xc_pod::injector::inject_sync",
            "ServiceName",
            "ServiceSyncPod",
            "InjectResult",
            py_inject_sync,
        );
        registry.register_fn3_named::<f32, f32, u32, FloatList>(
            "xcal.python::linspace",
            "f32",
            "f32",
            "u32",
            "Array<f32>",
            py_linspace,
        );
        registry.register_fn3_named::<f32, f32, f32, ()>(
            "xcal.python::make_dot",
            "f32",
            "f32",
            "f32",
            "()",
            py_make_dot,
        );
        // 宿主 → Python 脚本（erased；与 `xc-pod-lua` 的 run 同形）。
        registry.register_erased(RUN_PATH, &["String"], "Any", py_run);

        Ok(Self {
            id: PodId::next(),
            registry,
            module,
            injected: Mutex::new(Vec::new()),
            binders: Mutex::new(BTreeMap::new()),
            slots: Mutex::new(HashMap::new()),
            packages: Mutex::new(BTreeMap::new()),
            modules: Mutex::new(BTreeMap::new()),
        })
    }

    /// 注册一个能力的绑定器（必须在 Broker 装配前调用）。
    ///
    /// `required = false` ⇒ 该 Service 是 **optional**：没有提供者也能装配。
    pub fn register_capability<C: Capability<'static>>(&self, service: &str, required: bool) {
        let requirements = C::signatures()
            .iter()
            .map(|sig| ServiceRequirement::new(service, &sig.qualified().to_string(), required))
            .collect();
        self.binders
            .lock()
            .expect("Python 绑定表锁中毒")
            .insert(
                service.to_string(),
                BinderEntry {
                    bind: bind_capability::<C>,
                    requirements,
                },
            );
    }

    /// 宿主 → 脚本：执行一段 Python 源码，取回入口 Service 的产物 `T`。
    ///
    /// 解释器不知道 `T` 是什么（`SceneScript` / 任意录制结果），
    /// 因此这里对 `T` 完全泛型 —— 与 `xc-pod-lua::LuaPod::run` 对称。
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

    /// 已注入的 Service 名（测试 / 排查）。
    pub fn injected(&self) -> Vec<String> {
        self.injected
            .lock()
            .expect("Python 注入表锁中毒")
            .clone()
    }
}

impl Pod for PyPod {
    fn id(&self) -> PodId {
        self.id
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn functions(&self) -> &[QualifiedName] {
        self.registry.functions()
    }

    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)> {
        self.registry.resolve(name)
    }

    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
        let name = self
            .functions()
            .get(id.get() as usize)
            .ok_or_else(|| PodError::UnknownFunction(format!("函数编号 {id}")))?;
        // erased 入口必须拿 `PyPod` 本身（`RegistryPod` 只会把**自己**传给 handlers）。
        if name.name() == "run" {
            py_run(self, args)
        } else {
            self.registry.call(id, args)
        }
    }
}

fn py_pod(pod: &dyn Pod) -> &PyPod {
    pod.as_any()
        .downcast_ref::<PyPod>()
        .expect("PyPod 的调用者必须是 PyPod")
}

fn py_requirements(pod: &dyn Pod) -> Vec<ServiceRequirement> {
    let this = py_pod(pod);
    this.binders
        .lock()
        .expect("Python 绑定表锁中毒")
        .values()
        .flat_map(|entry| entry.requirements.iter().cloned())
        .collect()
}

/// 普通注入：**明确拒绝**非线程安全提供方（Python 闭包必须捕获 `Arc<dyn SyncPod>`）。
fn py_inject(pod: &dyn Pod, service: String, _service_pod: Arc<dyn Pod>) -> Result<(), PodError> {
    let _ = py_pod(pod);
    Err(PodError::CallFailed(format!(
        "Python 侧 `{service}` 需要线程安全 Service：提供方未登记 SyncPod 徽章\
         （用 PodNode::new_sync 注册；Broker 会优先走 inject_sync）"
    )))
}

/// 线程安全注入：把同一个实例的 `Arc<dyn SyncPod>` 交出去（闭包直接捕获）。
fn py_inject_sync(
    pod: &dyn Pod,
    service: String,
    service_pod: Arc<dyn SyncPod>,
) -> Result<(), PodError> {
    let this = py_pod(pod);
    let binder = this
        .binders
        .lock()
        .expect("Python 绑定表锁中毒")
        .get(&service)
        .map(|entry| entry.bind);
    let Some(binder) = binder else {
        return Err(PodError::CallFailed(format!(
            "未知 Service `{service}`（能力未注册；先调用 `PyPod::register_capability`）"
        )));
    };

    this.slots
        .lock()
        .expect("Python 注入表锁中毒")
        .insert(service.clone(), Arc::clone(&service_pod));
    this.injected
        .lock()
        .expect("Python 注入表锁中毒")
        .push(service.clone());

    let result = Python::attach(|py| binder(py, this, &service, service_pod));
    if let Err(error) = result {
        this.slots
            .lock()
            .expect("Python 注入表锁中毒")
            .remove(&service);
        return Err(PodError::CallFailed(format!(
            "Python 侧绑定 `{service}` 失败：{error}"
        )));
    }
    Ok(())
}

fn py_linspace(pod: &dyn Pod, from: f32, to: f32, samples: u32) -> FloatList {
    let this = py_pod(pod);
    Python::attach(|py| -> PyResult<FloatList> {
        let module = this.module.bind(py);
        let function = module.getattr("linspace")?;
        let output = function.call1((from, to, samples))?;
        Ok(FloatList(output.extract::<Vec<f32>>()?))
    })
    .unwrap_or_else(|_| FloatList(Vec::new()))
}

fn py_make_dot(pod: &dyn Pod, x: f32, y: f32, r: f32) {
    let this = py_pod(pod);
    let _ = Python::attach(|py| -> PyResult<()> {
        let module = this.module.bind(py);
        let function = module.getattr("make_dot")?;
        function.call1((x, y, r))?;
        Ok(())
    });
}

// ─── 宿主 → 脚本：run ──────────────────────────────────────────────────

fn py_run(pod: &dyn Pod, args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
    let source = args
        .first()
        .and_then(|arg| arg.downcast_ref::<String>())
        .cloned()
        .ok_or_else(|| PodError::CallFailed("run 需要字符串源码".to_string()))?;
    run_impl(pod, source)
}

/// 把本 Pod 的 `xcal` 包树安装到 `sys.modules`，使脚本可显式 `import`。
fn install_modules(py: Python<'_>, this: &PyPod) -> PyResult<()> {
    let sys = py.import("sys")?;
    let modules = sys.getattr("modules")?.cast_into::<PyDict>()?;
    for (name, module) in this
        .packages
        .lock()
        .expect("Python 包表锁中毒")
        .iter()
    {
        modules.set_item(name.as_str(), module.bind(py))?;
    }
    for (name, module) in this
        .modules
        .lock()
        .expect("Python 模块表锁中毒")
        .iter()
    {
        modules.set_item(name.as_str(), module.bind(py))?;
    }
    Ok(())
}

fn run_impl(pod: &dyn Pod, source: String) -> Result<Box<dyn Any>, PodError> {
    let this = py_pod(pod);
    let service = this
        .slots
        .lock()
        .expect("Python 注入表锁中毒")
        .get(ENTRY_SERVICE)
        .cloned()
        .ok_or_else(|| PodError::CallFailed("Python Pod 尚未注入入口服务（scene）".to_string()))?;

    // 每轮先丢弃上一轮录制状态。
    let reset = function_id(service.as_pod(), RESET_PATH, "()")?;
    service.call(reset, &[])?;

    // 在模块自己的 globals 里执行：脚本能直接看到注入的 `scene` / `content` 与类型构造器。
    Python::attach(|py| -> PyResult<()> {
        // `sys.modules["xcal"]` 是进程级的：安装 + 执行必须原子（见 PY_RUN_LOCK）。
        // RunGuard 同时做同线程重入探测（当前不存在合法重入路径）。
        let _run = RunGuard::acquire()
            .map_err(|error| PyErr::new::<PyRuntimeError, _>(error.to_string()))?;
        install_modules(py, this)?;
        let module = this.module.bind(py);
        let globals = module.dict();
        let code = CString::new(source)
            .map_err(|_| PyErr::new::<PyRuntimeError, _>("Python 源码含 NUL"))?;
        py.run(&code, Some(&globals), Some(&globals))
    })
    .map_err(|error| PodError::CallFailed(format!("Python 执行失败：{error}")))?;

    let finish = function_id(
        service.as_pod(),
        FINISH_PATH,
        "Result<Box<dyn Any>, String>",
    )?;
    let out = service.call(finish, &[])?;

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

fn function_id(pod: &dyn Pod, path: &str, ret: &str) -> Result<FunctionId, PodError> {
    pod.function_id(&QualifiedName::from_path(path, vec![], ret))
        .ok_or_else(|| PodError::UnknownFunction(path.to_string()))
}

/// 能力签名自检（保证 `PythonApi` 与注册名严格一致）。
pub fn registered_python_names(pod: &PyPod) -> Vec<String> {
    PythonApi::signatures()
        .iter()
        .filter(|sig| pod.functions().contains(&sig.qualified()))
        .map(|sig| sig.name.to_string())
        .collect()
}

#[cfg(test)]
mod run_guard_tests {
    use super::*;

    /// 顺序进入 / 退出是合法的（锁可重复获取）。
    #[test]
    fn sequential_runs_are_allowed() {
        drop(RunGuard::acquire().expect("第一次进入"));
        drop(RunGuard::acquire().expect("第二次进入"));
    }

    /// 同线程重入：debug 构建直接 panic（把"应该不会"变成可验证）。
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "py_run 重入")]
    fn reentrant_run_panics_in_debug() {
        let _outer = RunGuard::acquire().expect("外层进入");
        let _inner = RunGuard::acquire();
    }

    /// release 构建没有 debug_assert，但仍给**明确错误**（不静默、不锁死）。
    #[cfg(not(debug_assertions))]
    #[test]
    fn reentrant_run_returns_an_explicit_error_in_release() {
        let _outer = RunGuard::acquire().expect("外层进入");
        assert!(
            RunGuard::acquire().is_err(),
            "同线程重入必须返回明确错误"
        );
    }
}
