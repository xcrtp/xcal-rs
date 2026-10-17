// imp.rs — pyo3 实现（仅 `feature = "python"` 编译）。
//
// # 线程模型
//
// `Pod` 不要求 `Send + Sync`，而 `PyCFunction::new_closure` 要求闭包 `Send + Sync`。
// 因此 Python 侧的函数闭包只捕获一个**槽位号**；真正的 `Arc<dyn Pod>` 放在
// 线程本地表里（Python 调用始终在同一线程 + GIL 下）。这是类型系统约束下的
// 最小桥；不用 `unsafe`。

use std::any::Any;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::ffi::CString;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use pyo3::exceptions::{PyRuntimeError, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyCFunction, PyDict, PyList, PyModule, PyTuple};
use xc_pod::{
    Capability, ErasedFn, Field, FunctionId, QualifiedName, RegistryPod, ScalarKind, ScalarValue,
    ServiceRequirement, TypeSpec, Pod, PodError, PodId,
};
use xc_pod_binding::{BindingPlan, ValueCodec, invoke, short_name};

use crate::{ENTRY_SERVICE, FINISH_PATH, FloatList, PythonApi, RESET_PATH, RUN_PATH};

// ─── 线程本地服务表（闭包只捕获槽位号）─────────────────────────────────

thread_local! {
    static SERVICES: RefCell<HashMap<u64, Arc<dyn Pod>>> = RefCell::new(HashMap::new());
}

static NEXT_SLOT: AtomicU64 = AtomicU64::new(1);
/// Python 模块名必须**每个 PyPod 唯一**：`PyImport_ExecCodeModuleEx` 会复用
/// `sys.modules` 里的同名模块，多实例会共享 globals。
static NEXT_MODULE: AtomicU64 = AtomicU64::new(1);
/// `sys.modules` 里的 `xcal.*` 别名是进程级的；脚本执行期间必须串行，
/// 否则两个 PyPod 会互相覆盖。锁在 GIL 内获取 ⇒ 不会与解释器锁形成死锁。
static PY_RUN_LOCK: Mutex<()> = Mutex::new(());

fn register_service(pod: Arc<dyn Pod>) -> u64 {
    let id = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
    SERVICES.with(|services| {
        services.borrow_mut().insert(id, pod);
    });
    id
}

fn with_service<R>(slot: u64, f: impl FnOnce(&Arc<dyn Pod>) -> R) -> Option<R> {
    SERVICES.with(|services| services.borrow().get(&slot).map(f))
}

fn unregister_service(slot: u64) {
    SERVICES.with(|services| {
        services.borrow_mut().remove(&slot);
    });
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
    slot: u64,
) -> PyResult<Bound<'py, PyCFunction>> {
    PyCFunction::new_closure(
        py,
        None,
        None,
        move |args: &Bound<'_, PyTuple>, _kwargs: Option<&Bound<'_, PyDict>>| {
            let py = args.py();
            let values: Vec<Bound<'_, PyAny>> = args.iter().collect();
            let codec = PyCodec { py };
            match with_service(slot, |service| invoke(&codec, service, item, &values)) {
                Some(result) => result.map(|value| value.unbind()),
                None => Err(PyErr::new::<PyRuntimeError, _>("场景服务已释放")),
            }
        },
    )
}

fn make_ctor<'py>(
    py: Python<'py>,
    fields: &'static [Field],
    type_name: &'static str,
    variant: Option<&'static str>,
    ctor: Option<ErasedFn>,
    slot: u64,
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
                    let out = with_service(slot, |service| (ctor.call)(&**service, &[value]))
                        .ok_or_else(|| PyErr::new::<PyRuntimeError, _>("场景服务已释放"))?
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
type BindFn = for<'py> fn(Python<'py>, &PyPod, &str, u64) -> PyResult<()>;

/// 确保包链存在（如 `xcal`），返回最末一级包模块。
fn ensure_package(py: Python<'_>, this: &PyPod, path: &str) -> PyResult<Py<PyModule>> {
    let existing = this
        .packages
        .borrow()
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
        .borrow_mut()
        .insert(path.to_string(), module.clone_ref(py));
    Ok(module)
}

/// 把一个 capability 绑定为 `xcal.scene` 这样的真实模块，并挂到包树上。
fn bind_capability<C: Capability<'static>>(
    py: Python<'_>,
    this: &PyPod,
    service: &str,
    slot: u64,
) -> PyResult<()> {
    let namespace = C::signatures()
        .first()
        .map(|signature| signature.namespace)
        .filter(|namespace| !namespace.is_empty())
        .unwrap_or(service);

    let service_module = PyModule::new(py, namespace)?;
    let plan = BindingPlan::new(C::calls());
    for item in plan.calls {
        let function = make_function(py, *item, slot)?;
        service_module.setattr(item.signature.name, function)?;
    }

    // `xcal.scene`：挂到父包上；同时保留根解释器模块上的短名（旧脚本兼容）。
    if let Some((parent_path, name)) = namespace.rsplit_once('.') {
        let parent = ensure_package(py, this, parent_path)?;
        parent.bind(py).setattr(name, &service_module)?;
    }
    this.module.bind(py).setattr(service, &service_module)?;

    register_types(py, this, &service_module, &plan, slot)?;
    this.modules
        .borrow_mut()
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
    slot: u64,
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
                let function = make_ctor(py, fields, name, None, ctor, slot)?;
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
                        let function =
                            make_ctor(py, variant.fields, name, Some(variant.name), None, slot)?;
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

/// Python Pod：实现 `Pod` + `Injector` + `PythonApi`。
pub struct PyPod {
    id: PodId,
    registry: RegistryPod,
    module: Py<PyModule>,
    injected: RefCell<Vec<String>>,
    /// Service 名 → 能力绑定器（**不写死任何 domain 类型**）。
    binders: RefCell<BTreeMap<String, BinderEntry>>,
    /// 已注入 Service 的线程本地槽位（`Drop` 时清理）。
    slots: RefCell<HashMap<String, u64>>,
    /// capability 包树（`xcal` 等）；脚本执行前安装到 `sys.modules`。
    packages: RefCell<BTreeMap<String, Py<PyModule>>>,
    /// capability 模块（`xcal.scene` 等）；脚本可用 `import xcal.scene`。
    modules: RefCell<BTreeMap<String, Py<PyModule>>>,
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
            injected: RefCell::new(Vec::new()),
            binders: RefCell::new(BTreeMap::new()),
            slots: RefCell::new(HashMap::new()),
            packages: RefCell::new(BTreeMap::new()),
            modules: RefCell::new(BTreeMap::new()),
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
        self.binders.borrow_mut().insert(
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
        self.injected.borrow().clone()
    }
}

impl Drop for PyPod {
    fn drop(&mut self) {
        for slot in self.slots.borrow().values() {
            unregister_service(*slot);
        }
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
        .borrow()
        .values()
        .flat_map(|entry| entry.requirements.iter().cloned())
        .collect()
}

fn py_inject(pod: &dyn Pod, service: String, service_pod: Arc<dyn Pod>) -> Result<(), PodError> {
    let this = py_pod(pod);
    let binder = this.binders.borrow().get(&service).map(|entry| entry.bind);
    let Some(binder) = binder else {
        return Err(PodError::CallFailed(format!(
            "未知 Service `{service}`（能力未注册；先调用 `PyPod::register_capability`）"
        )));
    };

    let slot = register_service(Arc::clone(&service_pod));
    this.slots.borrow_mut().insert(service.clone(), slot);
    this.injected.borrow_mut().push(service.clone());

    let result = Python::attach(|py| binder(py, this, &service, slot));
    if let Err(error) = result {
        unregister_service(slot);
        this.slots.borrow_mut().remove(&service);
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
    for (name, module) in this.packages.borrow().iter() {
        modules.set_item(name.as_str(), module.bind(py))?;
    }
    for (name, module) in this.modules.borrow().iter() {
        modules.set_item(name.as_str(), module.bind(py))?;
    }
    Ok(())
}

fn run_impl(pod: &dyn Pod, source: String) -> Result<Box<dyn Any>, PodError> {
    let this = py_pod(pod);
    let slot = this
        .slots
        .borrow()
        .get(ENTRY_SERVICE)
        .copied()
        .ok_or_else(|| PodError::CallFailed("Python Pod 尚未注入入口服务（scene）".to_string()))?;

    // 每轮先丢弃上一轮录制状态。
    with_service(slot, |service| {
        let id = function_id(service, RESET_PATH, "()")?;
        service.call(id, &[])
    })
    .ok_or_else(|| PodError::CallFailed("入口服务已释放".to_string()))??;

    // 在模块自己的 globals 里执行：脚本能直接看到注入的 `scene` / `content` 与类型构造器。
    Python::attach(|py| -> PyResult<()> {
        // `sys.modules["xcal"]` 是进程级的：安装 + 执行必须原子（见 PY_RUN_LOCK）。
        let _guard = PY_RUN_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        install_modules(py, this)?;
        let module = this.module.bind(py);
        let globals = module.dict();
        let code = CString::new(source)
            .map_err(|_| PyErr::new::<PyRuntimeError, _>("Python 源码含 NUL"))?;
        py.run(&code, Some(&globals), Some(&globals))
    })
    .map_err(|error| PodError::CallFailed(format!("Python 执行失败：{error}")))?;

    let out = with_service(slot, |service| {
        let id = function_id(service, FINISH_PATH, "Result<Box<dyn Any>, String>")?;
        service.call(id, &[])
    })
    .ok_or_else(|| PodError::CallFailed("入口服务已释放".to_string()))??;

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

/// 能力签名自检（保证 `PythonApi` 与注册名严格一致）。
pub fn registered_python_names(pod: &PyPod) -> Vec<String> {
    PythonApi::signatures()
        .iter()
        .filter(|sig| pod.functions().contains(&sig.qualified()))
        .map(|sig| sig.name.to_string())
        .collect()
}
