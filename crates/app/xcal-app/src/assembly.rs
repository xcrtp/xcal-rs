// assembly.rs — App 的 **Pod 静态插件装配**。
//
// 描述层 / 执行层分离（D3）：
//
// ```text
// plan()     纯数据：节点名 / provides / requires（不建 Pod，可单测）
//    │
// assemble() 执行层：建 domain Service + 语言适配器 → Broker 校验 / 排序 / 注入
// ```
//
// 扩展方式（D3.3）：
// - 新增语言 = 新增 `xc-pod-*` crate + 这里加一个节点；
// - 新增 domain capability = `xcal-scene` 加一个函数（本文件与语言适配器都不改）。

use std::sync::Arc;

use xc_pod::{
    AssemblyPlan, Broker, BrokerError, Capability, FnSig, NodeSpec, Pod, PodNode,
    ServiceRequirement,
};
use xc_pod_lua::LuaPod;
use xc_prerender::InMemoryEngine;
use xcal_scene::pod::{self, ContentApi, PathStore, SceneApi};
use xcal_scene::scene_script::SceneScript;

/// 本 App 的分镜脚本（**App 自己的资产**，不属于语言适配器）。
pub const STORYBOARD_LUA: &str = include_str!("../scripts/storyboard.lua");
/// Python 版分镜（与 `storyboard.lua` 逐项等价）。
#[cfg(feature = "python")]
pub const STORYBOARD_PY: &str = include_str!("../scripts/storyboard.py");

/// 描述层：本 App 的静态插件清单（**纯数据**，可脱离 Pod 单测）。
///
/// `with_python` 只描述"是否挂 Python 节点"；能力需求的形状与运行时一致。
pub fn plan(with_python: bool) -> AssemblyPlan {
    let mut nodes = vec![
        NodeSpec::new("scene", &["scene"], Vec::new()),
        NodeSpec::new("content", &["content"], Vec::new()),
    ];

    let mut lua_requires = requirements("scene", SceneApi::signatures(), true);
    lua_requires.extend(requirements("content", ContentApi::signatures(), true));

    #[cfg(feature = "python")]
    {
        lua_requires.extend(requirements(
            "python",
            xc_pod_py::PythonApi::signatures(),
            false,
        ));
        if with_python {
            let mut python_requires = requirements("scene", SceneApi::signatures(), true);
            python_requires.extend(requirements("content", ContentApi::signatures(), true));
            nodes.push(NodeSpec::new("python", &["python"], python_requires));
        }
    }
    #[cfg(not(feature = "python"))]
    let _ = with_python;

    nodes.push(NodeSpec::new("lua", &["script"], lua_requires));
    AssemblyPlan::new(nodes)
}

fn requirements(service: &str, signatures: &[FnSig], required: bool) -> Vec<ServiceRequirement> {
    signatures
        .iter()
        .map(|signature| {
            ServiceRequirement::new(service, &signature.qualified().to_string(), required)
        })
        .collect()
}

/// 装配失败：语言节点初始化或 Broker 校验。
#[derive(Debug)]
pub enum AssemblyError {
    Broker(BrokerError),
    #[cfg(feature = "python")]
    Python(xc_pod::PodError),
}

impl std::fmt::Display for AssemblyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Broker(error) => write!(f, "{error}"),
            #[cfg(feature = "python")]
            Self::Python(error) => write!(f, "Python 适配器初始化失败：{error}"),
        }
    }
}

impl std::error::Error for AssemblyError {}

impl From<BrokerError> for AssemblyError {
    fn from(error: BrokerError) -> Self {
        Self::Broker(error)
    }
}

/// 装配产物：Broker（注入完成）+ 语言 Pod 句柄（测试 / 排查用）。
pub struct PodAssembly {
    pub broker: Broker,
    /// Lua Pod 句柄：只用于测试 / 排查（`registered_global_names`）。
    // 保留：默认构建无读取点，但 python feature 的测试会用到；属"排查句柄"而非漏接线。
    #[allow(dead_code)]
    pub lua: Arc<LuaPod>,
}

impl PodAssembly {
    /// 宿主 → Lua：执行分镜脚本，取回 `SceneScript`。
    pub fn run_script(&self, source: &str) -> Result<SceneScript, BrokerError> {
        let owned = source.to_string();
        let arg: &dyn std::any::Any = &owned;
        self.broker.run_with("lua", xc_pod_lua::RUN_ENTRY, &[arg])
    }

    /// 宿主 → Python：执行分镜脚本，取回 `SceneScript`。
    #[cfg(feature = "python")]
    pub fn run_python_script(&self, source: &str) -> Result<SceneScript, BrokerError> {
        let owned = source.to_string();
        let arg: &dyn std::any::Any = &owned;
        self.broker.run_with("python", xc_pod_py::RUN_ENTRY, &[arg])
    }
}

/// 静态装配：scene / content / lua（+ feature 开启时的 python）。
pub fn assemble(engine: Arc<InMemoryEngine>) -> Result<PodAssembly, AssemblyError> {
    // `nodes` 只在开启 python feature 时需要追加节点。
    #[cfg_attr(not(feature = "python"), allow(unused_mut))]
    let (lua, mut nodes) = base_nodes(engine);

    #[cfg(feature = "python")]
    {
        let python = Arc::new(xc_pod_py::PyPod::new().map_err(AssemblyError::Python)?);
        python.register_capability::<SceneApi>("scene", true);
        python.register_capability::<ContentApi>("content", true);
        // P1：Python 闭包要求 `Send + Sync` ⇒ 登记 SyncPod 徽章，Broker 走 inject_sync。
        nodes.push(PodNode::new_sync("python", python, &["python"]));
    }

    run_broker(lua, nodes)
}

/// 三个内置节点 + Lua 的能力注册。
fn base_nodes(engine: Arc<InMemoryEngine>) -> (Arc<LuaPod>, Vec<PodNode>) {
    let paths = PathStore::new();
    let scene = pod::scene_service(paths.clone());
    let content = pod::content_service(engine, paths);

    let lua = Arc::new(LuaPod::new());
    lua.register_capability::<SceneApi>("scene", true);
    lua.register_capability::<ContentApi>("content", true);
    #[cfg(feature = "python")]
    lua.register_capability::<xc_pod_py::PythonApi>("python", false);

    let nodes = vec![
        // scene / content 只含装配期状态（内部用锁），登记 SyncPod 徽章供 Python 闭包捕获。
        PodNode::new_sync("scene", scene, &["scene"]),
        PodNode::new_sync("content", content, &["content"]),
        // LuaPod 持有 mlua 解释器（非 Send + Sync）⇒ 不登记徽章。
        PodNode::new("lua", Arc::clone(&lua) as Arc<dyn Pod>, &["script"]),
    ];
    (lua, nodes)
}

fn run_broker(lua: Arc<LuaPod>, nodes: Vec<PodNode>) -> Result<PodAssembly, AssemblyError> {
    // 描述层先校验（不建 Pod 也能跑）；再与 Broker 从真实 Pod 推导的图对齐。
    let declared = plan(nodes.iter().any(|node| node.name == "python"));
    let declared_report = declared.validate()?;

    let broker = Broker::new(nodes);
    let report = broker.assemble()?;
    debug_assert_eq!(
        declared_report.order, report.order,
        "描述层与执行层的拓扑序不一致"
    );
    debug_assert_eq!(
        declared_report.edges, report.edges,
        "描述层与执行层的依赖边不一致"
    );
    debug_assert_eq!(
        declared_report.optional_missing, report.optional_missing,
        "描述层与执行层的 optional 缺失不一致"
    );

    eprintln!("[xcal-app] Pod 装配：\n{report}");
    Ok(PodAssembly { broker, lua })
}

#[cfg(test)]
mod tests {
    use super::*;
    use xc_pod::BrokerError;

    /// 描述层：拓扑 / 边 / optional 缺失全部可在**不建 Pod** 时断言。
    #[test]
    fn plan_is_pure_and_ordered() {
        let report = plan(false).validate().expect("静态清单合法");
        let position = |name: &str| {
            report
                .order
                .iter()
                .position(|item| item == name)
                .unwrap_or_else(|| panic!("缺节点 {name}"))
        };
        assert!(position("scene") < position("lua"));
        assert!(position("content") < position("lua"));
        assert!(
            report
                .edges
                .contains(&("scene".to_string(), "lua".to_string()))
        );
        assert!(
            report
                .edges
                .contains(&("content".to_string(), "lua".to_string()))
        );
        assert!(report.injected.is_empty(), "描述层不做注入");
    }

    /// 描述层：Python 节点（启用时）依赖 scene，且排在 lua 之前。
    #[cfg(feature = "python")]
    #[test]
    fn plan_with_python_is_ordered() {
        let report = plan(true).validate().expect("静态清单合法");
        let position = |name: &str| {
            report
                .order
                .iter()
                .position(|item| item == name)
                .unwrap_or_else(|| panic!("缺节点 {name}"))
        };
        assert!(position("scene") < position("python"));
        assert!(position("python") < position("lua"));
        assert!(report.optional_missing.is_empty());
    }

    /// 描述层：环检测不需要真实 Pod。
    #[test]
    fn plan_detects_cycle_without_pods() {
        let plan = AssemblyPlan::new(vec![
            NodeSpec::new(
                "a",
                &["a"],
                vec![ServiceRequirement::required("b", "b::ping() -> ()")],
            ),
            NodeSpec::new(
                "b",
                &["b"],
                vec![ServiceRequirement::required("a", "a::ping() -> ()")],
            ),
        ]);
        assert!(matches!(plan.validate(), Err(BrokerError::Cycle { .. })));
    }
}
