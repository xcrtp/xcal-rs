// math_pod.rs — **`ze-pod-core` 的第一个生产消费方**：内置数学函数由 Pod 提供。
//
// # 为什么放在这里（而不是 `ze-notion`）
//
// `FunctionX2Y` 在理念层，但"函数从哪来"是**使用侧**的事：
// - `ze-notion` 不得依赖 `ze-pod-core`（分层铁律，本轮 §7.2）；
// - `zela-scene` 已依赖两者，且函数图像的持久化（名字壳 → 函数体）就在这里。
//
// # 适配器形态
//
// ```text
// Pod 侧函数： fn(&dyn Pod, X) -> Y
// DynFn<X,Y>： Fn(X) -> Y
// ```
//
// 适配器是**闭包**（自定义 struct 无法在 stable 上实现 `Fn`）：
// 捕获 `Arc<dyn Pod + Send + Sync>` + 解析出的裸 fn 指针。
// - **Send + Sync**：`Arc<dyn Pod + Send + Sync>` 满足；
// - **可 Clone**：`DynFn` 的 blanket impl 深克隆闭包（`Arc` 共享，行为一致）；
// - **name**：`FunctionX2Y::named` 写入，序列化只写它。
//
// 调用走 **typed 快路径**（`resolve` 拿到的 fn 指针），不查表、不比字符串。
// 纯解释型 Pod（只有 erased `call`）不适用本适配器 —— 它们本来也提供不了
// `DynFn` 的零成本调用形态。
//
// # 适用边界（C3 记账）
//
// 本适配器要求 Pod **能提供 typed 裸 fn 指针**（`RegistryPod` / 原生 Pod）。
// 当初 Pod 抽象的愿景里，Python 脚本与 IPC 远程 Pod 也是 Pod，但它们**只有
// erased `call`** ⇒ 本适配器对它们无效。接入这类 Pod 时需要一个**走慢路径的
// 独立适配器**（把 `call` 包成 `Fn`）；本轮不写（无消费方，写了就是纸面能力）。

use std::sync::{Arc, OnceLock};

use ze_notion::FunctionX2Y;
use ze_pod_core::{IntoPodTypeName, Pod, QualifiedName, RegistryPod};

/// 可当函数提供者的 Pod：**具体类型**必须 `Send + Sync`（trait 本身不变）。
///
/// `Cell` 型有状态 Pod 仍可存在，只是不能当函数提供者。
pub type SharedPod = Arc<dyn Pod + Send + Sync>;

/// 把 Pod 里的一个 typed 函数适配成 [`FunctionX2Y`]。
///
/// 名字不存在 / 签名不匹配（不是 `fn(&dyn Pod, X) -> Y`）⇒ `None`。
pub fn function_from_pod<X, Y>(pod: &SharedPod, name: &str) -> Option<FunctionX2Y<X, Y>>
where
    X: IntoPodTypeName + 'static,
    Y: IntoPodTypeName + 'static,
{
    // 完整函数名 = path + 参数类型名 + 返回类型名（与注册侧的口径一致）。
    let qualified = QualifiedName::from_path(name, vec![X::NAME.to_string()], Y::NAME);
    let raw = pod.resolve(&qualified)?;
    let f = *raw.downcast_ref::<fn(&dyn Pod, X) -> Y>()?;
    let pod = Arc::clone(pod);
    Some(FunctionX2Y::named(name, move |x: X| f(&*pod, x)))
}

/// 内置数学函数的 Pod（装配期注册一次，调用期只读）。
///
/// 7 个名字即原 `ze-notion::builtin_f32` 那张表（本轮**搬进 Pod**；
/// `ze-notion` 不依赖 `ze-pod-core`，故表不再留在理念层）。
pub fn math_pod() -> &'static SharedPod {
    static MATH_POD: OnceLock<SharedPod> = OnceLock::new();
    MATH_POD.get_or_init(|| {
        let mut pod = RegistryPod::new();
        pod.register_fn1("xcal.math.sin", |_: &dyn Pod, x: f32| x.sin());
        pod.register_fn1("xcal.math.cos", |_: &dyn Pod, x: f32| x.cos());
        pod.register_fn1("xcal.math.tan", |_: &dyn Pod, x: f32| x.tan());
        pod.register_fn1("xcal.math.abs", |_: &dyn Pod, x: f32| x.abs());
        pod.register_fn1("xcal.math.sqrt", |_: &dyn Pod, x: f32| x.sqrt());
        pod.register_fn1("xcal.math.exp", |_: &dyn Pod, x: f32| x.exp());
        pod.register_fn1("xcal.math.ln", |_: &dyn Pod, x: f32| x.ln());
        Arc::new(pod)
    })
}

/// **经 Pod 反查**的内置函数（唯一内置表入口）。
///
/// 反序列化后的默认绑定、场景装配都走它。
pub fn builtin_f32(name: &str) -> Option<FunctionX2Y<f32, f32>> {
    function_from_pod(math_pod(), name)
}

/// 反序列化后的**默认绑定**：把组件里的"名字壳"按 [`math_pod`] 补成函数体。
///
/// 未登记的名字保持未绑定（调用会 panic，明确失败），调用方可自行用
/// [`FunctionX2Y::bind`] 补。
pub fn rebind_components(components: &mut ze_scene_contract::Components) {
    for c in components.iter_mut() {
        // 必须显式解引用到 `dyn Component`（`Box<dyn Component>` 也满足 blanket impl）。
        if let Some(graph) = (**c)
            .as_any_mut()
            .downcast_mut::<ze_object::FunctionGraph>()
        {
            if graph.func.is_bound() {
                continue;
            }
            let Some(name) = graph.func.name.clone() else {
                continue;
            };
            if let Some(f) = builtin_f32(&name) {
                graph.func = f;
            }
        }
    }
}
