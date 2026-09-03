//! Phase C integration: `#[derive(NodeSpec)]` generates a working `Node` +
//! `NodeSpec` impl. The derived nodes are registered, evaluated by
//! `LazyPullEval`, and edited through `apply_property` — reusing the Phase A
//! property-driven executor pattern. Zero hand-written `Node`/`NodeSpec`
//! boilerplate.

use xc_graph_algo::KahnSorter;
use xc_graph_eval::LazyPullEval;
use xc_node::{
    EvalError, EvalErrorPolicy, EvalStrategy, ExecContext, Graph, InputSocketId, Node,
    NodeExecutor, NodeId, OutputSocketId, SimpleGraph,
};
use xc_node_schema::{NodeRegistry, NodeSpec, Property, PropertyError, PropertyValue};

// ---------------------------------------------------------------------------
// Derived nodes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, NodeSpec)]
#[node(
    type_name = "demo.constant.f32",
    display = "Constant",
    category = "Source",
    outputs = ["f32"],
    state = "()",
    dirty = "always"
)]
struct ConstantF32 {
    #[property(label = "value", range = 0.0..=10.0, step = 0.01, default = 1.0)]
    value: f32,
}

#[derive(Debug, Clone, NodeSpec)]
#[node(
    type_name = "demo.add.f32",
    display = "Add",
    category = "Math",
    inputs = ["f32"],
    outputs = ["f32"],
    state = "()",
    dirty = "always"
)]
struct AddF32 {
    #[property(label = "b", range = -5.0..=5.0, step = 0.1, default = 1.0)]
    b: f32,
    // Internal (non-property) state field — ignored by the derive. Verifies the
    // macro only generates code for fields carrying `#[property]`.
    #[allow(dead_code)]
    cached: f32,
}

#[derive(Debug, Clone, NodeSpec)]
#[node(
    type_name = "demo.display.f32",
    display = "Display",
    category = "Sink",
    inputs = ["f32"],
    state = "f32",
    dirty = "always"
)]
struct DisplayF32 {
    #[property(label = "label")]
    label: String,
}

/// A node exercising several property kinds to verify the derive covers the
/// full常用子集 (f32, i64, bool, String, [f32;3], [u8;4]).
#[derive(Debug, Clone, NodeSpec)]
#[node(
    type_name = "demo.mix",
    display = "Mix",
    category = "Misc",
    outputs = ["f32"],
    state = "()"
)]
struct MixNode {
    #[property(label = "freq", range = 0.0..=10.0)]
    freq: f32,
    #[property(label = "count", min = 0, max = 100)]
    count: i32,
    #[property(label = "enabled")]
    enabled: bool,
    #[property(label = "name")]
    name: String,
    #[property(label = "color")]
    color: [u8; 4],
}

// ---------------------------------------------------------------------------
// Property-driven executor (same pattern as Phase A; no per-node downcast —
// reads properties via `NodeRegistry::spec_of`).
// ---------------------------------------------------------------------------

struct PropertyExecutor {
    registry: NodeRegistry,
}

impl NodeExecutor for PropertyExecutor {
    fn execute(&self, node: &dyn Node, ctx: &mut dyn ExecContext) -> Result<(), EvalError> {
        let here = ctx.current_node();
        let spec = self.registry.spec_of(node).ok_or_else(|| {
            EvalError::new(
                format!("no NodeSpec for `{}`", node.meta().type_name),
                here,
                None,
            )
        })?;
        match node.meta().type_name.as_str() {
            "demo.constant.f32" => {
                let v = scalar_field(&spec.properties(), "value")?;
                ctx.set_output(OutputSocketId::new(here, 0), Box::new(v))?;
            }
            "demo.add.f32" => {
                let input = peek_f32(ctx, here, 0)?;
                let b = scalar_field(&spec.properties(), "b")?;
                ctx.set_output(OutputSocketId::new(here, 0), Box::new(input + b))?;
            }
            "demo.display.f32" => {
                let input = peek_f32(ctx, here, 0)?;
                ctx.set_state(Box::new(input))?;
            }
            "demo.mix" => {
                let freq = scalar_field(&spec.properties(), "freq")?;
                ctx.set_output(OutputSocketId::new(here, 0), Box::new(freq))?;
            }
            other => {
                return Err(EvalError::new(
                    format!("unknown node type `{other}`"),
                    here,
                    None,
                ));
            }
        }
        Ok(())
    }
}

fn scalar_field(props: &[(String, Property)], name: &str) -> Result<f32, EvalError> {
    for (n, p) in props {
        if n == name
            && let Property::Scalar(c) = p
        {
            return Ok(c.value);
        }
    }
    Err(EvalError::new(
        format!("missing scalar field `{name}`"),
        NodeId(0),
        None,
    ))
}

fn peek_f32(ctx: &mut dyn ExecContext, node: NodeId, idx: u32) -> Result<f32, EvalError> {
    let v = ctx.peek_input(InputSocketId::new(node, idx))?;
    v.downcast_ref::<f32>()
        .copied()
        .ok_or_else(|| EvalError::new("input not f32", node, None))
}

// ---------------------------------------------------------------------------
// Registry + graph
// ---------------------------------------------------------------------------

fn build_registry() -> NodeRegistry {
    let mut reg = NodeRegistry::new();
    reg.register::<ConstantF32>("Source", ConstantF32::default_node);
    reg.register::<AddF32>("Math", AddF32::default_node);
    reg.register::<DisplayF32>("Sink", DisplayF32::default_node);
    reg.register::<MixNode>("Misc", MixNode::default_node);
    reg
}

fn build_graph() -> (SimpleGraph, NodeId, NodeId, NodeId) {
    let mut g = SimpleGraph::default();
    let c = g.add_node(Box::new(ConstantF32::default()));
    let a = g.add_node(Box::new(AddF32::default()));
    let d = g.add_node(Box::new(DisplayF32::default()));
    g.quick_connect(OutputSocketId::new(c, 0), InputSocketId::new(a, 0))
        .unwrap();
    g.quick_connect(OutputSocketId::new(a, 0), InputSocketId::new(d, 0))
        .unwrap();
    (g, c, a, d)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn derived_nodes_propagate_through_eval() {
    let (mut g, c, a, d) = build_graph();
    let reg = build_registry();
    let exec = PropertyExecutor {
        registry: reg.clone(),
    };
    let mut eval = LazyPullEval::new(Box::new(KahnSorter::new()), EvalErrorPolicy::FailFast);

    // Default Constant(1.0) + Add(1.0) = 2.0.
    eval.evaluate(&g, &exec, Some(OutputSocketId::new(d, 0)))
        .unwrap();
    let out = eval.cached_output(OutputSocketId::new(a, 0)).unwrap();
    assert_eq!(*out.downcast_ref::<f32>().unwrap(), 2.0);

    // Edit both properties via NodeSpec (registry bridge).
    {
        let node = g.get_node_mut(c).unwrap();
        let spec = reg.spec_of_mut(node).unwrap();
        spec.apply_property("value", PropertyValue::Scalar(5.0))
            .unwrap();
    }
    {
        let node = g.get_node_mut(a).unwrap();
        let spec = reg.spec_of_mut(node).unwrap();
        spec.apply_property("b", PropertyValue::Scalar(3.0))
            .unwrap();
    }
    eval.evaluate(&g, &exec, Some(OutputSocketId::new(d, 0)))
        .unwrap();
    let out = eval.cached_output(OutputSocketId::new(a, 0)).unwrap();
    assert_eq!(*out.downcast_ref::<f32>().unwrap(), 8.0); // 5 + 3
}

#[test]
fn derived_properties_listed_and_typed() {
    let reg = build_registry();
    let node = MixNode::default();
    let spec = reg.spec_of(&node).unwrap();
    let props = spec.properties();
    let names: Vec<&str> = props.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["freq", "count", "enabled", "name", "color"]);
    // Each has the right Property variant.
    assert!(matches!(find_prop(&props, "freq"), Property::Scalar(_)));
    assert!(matches!(find_prop(&props, "count"), Property::Integer(_)));
    assert!(matches!(find_prop(&props, "enabled"), Property::Bool(_)));
    assert!(matches!(find_prop(&props, "name"), Property::String(_)));
    assert!(matches!(find_prop(&props, "color"), Property::Color(_)));
}

#[test]
fn derived_apply_property_type_checks() {
    let mut node = MixNode::default();
    // Type mismatch -> error, not panic.
    let err = node
        .apply_property("freq", PropertyValue::Bool(false))
        .unwrap_err();
    assert!(matches!(err, PropertyError::TypeMismatch { .. }));
    // Unknown property -> error.
    let err = node
        .apply_property("nope", PropertyValue::Scalar(1.0))
        .unwrap_err();
    assert!(matches!(err, PropertyError::UnknownProperty(_)));
    // Integer assignment with cast.
    node.apply_property("count", PropertyValue::Integer(42))
        .unwrap();
    assert_eq!(node.count, 42);
    // String assignment.
    node.apply_property("name", PropertyValue::String("hi".into()))
        .unwrap();
    assert_eq!(node.name, "hi");
    // Color assignment.
    node.apply_property("color", PropertyValue::Color([10, 20, 30, 40]))
        .unwrap();
    assert_eq!(node.color, [10, 20, 30, 40]);
}

#[test]
fn derived_spawn_from_registry() {
    let reg = build_registry();
    let node = reg.spawn("demo.constant.f32").unwrap();
    assert_eq!(node.meta().type_name, "demo.constant.f32");
    assert_eq!(node.outputs().len(), 1);
}

#[test]
fn derived_categories() {
    let reg = build_registry();
    let cats = reg.categories();
    let names: Vec<&str> = cats.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["Source", "Math", "Sink", "Misc"]);
}

fn find_prop<'a>(props: &'a [(String, Property)], name: &str) -> &'a Property {
    props
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, p)| p)
        .expect("property present")
}
