// ze-pod-py — **纯 Python 适配器**（pyo3，feature = "python"）。
//
// 生产依赖只有 `ze-pod-core` / `ze-pod-binding` / `ze-pod-macro`（+ 可选的 `pyo3`）：
// 不依赖任何 `zela-*`；domain 能力由 App 通过 `register_capability::<C>` 注入。
//
// # 为什么是可选
//
// `pyo3` 需要系统 Python（或 abi3），与 `mlua vendored` 的"零运行时依赖"正面冲突。
// 因此 crate 层保持 `python` feature 可选；App 默认开启（见 `zela-app/Cargo.toml`）。
//
// # 两个方向
//
// ```text
// Python → 宿主：装配期。注入的 `scene.*` / `content.*` 由 `calls()` 自动生成；
// 宿主 → Python：Pod::call("xcal.python::run", [source])（erased 慢路径）。
// ```
//
// # 职责
//
// 只做装配期的活（一次算一批点 → 固化成对象）：
// - `linspace`：真实执行 Python 代码，返回批量数据；
// - `make_dot`：Python 侧经注入的能力在宿主里建对象（**不绕过句柄检查**）；
// - `run`：执行一段 Python 脚本，取回入口 Service 的产物。
//
// 每帧 `FunctionX2Y` / 跨 Pod batch 调用不在本轮（见计划 §9 记账）。
#![forbid(unsafe_code)]

use ze_pod_core::{IntoPodTypeName, TypeSpec, PodType, PodTypeName, PodValue, PodValueError};
use ze_pod_macro::capability;

pub mod export;

pub use export::{export_docs, pyi_files};

/// 宿主 → Python 脚本的唯一入口（erased）。
pub const RUN_PATH: &str = "xcal.python::run";
/// `run` 的完整函数名（含参数 / 返回类型；`Broker::run_with` 用）。
pub const RUN_ENTRY: &str = "xcal.python::run(String) -> Any";
/// 入口 Service 的约定名（与 `ze-pod-lua` 相同的协议，不是 crate 依赖）。
pub const ENTRY_SERVICE: &str = "scene";
/// 入口 Service 的内部交接函数（不是脚本可见 API）。
pub const RESET_PATH: &str = "xcal.scene::__reset";
pub const FINISH_PATH: &str = "xcal.scene::__finish";

/// `Array<f32>` 的具名载体（orphan 规则：不能为 `Vec<f32>` 实现外部 trait）。
#[derive(Clone, Debug, PartialEq)]
pub struct FloatList(pub Vec<f32>);

impl PodType for FloatList {
    const SPEC: &'static TypeSpec = <Vec<f32> as PodType>::SPEC;

    fn to_pod_value(&self) -> PodValue {
        <Vec<f32> as PodType>::to_pod_value(&self.0)
    }

    fn from_pod_value(value: &PodValue) -> Result<Self, PodValueError> {
        <Vec<f32> as PodType>::from_pod_value(value).map(Self)
    }
}

impl IntoPodTypeName for FloatList {
    const NAME: PodTypeName = "Array<f32>";
}

/// Python Pod 提供给宿主的批量能力。
#[capability(
    namespace = "xcal.python",
    version = "1",
    description = "Python 适配器的装配期批量能力",
    bindings
)]
pub struct PythonApi<'a> {
    /// 在 `[from, to]` 上均匀取 `samples` 个点（Python 执行）。
    pub linspace: fn(&'a dyn Pod, from: f32, to: f32, samples: u32) -> FloatList,

    /// Python 侧调用注入的 `scene.*` 建一个圆（**真实 Pod → 宿主调用**）。
    pub make_dot: fn(&'a dyn Pod, x: f32, y: f32, r: f32) -> (),
}

#[cfg(feature = "python")]
mod imp;

#[cfg(feature = "python")]
pub use imp::{PyPod, registered_python_names};
