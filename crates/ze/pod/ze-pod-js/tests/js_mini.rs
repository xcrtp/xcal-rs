//! B3-Pa/Pb 验收：**纯 JS 适配器**的双向对接，用测试内自建的 mini domain Pod。
//!
//! 生产依赖里没有 `zela-*`；真实 domain 的完整分镜等价性用例在 `zela-app`。
//!
//! ```text
//! 脚本 → 宿主：scene.dot / scene.emit / scene.kind（注入的 mini Service）
//! 宿主 → 脚本：Broker::run_with("js", "zela.script::run(String) -> Any")
//! ```

#![allow(dead_code)] // capability 的 inherent 方法不全部调用。

use std::any::Any;
use std::cell::RefCell;
use std::sync::Arc;

use ze_pod_binding::CapabilityDocs;
use ze_pod_core::{
    Broker, Capability, FunctionId, Handle, Pod, PodError, PodId, PodNode, QualifiedName,
    RegistryPod, ServiceRequirement,
};
use ze_pod_js::{JsPod, RUN_ENTRY};
use ze_pod_macro::{PodType, capability};

// ─── mini domain（只在测试里存在）───────────────────────────────────────

#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "mini.Dot")]
struct Dot {
    x: f32,
    y: f32,
    r: f32,
}

#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "mini.Kind")]
enum Kind {
    Const,
    Weight { value: f32 },
}

#[capability(namespace = "mini", bindings)]
struct MiniApi<'a> {
    /// 创建一个点组件。
    dot: fn(&'a dyn Pod, shape: Dot) -> Handle,
    /// 累加一个数值。
    emit: fn(&'a dyn Pod, value: f32) -> (),
    /// 读累计值。
    total: fn(&'a dyn Pod) -> f32,
    /// 记录一个枚举值（验证 unit 变体是值、带字段变体是构造器）。
    kind: fn(&'a dyn Pod, kind: Kind) -> (),
    /// 记录一个整数（验证 JS 侧显式整数映射：小数 / 越界必须报错）。
    count: fn(&'a dyn Pod, n: u32) -> (),
}

#[derive(Debug, Default, PartialEq)]
struct MiniOutput {
    dots: usize,
    values: Vec<f32>,
    kinds: Vec<String>,
    counts: Vec<u32>,
}

#[derive(Default)]
struct Recorded {
    dots: usize,
    values: Vec<f32>,
    kinds: Vec<String>,
    counts: Vec<u32>,
}

/// 宿主侧 mini Service Pod：**不依赖任何 domain crate**。
struct MiniPod {
    id: PodId,
    registry: RegistryPod,
    rec: RefCell<Recorded>,
}

impl MiniPod {
    fn new() -> Self {
        let id = PodId::next();
        let mut registry = RegistryPod::new();
        registry.register_fn1_named::<Dot, Handle>(
            "mini::dot",
            "mini.Dot",
            "ze_pod_core.Handle",
            mini_dot,
        );
        registry.register_fn1::<f32, ()>("mini::emit", mini_emit);
        registry.register_fn0::<f32>("mini::total", mini_total);
        registry.register_fn1::<Kind, ()>("mini::kind", mini_kind);
        registry.register_fn1::<u32, ()>("mini::count", mini_count);
        // 入口服务的内部交接（协议由 ze-pod-js 的常量给出）。
        registry.register_erased(ze_pod_js::RESET_PATH, &[], "()", |pod, _| {
            *mini(pod).rec.borrow_mut() = Recorded::default();
            Ok(Box::new(()))
        });
        registry.register_erased(
            ze_pod_js::FINISH_PATH,
            &[],
            "Result<Box<dyn Any>, String>",
            |pod, _| Ok(Box::new(mini(pod).finish().map(|out| Box::new(out) as Box<dyn Any>))),
        );

        Self {
            id,
            registry,
            rec: RefCell::new(Recorded::default()),
        }
    }

    fn finish(&self) -> Result<MiniOutput, String> {
        let rec = self.rec.borrow();
        Ok(MiniOutput {
            dots: rec.dots,
            values: rec.values.clone(),
            kinds: rec.kinds.clone(),
            counts: rec.counts.clone(),
        })
    }
}

fn mini(pod: &dyn Pod) -> &MiniPod {
    pod.as_any().downcast_ref::<MiniPod>().expect("MiniPod")
}

fn mini_dot(pod: &dyn Pod, shape: Dot) -> Handle {
    let this = mini(pod);
    this.rec.borrow_mut().dots += 1;
    // 句柄只用于验证"值穿过边界"，slot 编码三个字段的和。
    Handle::new(this.id, (shape.x + shape.y + shape.r) as u64, 1)
}

fn mini_emit(pod: &dyn Pod, value: f32) {
    mini(pod).rec.borrow_mut().values.push(value);
}

fn mini_total(pod: &dyn Pod) -> f32 {
    mini(pod).rec.borrow().values.iter().sum()
}

fn mini_kind(pod: &dyn Pod, kind: Kind) {
    let label = match kind {
        Kind::Const => "Const".to_string(),
        Kind::Weight { value } => format!("Weight({value})"),
    };
    mini(pod).rec.borrow_mut().kinds.push(label);
}

fn mini_count(pod: &dyn Pod, n: u32) {
    mini(pod).rec.borrow_mut().counts.push(n);
}

impl Pod for MiniPod {
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
        match name.name() {
            "__reset" => {
                *self.rec.borrow_mut() = Recorded::default();
                Ok(Box::new(()))
            }
            "__finish" => Ok(Box::new(
                self.finish().map(|out| Box::new(out) as Box<dyn Any>),
            )),
            _ => self.registry.call(id, args),
        }
    }
}

// ─── 装配 ──────────────────────────────────────────────────────────────

/// `scene` 是入口服务约定名（解释器与录制器的协议）。
fn assembly() -> (Arc<MiniPod>, Arc<JsPod>, Broker) {
    let mini = Arc::new(MiniPod::new());
    let js = Arc::new(JsPod::new());
    js.register_capability::<MiniApi>("scene", true);

    let broker = Broker::new(vec![
        // boa 的 `Context` 是 `!Send + !Sync` ⇒ 与 LuaPod 一样不登记 SyncPod 徽章。
        PodNode::new("scene", Arc::clone(&mini) as Arc<dyn Pod>, &["scene"]),
        PodNode::new("js", Arc::clone(&js) as Arc<dyn Pod>, &["script"]),
    ]);
    broker.assemble().expect("mini 装配成功");
    (mini, js, broker)
}

fn run(source: &str) -> MiniOutput {
    let (_, _, broker) = assembly();
    let owned = source.to_string();
    let arg: &dyn Any = &owned;
    broker
        .run_with::<MiniOutput>("js", RUN_ENTRY, &[arg])
        .expect("JS 脚本执行成功")
}

// ─── 用例 ──────────────────────────────────────────────────────────────

/// 双向：JS 调宿主能力、宿主取回结构化结果（与 `lua_mini.rs` 同形）。
#[test]
fn js_calls_host_and_host_runs_js() {
    let out = run(
        r#"
        const dot = Dot({ x: 1.0, y: 2.0, r: 0.5 });
        scene.emit(3.5);
        scene.kind(Kind.Const);
        scene.kind(Kind.Weight({ value: 2.0 }));
        scene.emit(scene.total() + 1.0);
        scene.count(7);
        "#,
    );
    assert_eq!(out.dots, 1);
    assert_eq!(out.values, vec![3.5, 4.5], "宿主返回值被 JS 读回");
    assert_eq!(out.kinds, vec!["Const", "Weight(2)"]);
    assert_eq!(out.counts, vec![7]);
}

/// 适配器的便捷入口（不经 Broker）与 Broker 路径结果一致。
#[test]
fn js_pod_run_is_generic() {
    let (_, js, _) = assembly();
    let out: MiniOutput = js.run("scene.emit(1.5); scene.emit(2.5)").expect("脚本成功");
    assert_eq!(out.values, vec![1.5, 2.5]);
}

/// 注册名 == capability 签名（绑定层与实际函数表不漂移）。
#[test]
fn registered_names_match_signatures() {
    let (_, js, _) = assembly();
    let mut registered = js.registered_global_names("scene");
    let mut expected: Vec<String> = MiniApi::signatures()
        .iter()
        .map(|sig| sig.name.to_string())
        .collect();
    registered.sort();
    expected.sort();
    assert_eq!(registered, expected);
}

/// 文档：`.d.ts` 由 `CapabilityDocs` 生成（与 LuaLS 同源）。
#[test]
fn docs_are_complete_and_structural() {
    assert_eq!(MiniApi::docs().len(), MiniApi::signatures().len());
    for doc in MiniApi::docs() {
        assert!(!doc.description.is_empty(), "`{}` 缺 ///", doc.name);
    }

    let capabilities = [CapabilityDocs {
        name: "mini",
        calls: MiniApi::calls(),
        docs: MiniApi::docs(),
        meta: MiniApi::meta(),
    }];

    let ts = ze_pod_js::d_ts(&capabilities);
    assert!(ts.contains("emit(value: number): void"), "实际：\n{ts}");
    assert!(ts.contains("kind(kind: Kind): void"), "实际：\n{ts}");
    assert!(ts.contains("export interface Dot"), "实际：\n{ts}");
    assert!(ts.contains("export declare function Dot(spec: Dot): Handle"));
    assert!(ts.contains("const Const: Value"), "unit 变体应是值");

    let json: serde_json::Value =
        serde_json::from_str(&ze_pod_binding::capabilities_json(&capabilities)).expect("合法 JSON");
    let functions = json["capabilities"]["mini"]["functions"]
        .as_array()
        .expect("mini functions");
    assert_eq!(functions.len(), MiniApi::signatures().len());
}

/// 未注入 ⇒ `run` 明确失败（默认拒绝，不是空结果）。
#[test]
fn missing_injection_is_explicit() {
    let bare = JsPod::new();
    let err = bare
        .run::<MiniOutput>("1")
        .expect_err("未注入 ⇒ 必须失败");
    assert!(err.to_string().contains("尚未注入"), "实际：{err}");
}

/// 结构体缺字段 ⇒ JS 侧明确报错。
#[test]
fn missing_struct_field_is_an_explicit_error() {
    let (_, _, broker) = assembly();
    let source = "Dot({ x: 1.0, y: 2.0 })".to_string();
    let arg: &dyn Any = &source;
    let err = broker
        .run_with::<MiniOutput>("js", RUN_ENTRY, &[arg])
        .expect_err("缺字段 ⇒ 必须失败");
    assert!(err.to_string().contains("缺少字段"), "实际：{err}");
}

/// **B3-Pb 结构判据**：JS 只有 number ⇒ 整数映射必须显式（小数 / 越界报错）。
#[test]
fn integer_mapping_is_explicit() {
    let (_, _, broker) = assembly();

    for (source, expected) in [
        ("scene.count(1.5)", "必须是整数"),
        ("scene.count(-1)", "超出范围"),
        ("scene.count(4294967296)", "超出范围"),
    ] {
        let owned = source.to_string();
        let arg: &dyn Any = &owned;
        let err = broker
            .run_with::<MiniOutput>("js", RUN_ENTRY, &[arg])
            .expect_err("非法整数 ⇒ 必须失败");
        assert!(
            err.to_string().contains(expected),
            "`{source}` 的错误应含 `{expected}`，实际：{err}"
        );
    }
}

/// 装配描述层：不需要真实 Pod 也能验证 mini 组合的拓扑。
#[test]
fn mini_plan_orders_scene_before_js() {
    let plan = ze_pod_core::AssemblyPlan::new(vec![
        ze_pod_core::NodeSpec::new(
            "js",
            &["script"],
            vec![ServiceRequirement::required(
                "scene",
                "mini::dot(mini.Dot) -> ze_pod_core.Handle",
            )],
        ),
        ze_pod_core::NodeSpec::new("scene", &["scene"], Vec::new()),
    ]);
    let report = plan.validate().expect("纯数据校验成功");
    assert_eq!(report.order, vec!["scene", "js"]);
    assert_eq!(report.edges, vec![("scene".into(), "js".into())]);
}
