//! Broker：把「解释器 / 执行器 / 能力提供者」撮合成一张有向图。
//!
//! # 它不是什么
//!
//! **不是插件框架**：没有动态加载、没有配置文件、没有版本解析、没有服务发现。
//! Pod 集合是编译期已知、App 显式声明的有限集。
//!
//! # 描述层 / 执行层（D3）
//!
//! ```text
//! NodeSpec / AssemblyPlan   ← 纯数据：provides + requires
//!        │  validate()  （无 Pod 也能单测：拓扑序 / 环 / optional 缺失）
//!        ▼
//! Broker                    ← 执行层：校验函数级签名 → 按序注入 → 生命周期
//! ```
//!
//! # 结构性原则
//!
//! **Broker 持有全部 Pod，Pod 之间只在注入后持有 `Arc<dyn Pod>`**。
//! 环在注入前就被拓扑排序拒绝 ⇒ 不会构造出 `Arc` 互持。

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use crate::capability::Capability;
use crate::error::PodError;
use crate::handle::PodHandle;
use crate::inject::{
    Injector, MissingRequirements, Service, ServiceRequirement, SyncInjector,
    validate_requirements,
};
use crate::lifecycle::Lifecycle;
use crate::qualified::QualifiedName;
use crate::pod::{Pod, SyncPod, negotiate_dyn};

/// 描述层的一个节点：**纯数据**（不持有 Pod）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeSpec {
    /// 逻辑名（报告 / 环链 / 排查）。
    pub name: String,
    /// 提供的 Service 名（App 显式声明）。
    pub provides: Vec<String>,
    /// 订阅的 topic 名（扇出成员；空 = 不订阅）。
    pub topics: Vec<String>,
    /// 自述需求（Pod 侧 `Injector::requirements` 的产物，或手写用于纯数据测试）。
    pub requires: Vec<ServiceRequirement>,
}

impl NodeSpec {
    /// 逐字段构造。
    pub fn new(
        name: impl Into<String>,
        provides: &[&str],
        requires: Vec<ServiceRequirement>,
    ) -> Self {
        Self {
            name: name.into(),
            provides: provides.iter().map(|name| (*name).to_string()).collect(),
            topics: Vec::new(),
            requires,
        }
    }

    /// 追加 topic 订阅声明（扇出成员）。
    pub fn with_topics(mut self, topics: &[&str]) -> Self {
        self.topics = topics.iter().map(|topic| (*topic).to_string()).collect();
        self
    }
}

/// 描述层的装配图：**不建真实 Pod 也能验证拓扑 / 环 / optional**。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssemblyPlan {
    pub nodes: Vec<NodeSpec>,
}

impl AssemblyPlan {
    /// 构造。
    pub fn new(nodes: Vec<NodeSpec>) -> Self {
        Self { nodes }
    }

    /// 纯校验：提供者唯一性 → 依赖边 → 拓扑排序（含环检测）→ optional 缺失。
    pub fn validate(&self) -> Result<AssemblyReport, BrokerError> {
        let providers = provider_map(&self.nodes)?;
        let (dependencies, edges, optional_missing) = build_edges(&self.nodes, &providers)?;
        let order = topological_order(&self.nodes, &dependencies)?;
        Ok(AssemblyReport {
            order: order
                .iter()
                .map(|&index| self.nodes[index].name.clone())
                .collect(),
            edges,
            injected: Vec::new(),
            optional_missing,
        })
    }
}

/// Broker 的一个节点：一台 Pod + 它的逻辑身份。
#[derive(Clone)]
pub struct PodNode {
    /// 逻辑名（用于报告 / 环链 / 排查；不参与协商）。
    pub name: String,
    /// Pod 实例。
    pub pod: Arc<dyn Pod>,
    /// **同一个实例**的线程安全视图（仅当注册时登记了 `SyncPod` 徽章）。
    ///
    /// `None` ⇒ 该 Pod 没有声明 `Send + Sync`：跨线程接收方（如 pyo3 闭包）
    /// 必须得到明确的 `NotSyncPod` 错误，而不是静默降级。
    pub sync: Option<Arc<dyn SyncPod>>,
    /// 它提供的 Service 名（App 显式声明；一个 Pod 可提供多个）。
    pub provides: Vec<String>,
    /// 它订阅的 topic 名（扇出成员）。
    pub topics: Vec<String>,
}

impl PodNode {
    /// 逐字段构造（**不登记**线程安全徽章）。
    pub fn new(name: impl Into<String>, pod: Arc<dyn Pod>, provides: &[&str]) -> Self {
        Self {
            name: name.into(),
            pod,
            sync: None,
            provides: provides.iter().map(|name| (*name).to_string()).collect(),
            topics: Vec::new(),
        }
    }

    /// 构造**线程安全**节点：`pod` 同时生成 `Arc<dyn Pod>` 与 `Arc<dyn SyncPod>`，
    /// 两者指向同一个实例（不是两份状态）。
    pub fn new_sync<T>(name: impl Into<String>, pod: Arc<T>, provides: &[&str]) -> Self
    where
        T: Pod + Send + Sync,
    {
        Self {
            name: name.into(),
            pod: Arc::clone(&pod) as Arc<dyn Pod>,
            sync: Some(pod as Arc<dyn SyncPod>),
            provides: provides.iter().map(|name| (*name).to_string()).collect(),
            topics: Vec::new(),
        }
    }

    /// 追加 topic 订阅声明（扇出成员）。
    ///
    /// 硬规则：扇出成员由 **Broker 查询**得到，发布方不得硬编码订阅者列表。
    pub fn with_topics(mut self, topics: &[&str]) -> Self {
        self.topics = topics.iter().map(|topic| (*topic).to_string()).collect();
        self
    }

    /// 该节点的寻址句柄（`Pod::id()` 的包装）。
    pub fn handle(&self) -> PodHandle {
        PodHandle::new(self.pod.id())
    }
}

/// 一次成功装配的可审计结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssemblyReport {
    /// 注入顺序（依赖在前）。
    pub order: Vec<String>,
    /// 依赖边：`(提供方, 消费方)`。
    pub edges: Vec<(String, String)>,
    /// 实际注入：`(消费方, Service 名)`。
    pub injected: Vec<(String, String)>,
    /// optional 需求中没有提供者的项。
    pub optional_missing: Vec<ServiceRequirement>,
}

impl fmt::Display for AssemblyReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "装配顺序: {}", self.order.join(" -> "))?;
        if !self.edges.is_empty() {
            let edges: Vec<String> = self
                .edges
                .iter()
                .map(|(provider, consumer)| format!("{provider} -> {consumer}"))
                .collect();
            writeln!(f, "依赖边: {}", edges.join(", "))?;
        }
        if !self.injected.is_empty() {
            let injected: Vec<String> = self
                .injected
                .iter()
                .map(|(consumer, service)| format!("{consumer} <- {service}"))
                .collect();
            writeln!(f, "已注入: {}", injected.join(", "))?;
        }
        if !self.optional_missing.is_empty() {
            let missing: Vec<String> = self
                .optional_missing
                .iter()
                .map(|req| format!("{} / {}", req.service, req.function))
                .collect();
            writeln!(f, "optional 缺失: {}", missing.join(", "))?;
        }
        Ok(())
    }
}

/// 装配失败：全部是**明确错误**（不是静默 / 不是泄漏）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrokerError {
    /// 同一个 Service 被多个节点提供。
    DuplicateProvider {
        service: String,
        first: String,
        second: String,
    },
    /// 依赖环：`chain` 首尾相同，例如 `["lua", "python", "lua"]`。
    Cycle { chain: Vec<String> },
    /// 必需能力没有提供者。
    MissingRequired {
        service: String,
        function: String,
        required_by: String,
    },
    /// 有需求却没有暴露 `Injector`（默认拒绝的 Pod 不能依赖别人）。
    RequiresButNoInjector { node: String },
    /// `Injector::inject` 返回错误。
    Injection { node: String, source: PodError },
    /// 有提供者，但函数级签名不满足（缺函数）。
    Missing {
        node: String,
        missing: MissingRequirements,
    },
    /// `run` 的目标节点不存在。
    UnknownNode { node: String },
    /// `run` 的入口函数名不合法。
    BadEntry { entry: String, message: String },
    /// `run` 的目标节点没有该入口函数。
    UnknownEntry { node: String, entry: String },
    /// 入口调用失败。
    Run { node: String, source: PodError },
    /// 入口返回值不是期望的 `T`。
    BadOutput { node: String, expected: String },
    /// 查询能力时没有任何提供者。
    NoProvider { capability: String },
    /// 查询能力时有多个提供者，且调用方未显式指定节点。
    AmbiguousProvider {
        capability: String,
        providers: Vec<String>,
    },
    /// 显式指定的节点不提供该能力。
    UnsupportedCapability { node: String, capability: String },
    /// 请求线程安全视图，但该节点没有登记 `SyncPod` 徽章。
    NotSyncPod { node: String },
}

impl fmt::Display for BrokerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateProvider {
                service,
                first,
                second,
            } => write!(f, "Service `{service}` 同时由 `{first}` 与 `{second}` 提供"),
            Self::Cycle { chain } => {
                write!(f, "检测到 Pod 依赖环：{}", chain.join(" -> "))
            }
            Self::MissingRequired {
                service,
                function,
                required_by,
            } => write!(
                f,
                "`{required_by}` 需要 `{service}` 的 `{function}`，但没有提供者"
            ),
            Self::RequiresButNoInjector { node } => write!(
                f,
                "`{node}` 声明了宿主能力需求，但没有暴露 Injector（默认拒绝注入）"
            ),
            Self::Injection { node, source } => {
                write!(f, "向 `{node}` 注入失败：{source}")
            }
            Self::Missing { node, missing } => {
                write!(f, "`{node}` 缺少必需能力：{missing}")
            }
            Self::UnknownNode { node } => write!(f, "装配图中没有节点 `{node}`"),
            Self::BadEntry { entry, message } => {
                write!(f, "入口函数名 `{entry}` 不合法：{message}")
            }
            Self::UnknownEntry { node, entry } => {
                write!(f, "`{node}` 没有入口函数 `{entry}`")
            }
            Self::Run { node, source } => write!(f, "调用 `{node}` 失败：{source}"),
            Self::BadOutput { node, expected } => {
                write!(f, "`{node}` 的返回值不是 `{expected}`")
            }
            Self::NoProvider { capability } => {
                write!(f, "能力 `{capability}` 没有任何提供者")
            }
            Self::AmbiguousProvider {
                capability,
                providers,
            } => write!(
                f,
                "能力 `{capability}` 有多个提供者（{}），必须显式指定节点",
                providers.join(", ")
            ),
            Self::UnsupportedCapability { node, capability } => {
                write!(f, "`{node}` 不提供能力 `{capability}`")
            }
            Self::NotSyncPod { node } => write!(
                f,
                "`{node}` 没有登记 SyncPod 徽章（不是 Send + Sync），不能跨线程注入"
            ),
        }
    }
}

impl std::error::Error for BrokerError {}

/// 装配器：**撮合 + 顺序 + 环检测 + 注入 + 生命周期 + 跑入口**。
pub struct Broker {
    nodes: Vec<PodNode>,
}

impl Broker {
    /// 用显式声明的节点构造（顺序只影响报告的确定性，不影响正确性）。
    pub fn new(nodes: Vec<PodNode>) -> Self {
        Self { nodes }
    }

    /// 只读节点列表。
    pub fn nodes(&self) -> &[PodNode] {
        &self.nodes
    }

    // ─── 查询（注册表即服务发现；对外只暴露「查询」）─────────────────────

    /// 查询：所有提供能力 `C` 的节点（按声明序）。
    ///
    /// 「注册表即发现」——本方法就是 broker 对外暴露的唯一发现入口，
    /// 不另开第二套 `Discovery` 概念。
    pub fn providers_of<'c, C>(&'c self) -> Vec<&'c PodNode>
    where
        C: Capability<'c>,
    {
        self.nodes
            .iter()
            .filter(|node| negotiate_dyn::<C>(&*node.pod).is_some())
            .collect()
    }

    /// 路由策略 1（显式指定）：在指定节点上协商能力。
    ///
    /// 确定、可测试；节点不存在或能力缺失都是**明确错误**。
    pub fn negotiate_on<'c, C>(&'c self, node: &str) -> Result<C, BrokerError>
    where
        C: Capability<'c>,
    {
        let target = self
            .nodes
            .iter()
            .find(|item| item.name == node)
            .ok_or_else(|| BrokerError::UnknownNode {
                node: node.to_string(),
            })?;
        negotiate_dyn::<C>(&*target.pod).ok_or_else(|| {
            BrokerError::UnsupportedCapability {
                node: target.name.clone(),
                capability: capability_label::<'c, C>(),
            }
        })
    }

    /// 路由策略 2/3（唯一则自动、多个未指定 ⇒ 明确报错）。
    pub fn negotiate_unique<'c, C>(&'c self) -> Result<C, BrokerError>
    where
        C: Capability<'c>,
    {
        let providers = self.providers_of::<C>();
        match providers.len() {
            0 => Err(BrokerError::NoProvider {
                capability: capability_label::<'c, C>(),
            }),
            1 => Ok(negotiate_dyn::<C>(&*providers[0].pod)
                .expect("providers_of 已判定该节点提供能力")),
            _ => Err(BrokerError::AmbiguousProvider {
                capability: capability_label::<'c, C>(),
                providers: providers
                    .iter()
                    .map(|node| node.name.clone())
                    .collect(),
            }),
        }
    }

    /// **topic 成员查询**：返回订阅了 `topic` 的节点句柄，**按 `PodId` 升序**。
    ///
    /// 这是「广播 = 查询 + 迭代单播」的唯一成员来源：
    /// - 发布方**不得**硬编码订阅者句柄列表；
    /// - 扇出顺序 = 本方法返回顺序（全序，可复现）。
    pub fn topic_members(&self, topic: &str) -> Vec<PodHandle> {
        let mut members: Vec<PodHandle> = self
            .nodes
            .iter()
            .filter(|node| node.topics.iter().any(|name| name == topic))
            .map(PodNode::handle)
            .collect();
        // 硬规则：按 PodId 排序（`PodHandle` 的 Ord 就是 PodId 的 Ord）。
        members.sort_unstable();
        members
    }

    /// 按寻址句柄查节点。
    pub fn node_by_handle(&self, handle: PodHandle) -> Option<&PodNode> {
        self.nodes.iter().find(|node| node.handle() == handle)
    }

    /// **线程安全视图查询**：按句柄取同一实例的 `Arc<dyn SyncPod>`。
    ///
    /// 节点未登记 `SyncPod` 徽章 ⇒ **明确** [`BrokerError::NotSyncPod`]
    /// （不静默降级成 `Arc<dyn Pod>`：调用方要跨线程，降级只会把错误推迟到运行时）。
    pub fn sync_pod(&self, handle: PodHandle) -> Result<Arc<dyn SyncPod>, BrokerError> {
        let node = self
            .node_by_handle(handle)
            .ok_or_else(|| BrokerError::UnknownNode {
                node: handle.to_string(),
            })?;
        node.sync
            .clone()
            .ok_or_else(|| BrokerError::NotSyncPod {
                node: node.name.clone(),
            })
    }

    /// 路由策略 1 的句柄版本：在 `handle` 指向的节点上协商能力。
    ///
    /// topic 扇出用它（成员身份来自查询，不是发布方持有的 `Arc<dyn Pod>`）。
    pub fn negotiate_on_handle<'c, C>(&'c self, handle: PodHandle) -> Result<C, BrokerError>
    where
        C: Capability<'c>,
    {
        let target = self
            .node_by_handle(handle)
            .ok_or_else(|| BrokerError::UnknownNode {
                node: handle.to_string(),
            })?;
        negotiate_dyn::<C>(&*target.pod).ok_or_else(|| {
            BrokerError::UnsupportedCapability {
                node: target.name.clone(),
                capability: capability_label::<'c, C>(),
            }
        })
    }

    /// 从节点自述需求提取出**纯数据描述**（可脱离 Broker 单测 / 存档）。
    pub fn plan(&self) -> AssemblyPlan {
        AssemblyPlan::new(
            self.nodes
                .iter()
                .map(|node| NodeSpec {
                    name: node.name.clone(),
                    provides: node.provides.clone(),
                    topics: node.topics.clone(),
                    requires: negotiate_dyn::<Injector>(&*node.pod)
                        .map(|injector| injector.requirements())
                        .unwrap_or_default(),
                })
                .collect(),
        )
    }

    /// 装配：描述层校验 → 函数级校验 → 按序注入 → 报告。
    pub fn assemble(&self) -> Result<AssemblyReport, BrokerError> {
        let plan = self.plan();
        let mut report = plan.validate()?;

        let by_name: BTreeMap<&str, &PodNode> = self
            .nodes
            .iter()
            .map(|node| (node.name.as_str(), node))
            .collect();

        // 函数级校验（有提供者但签名不满足 ⇒ 明确错误）。
        let services: Vec<Service> = plan
            .nodes
            .iter()
            .flat_map(|spec| {
                let node = by_name[spec.name.as_str()];
                spec.provides
                    .iter()
                    .map(|service| Service::new(service.clone(), Arc::clone(&node.pod)))
            })
            .collect();
        for spec in &plan.nodes {
            if spec.requires.is_empty() {
                continue;
            }
            validate_requirements(&spec.requires, &services).map_err(|missing| {
                BrokerError::Missing {
                    node: spec.name.clone(),
                    missing,
                }
            })?;
        }

        // 按序注入：只注入该 Pod 实际用到的 Service。
        let mut providers: BTreeMap<&str, &PodNode> = BTreeMap::new();
        for spec in &plan.nodes {
            let node = by_name[spec.name.as_str()];
            for service in &spec.provides {
                providers.insert(service.as_str(), node);
            }
        }
        for name in &report.order {
            let spec = plan
                .nodes
                .iter()
                .find(|spec| &spec.name == name)
                .expect("拓扑序来自描述层");
            if spec.requires.is_empty() {
                continue;
            }
            let node = by_name[name.as_str()];
            let injector: Injector<'_> =
                negotiate_dyn(&*node.pod).ok_or_else(|| BrokerError::RequiresButNoInjector {
                    node: node.name.clone(),
                })?;
            let mut seen = BTreeSet::new();
            for requirement in &spec.requires {
                if !seen.insert(requirement.service.clone()) {
                    continue;
                }
                let Some(provider) = providers.get(requirement.service.as_str()) else {
                    continue; // optional 缺失（必需项已在校验阶段报错）
                };
                // 提供方登记了 SyncPod 且消费方暴露 SyncInjector ⇒ 交付线程安全视图。
                // 否则走普通注入（`Arc<dyn Pod>`），行为与引入 SyncPod 之前一致。
                let sync_injection = provider.sync.as_ref().and_then(|sync| {
                    negotiate_dyn::<SyncInjector>(&*node.pod)
                        .map(|injector| (injector, Arc::clone(sync)))
                });
                let injected = match sync_injection {
                    Some((injector, sync)) => injector.inject(&requirement.service, sync),
                    None => injector.inject(&requirement.service, Arc::clone(&provider.pod)),
                };
                injected.map_err(|source| BrokerError::Injection {
                    node: node.name.clone(),
                    source,
                })?;
                report
                    .injected
                    .push((node.name.clone(), requirement.service.clone()));
            }
        }

        Ok(report)
    }

    /// 在已装配完成的图上，调用 `target` 的零参 `entry`，取回 `T`。
    ///
    /// 入口名必须是**完整函数名**（含参数与返回类型），例如
    /// `"xcal.script::run(String) -> Any"`；带参入口用 [`Self::run_with`]。
    pub fn run<T: 'static>(&self, target: &str, entry: &str) -> Result<T, BrokerError> {
        self.run_with(target, entry, &[])
    }

    /// 带参版本：`Pod::call` 的类型擦除实参原样透传。
    pub fn run_with<T: 'static>(
        &self,
        target: &str,
        entry: &str,
        args: &[&dyn Any],
    ) -> Result<T, BrokerError> {
        let node = self
            .nodes
            .iter()
            .find(|node| node.name == target)
            .ok_or_else(|| BrokerError::UnknownNode {
                node: target.to_string(),
            })?;
        let name = QualifiedName::parse(entry).map_err(|error| BrokerError::BadEntry {
            entry: entry.to_string(),
            message: error.to_string(),
        })?;
        let id = node
            .pod
            .function_id(&name)
            .ok_or_else(|| BrokerError::UnknownEntry {
                node: target.to_string(),
                entry: entry.to_string(),
            })?;
        let out = node.pod.call(id, args).map_err(|source| BrokerError::Run {
            node: target.to_string(),
            source,
        })?;
        out.downcast::<T>()
            .map(|boxed| *boxed)
            .map_err(|_| BrokerError::BadOutput {
                node: target.to_string(),
                expected: std::any::type_name::<T>().to_string(),
            })
    }

    /// 对每个暴露 `Lifecycle` 的 Pod 调 `dispose`（收集错误，不中断其余 Pod）。
    pub fn dispose(&self) -> Vec<(String, PodError)> {
        self.run_lifecycle(|lifecycle| lifecycle.dispose())
    }

    /// 对每个暴露 `Lifecycle` 的 Pod 调 `reset`。
    pub fn reset(&self) -> Vec<(String, PodError)> {
        self.run_lifecycle(|lifecycle| lifecycle.reset())
    }

    fn run_lifecycle(
        &self,
        call: impl Fn(&Lifecycle<'_>) -> Result<(), PodError>,
    ) -> Vec<(String, PodError)> {
        let mut errors = Vec::new();
        for node in &self.nodes {
            if let Some(lifecycle) = negotiate_dyn::<Lifecycle>(&*node.pod) {
                if let Err(error) = call(&lifecycle) {
                    errors.push((node.name.clone(), error));
                }
            }
        }
        errors
    }
}

/// capability 的可读标签（错误信息用）：取首条签名的完整函数名。
fn capability_label<'c, C>() -> String
where
    C: Capability<'c>,
{
    C::signatures()
        .first()
        .map(|sig| sig.qualified().to_string())
        .unwrap_or_else(|| std::any::type_name::<C>().to_string())
}

fn provider_map(nodes: &[NodeSpec]) -> Result<BTreeMap<String, usize>, BrokerError> {
    let mut providers = BTreeMap::new();
    for (index, node) in nodes.iter().enumerate() {
        for service in &node.provides {
            if let Some(previous) = providers.insert(service.clone(), index) {
                return Err(BrokerError::DuplicateProvider {
                    service: service.clone(),
                    first: nodes[previous].name.clone(),
                    second: node.name.clone(),
                });
            }
        }
    }
    Ok(providers)
}

/// 依赖边：`consumer → providers`；同时返回去重后的可见边与 optional 缺失。
fn build_edges(
    nodes: &[NodeSpec],
    providers: &BTreeMap<String, usize>,
) -> Result<
    (
        Vec<BTreeSet<usize>>,
        Vec<(String, String)>,
        Vec<ServiceRequirement>,
    ),
    BrokerError,
> {
    let mut dependencies = vec![BTreeSet::new(); nodes.len()];
    let mut edges = BTreeSet::new();
    let mut optional_missing = Vec::new();

    for (consumer, node) in nodes.iter().enumerate() {
        for requirement in &node.requires {
            match providers.get(&requirement.service) {
                Some(&provider) => {
                    dependencies[consumer].insert(provider);
                    edges.insert((nodes[provider].name.clone(), node.name.clone()));
                }
                None if requirement.required => {
                    return Err(BrokerError::MissingRequired {
                        service: requirement.service.clone(),
                        function: requirement.function.clone(),
                        required_by: node.name.clone(),
                    });
                }
                None => optional_missing.push(requirement.clone()),
            }
        }
    }

    Ok((dependencies, edges.into_iter().collect(), optional_missing))
}

/// DFS 拓扑排序；遇到灰色节点 ⇒ 明确环错误（含参与链）。
fn topological_order(
    nodes: &[NodeSpec],
    dependencies: &[BTreeSet<usize>],
) -> Result<Vec<usize>, BrokerError> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        White,
        Gray,
        Black,
    }

    fn visit(
        index: usize,
        nodes: &[NodeSpec],
        dependencies: &[BTreeSet<usize>],
        marks: &mut [Mark],
        stack: &mut Vec<usize>,
        order: &mut Vec<usize>,
    ) -> Result<(), BrokerError> {
        match marks[index] {
            Mark::Black => return Ok(()),
            Mark::Gray => {
                let start = stack.iter().position(|&item| item == index).unwrap_or(0);
                let mut chain: Vec<String> = stack[start..]
                    .iter()
                    .map(|&item| nodes[item].name.clone())
                    .collect();
                chain.push(nodes[index].name.clone());
                return Err(BrokerError::Cycle { chain });
            }
            Mark::White => {}
        }

        marks[index] = Mark::Gray;
        stack.push(index);
        for &dependency in &dependencies[index] {
            visit(dependency, nodes, dependencies, marks, stack, order)?;
        }
        stack.pop();
        marks[index] = Mark::Black;
        order.push(index);
        Ok(())
    }

    let mut marks = vec![Mark::White; nodes.len()];
    let mut stack = Vec::new();
    let mut order = Vec::new();
    for index in 0..nodes.len() {
        visit(
            index,
            nodes,
            dependencies,
            &mut marks,
            &mut stack,
            &mut order,
        )?;
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::cell::RefCell;

    use super::*;
    use crate::capability::FnSig;
    use crate::handle::PodId;
    use crate::inject::INJECTOR_NAMESPACE;
    use crate::pod::FunctionId;

    fn injector_names() -> [QualifiedName; 2] {
        [
            FnSig::new(
                INJECTOR_NAMESPACE,
                "requirements",
                &[],
                "ServiceRequirements",
            )
            .qualified(),
            FnSig::new(
                INJECTOR_NAMESPACE,
                "inject",
                &["ServiceName", "ServicePod"],
                "InjectResult",
            )
            .qualified(),
        ]
    }

    /// 测试 Pod：自述需求 + 记录被注入了什么。
    struct TestPod {
        id: PodId,
        names: Vec<QualifiedName>,
        requirements: Vec<ServiceRequirement>,
        injected: RefCell<Vec<String>>,
        requirements_fn: fn(&dyn Pod) -> Vec<ServiceRequirement>,
        inject_fn: fn(&dyn Pod, String, Arc<dyn Pod>) -> Result<(), PodError>,
    }

    impl TestPod {
        fn new(requirements: Vec<ServiceRequirement>, provided_functions: &[&str]) -> Arc<Self> {
            let mut names = injector_names().to_vec();
            for function in provided_functions {
                names.push(
                    QualifiedName::parse(function)
                        .unwrap_or_else(|_| panic!("非法函数名：{function}")),
                );
            }
            Arc::new(Self {
                id: PodId::next(),
                names,
                requirements,
                injected: RefCell::new(Vec::new()),
                requirements_fn: test_requirements,
                inject_fn: test_inject,
            })
        }
    }

    fn test_requirements(pod: &dyn Pod) -> Vec<ServiceRequirement> {
        pod.as_any()
            .downcast_ref::<TestPod>()
            .expect("TestPod")
            .requirements
            .clone()
    }

    fn test_inject(pod: &dyn Pod, service: String, _service_pod: Arc<dyn Pod>) -> Result<(), PodError> {
        pod.as_any()
            .downcast_ref::<TestPod>()
            .expect("TestPod")
            .injected
            .borrow_mut()
            .push(service);
        Ok(())
    }

    impl Pod for TestPod {
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
            } else {
                None
            }
        }

        fn call(&self, id: FunctionId, _args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
            Err(PodError::CallFailed(format!("TestPod 不支持 {id}")))
        }
    }

    fn node(
        name: &str,
        requirements: Vec<ServiceRequirement>,
        provides: &[&str],
        functions: &[&str],
    ) -> PodNode {
        PodNode::new(
            name,
            TestPod::new(requirements, functions) as Arc<dyn Pod>,
            provides,
        )
    }

    /// 描述层：**不建 Pod** 也能验证拓扑 / optional。
    #[test]
    fn assembly_plan_is_pure_data() {
        let plan = AssemblyPlan::new(vec![
            NodeSpec::new(
                "lua",
                &[],
                vec![
                    ServiceRequirement::required("scene", "scene::spawn() -> ()"),
                    ServiceRequirement::optional("python", "python::linspace() -> ()"),
                ],
            ),
            NodeSpec::new("scene", &["scene"], vec![]),
            NodeSpec::new("content", &["content"], vec![]),
        ]);
        let report = plan.validate().expect("纯数据校验成功");
        let position = |name: &str| report.order.iter().position(|item| item == name).unwrap();
        assert!(position("scene") < position("lua"));
        assert_eq!(report.edges.len(), 1);
        assert_eq!(report.optional_missing.len(), 1);
        assert!(report.injected.is_empty(), "描述层不做注入");
    }

    /// 描述层：环检测不需要真实 Pod。
    #[test]
    fn assembly_plan_detects_cycle_without_pods() {
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
        match plan.validate().unwrap_err() {
            BrokerError::Cycle { chain } => assert_eq!(chain, vec!["a", "b", "a"]),
            other => panic!("期望 Cycle，实际 {other}"),
        }
    }

    /// 环检测：A → B → A 必须得到明确错误（含链）。
    #[test]
    fn cycle_is_an_explicit_error() {
        let a = node(
            "a",
            vec![ServiceRequirement::required("b", "b::ping() -> ()")],
            &["a"],
            &["a::ping() -> ()"],
        );
        let b = node(
            "b",
            vec![ServiceRequirement::required("a", "a::ping() -> ()")],
            &["b"],
            &["b::ping() -> ()"],
        );
        let error = Broker::new(vec![a, b]).assemble().unwrap_err();
        match error {
            BrokerError::Cycle { chain } => {
                assert_eq!(chain, vec!["a", "b", "a"], "环链必须可读");
            }
            other => panic!("期望 Cycle，实际 {other}"),
        }
    }

    /// 拓扑顺序：provider 必须在 consumer 之前；optional 缺失不阻断。
    #[test]
    fn assembly_orders_providers_before_consumers() {
        let lua = node(
            "lua",
            vec![
                ServiceRequirement::required("scene", "scene::spawn() -> ()"),
                ServiceRequirement::optional("python", "python::linspace() -> ()"),
            ],
            &["script"],
            &[],
        );
        let scene = node("scene", vec![], &["scene"], &["scene::spawn() -> ()"]);
        let python = node(
            "python",
            vec![ServiceRequirement::required(
                "scene",
                "scene::spawn() -> ()",
            )],
            &["python"],
            &["python::linspace() -> ()"],
        );

        let report = Broker::new(vec![lua, scene, python])
            .assemble()
            .expect("装配成功");
        let position = |name: &str| report.order.iter().position(|item| item == name).unwrap();
        assert!(position("scene") < position("lua"));
        assert!(position("scene") < position("python"));
        assert!(position("python") < position("lua"));
        assert_eq!(
            report.edges.len(),
            3,
            "scene→lua / scene→python / python→lua"
        );
        assert_eq!(report.optional_missing.len(), 0);
    }

    /// optional 缺失：装配通过、不产生边、报告里记账。
    #[test]
    fn missing_optional_does_not_block() {
        let lua = node(
            "lua",
            vec![ServiceRequirement::optional(
                "python",
                "python::linspace() -> ()",
            )],
            &["script"],
            &[],
        );
        let report = Broker::new(vec![lua])
            .assemble()
            .expect("optional 缺失可装配");
        assert_eq!(report.optional_missing.len(), 1);
        assert!(report.edges.is_empty());
    }

    /// 必需项缺失：明确错误，指出需求方。
    #[test]
    fn missing_required_is_an_explicit_error() {
        let lua = node(
            "lua",
            vec![ServiceRequirement::required(
                "scene",
                "scene::spawn() -> ()",
            )],
            &["script"],
            &[],
        );
        let error = Broker::new(vec![lua]).assemble().unwrap_err();
        assert!(matches!(
            error,
            BrokerError::MissingRequired { ref required_by, .. } if required_by == "lua"
        ));
    }

    /// 重复提供者：明确错误。
    #[test]
    fn duplicate_provider_is_an_explicit_error() {
        let first = node("first", vec![], &["scene"], &[]);
        let second = node("second", vec![], &["scene"], &[]);
        let error = Broker::new(vec![first, second]).assemble().unwrap_err();
        assert!(matches!(error, BrokerError::DuplicateProvider { .. }));
    }

    /// 注入只发生在需要的边上，并按拓扑顺序。
    #[test]
    fn injection_follows_edges() {
        let lua = node(
            "lua",
            vec![ServiceRequirement::required(
                "scene",
                "scene::spawn() -> ()",
            )],
            &["script"],
            &[],
        );
        let scene = node("scene", vec![], &["scene"], &["scene::spawn() -> ()"]);
        let lua_pod = Arc::clone(&lua.pod);
        let report = Broker::new(vec![lua, scene]).assemble().expect("装配成功");
        assert_eq!(
            report.injected,
            vec![("lua".to_string(), "scene".to_string())]
        );
        let injected = lua_pod
            .as_any()
            .downcast_ref::<TestPod>()
            .unwrap()
            .injected
            .borrow();
        assert_eq!(*injected, vec!["scene".to_string()]);
    }

    /// `run` 的明确错误：未知节点 / 未知入口 / 返回类型不符。
    #[test]
    fn run_errors_are_explicit() {
        let broker = Broker::new(vec![node("lua", vec![], &[], &[])]);
        assert!(matches!(
            broker.run::<()>("missing", "x::run() -> ()"),
            Err(BrokerError::UnknownNode { .. })
        ));
        assert!(matches!(
            broker.run::<()>("lua", "x::run() -> ()"),
            Err(BrokerError::UnknownEntry { .. })
        ));
    }

    // ─── 路由策略（§3.5）：显式指定 / 唯一则自动 / 多个未指定 ⇒ 报错 ─────

    fn lifecycle_node(name: &str) -> PodNode {
        fn dispose(_: &dyn Pod) -> Result<(), PodError> {
            Ok(())
        }
        fn reset(_: &dyn Pod) -> Result<(), PodError> {
            Ok(())
        }
        let mut pod = crate::registry::RegistryPod::new();
        pod.register_fn0_named::<Result<(), PodError>>(
            "ze_pod_core::lifecycle::dispose",
            "LifecycleResult",
            dispose,
        );
        pod.register_fn0_named::<Result<(), PodError>>(
            "ze_pod_core::lifecycle::reset",
            "LifecycleResult",
            reset,
        );
        PodNode::new(name, Arc::new(pod) as Arc<dyn Pod>, &[])
    }

    #[test]
    fn routing_is_explicit_or_unique() {
        use crate::lifecycle::Lifecycle;

        // 唯一 ⇒ 自动。
        let single = Broker::new(vec![lifecycle_node("only")]);
        assert!(single.negotiate_unique::<Lifecycle<'_>>().is_ok());
        assert_eq!(single.providers_of::<Lifecycle<'_>>().len(), 1);

        // 显式指定 ⇒ 确定。
        let broker = Broker::new(vec![lifecycle_node("a"), lifecycle_node("b")]);
        assert!(broker.negotiate_on::<Lifecycle<'_>>("a").is_ok());
        assert!(matches!(
            broker.negotiate_on::<Lifecycle<'_>>("missing"),
            Err(BrokerError::UnknownNode { .. })
        ));

        // 多个提供者、未指定 ⇒ **明确报错**（不是随便挑一个）。
        match broker.negotiate_unique::<Lifecycle<'_>>() {
            Err(BrokerError::AmbiguousProvider {
                capability,
                providers,
            }) => {
                assert!(capability.contains("dispose"), "错误里要能看出是哪个能力");
                assert_eq!(providers, vec!["a".to_string(), "b".to_string()]);
            }
            Ok(_) => panic!("多个提供者未指定时必须报错，实际协商成功"),
            Err(other) => panic!("期望 AmbiguousProvider，实际 {other}"),
        }

        // 无提供者 ⇒ 明确报错。
        let empty = Broker::new(vec![]);
        assert!(matches!(
            empty.negotiate_unique::<Lifecycle<'_>>(),
            Err(BrokerError::NoProvider { .. })
        ));
    }

    // ─── P1：SyncPod 徽章 ─────────────────────────────────────────────

    /// 登记徽章的节点能取到**同一实例**的线程安全视图。
    #[test]
    fn sync_pod_returns_the_same_instance() {
        let registry = Arc::new(crate::registry::RegistryPod::new());
        let node = PodNode::new_sync("sync", Arc::clone(&registry), &[]);
        let handle = node.handle();
        let broker = Broker::new(vec![node]);

        let sync = broker.sync_pod(handle).expect("已登记 SyncPod 徽章");
        assert_eq!(sync.id(), registry.id(), "必须是同一个 Pod 实例");
    }

    /// 未登记徽章 ⇒ **明确** `NotSyncPod`（不静默降级）。
    #[test]
    fn sync_pod_without_badge_is_an_explicit_error() {
        let pod: Arc<dyn Pod> = Arc::new(crate::registry::RegistryPod::new());
        let node = PodNode::new("plain", pod, &[]);
        let handle = node.handle();
        let error = match Broker::new(vec![node]).sync_pod(handle) {
            Ok(_) => panic!("未登记徽章时必须报 NotSyncPod"),
            Err(error) => error,
        };
        assert!(
            matches!(error, BrokerError::NotSyncPod { ref node } if node == "plain"),
            "期望 NotSyncPod，实际 {error:?}"
        );
    }

    /// 未知句柄 ⇒ 明确 `UnknownNode`。
    #[test]
    fn sync_pod_unknown_handle_is_an_explicit_error() {
        let broker = Broker::new(vec![]);
        let error = match broker.sync_pod(PodHandle::new(PodId::new(999))) {
            Ok(_) => panic!("未知句柄必须报 UnknownNode"),
            Err(error) => error,
        };
        assert!(matches!(error, BrokerError::UnknownNode { .. }));
    }
}
