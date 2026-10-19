//! Headless (no-egui) integration test for `zela-node-ui`.
//!
//! Builds a `Constant -> Add -> Display` graph out of hand-written `NodeSpec`
//! nodes, runs [`ze_graph_eval::LazyPullEval`], and asserts that editing a
//! property via [`NodeSpec::apply_property`] propagates to downstream node
//! outputs/state on the next eval pass. This is the plan's Phase A acceptance:
//! the property model is fully decoupled from any renderer.

use std::any::Any;

use ze_graph_algo::KahnSorter;
use ze_graph_eval::LazyPullEval;
use ze_node_core::{
    DirtyPolicy, EvalError, EvalErrorPolicy, EvalStrategy, ExecContext, Graph, InputSocket,
    InputSocketId, NamedTypeTag, Node, NodeExecutor, NodeId, NodeMeta, OutputSocket,
    OutputSocketId, SimpleGraph, TypeTag,
};
use ze_node_schema::{
    NodeRegistry, NodeSpec, Property, PropertyError, PropertyValue, ScalarConfig, StringConfig,
};

// ---------------------------------------------------------------------------
// Demo nodes (hand-written NodeSpec impls)
// ---------------------------------------------------------------------------

fn f32_tag() -> Box<dyn TypeTag> {
    Box::new(NamedTypeTag::new("f32"))
}

macro_rules! impl_as_any {
    () => {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    };
}

/// 0 in, 1 out: outputs its `value` property.
#[derive(Debug)]
struct ConstantF32 {
    meta: NodeMeta,
    outputs: Vec<OutputSocket>,
    value: f32,
}
impl ConstantF32 {
    fn new(value: f32) -> Self {
        Self {
            meta: NodeMeta::new("demo.constant.f32", "Constant", 1),
            outputs: vec![OutputSocket::new(f32_tag()).with_index(0)],
            value,
        }
    }
}
impl Node for ConstantF32 {
    impl_as_any!();
    fn meta(&self) -> &NodeMeta {
        &self.meta
    }
    fn inputs(&self) -> &[InputSocket] {
        &[]
    }
    fn outputs(&self) -> &[OutputSocket] {
        &self.outputs
    }
    fn dirty_policy(&self) -> DirtyPolicy {
        DirtyPolicy::Always
    }
    fn state_type(&self) -> Box<dyn TypeTag> {
        Box::new(NamedTypeTag::new("()"))
    }
}
impl NodeSpec for ConstantF32 {
    fn properties(&self) -> Vec<(String, Property)> {
        vec![(
            "value".into(),
            Property::Scalar(ScalarConfig {
                label: "value".into(),
                value: self.value,
                min: -10.0,
                max: 10.0,
                step: Some(0.01),
                unit: None,
            }),
        )]
    }
    fn apply_property(&mut self, name: &str, value: PropertyValue) -> Result<(), PropertyError> {
        if name == "value" {
            let PropertyValue::Scalar(v) = value else {
                return Err(PropertyError::TypeMismatch {
                    expected: "scalar".into(),
                    got: "non-scalar".into(),
                });
            };
            self.value = v;
            Ok(())
        } else {
            Err(PropertyError::UnknownProperty(name.into()))
        }
    }
}

/// 1 in, 1 out: output = input + `b` property.
#[derive(Debug)]
struct AddF32 {
    meta: NodeMeta,
    inputs: Vec<InputSocket>,
    outputs: Vec<OutputSocket>,
    b: f32,
}
impl AddF32 {
    fn new(b: f32) -> Self {
        Self {
            meta: NodeMeta::new("demo.add.f32", "Add", 1),
            inputs: vec![InputSocket::new(f32_tag()).with_index(0)],
            outputs: vec![OutputSocket::new(f32_tag()).with_index(0)],
            b,
        }
    }
}
impl Node for AddF32 {
    impl_as_any!();
    fn meta(&self) -> &NodeMeta {
        &self.meta
    }
    fn inputs(&self) -> &[InputSocket] {
        &self.inputs
    }
    fn outputs(&self) -> &[OutputSocket] {
        &self.outputs
    }
    fn dirty_policy(&self) -> DirtyPolicy {
        DirtyPolicy::Always
    }
    fn state_type(&self) -> Box<dyn TypeTag> {
        Box::new(NamedTypeTag::new("()"))
    }
}
impl NodeSpec for AddF32 {
    fn properties(&self) -> Vec<(String, Property)> {
        vec![(
            "b".into(),
            Property::Scalar(ScalarConfig {
                label: "b".into(),
                value: self.b,
                min: -5.0,
                max: 5.0,
                step: Some(0.1),
                unit: None,
            }),
        )]
    }
    fn apply_property(&mut self, name: &str, value: PropertyValue) -> Result<(), PropertyError> {
        if name == "b" {
            let PropertyValue::Scalar(v) = value else {
                return Err(PropertyError::TypeMismatch {
                    expected: "scalar".into(),
                    got: "non-scalar".into(),
                });
            };
            self.b = v;
            Ok(())
        } else {
            Err(PropertyError::UnknownProperty(name.into()))
        }
    }
}

/// 1 in, 0 out: stores the received value in its state + a label property.
#[derive(Debug)]
struct DisplayF32 {
    meta: NodeMeta,
    inputs: Vec<InputSocket>,
    label: String,
}
impl DisplayF32 {
    fn new(label: impl Into<String>) -> Self {
        Self {
            meta: NodeMeta::new("demo.display.f32", "Display", 1),
            inputs: vec![InputSocket::new(f32_tag()).with_index(0)],
            label: label.into(),
        }
    }
}
impl Node for DisplayF32 {
    impl_as_any!();
    fn meta(&self) -> &NodeMeta {
        &self.meta
    }
    fn inputs(&self) -> &[InputSocket] {
        &self.inputs
    }
    fn outputs(&self) -> &[OutputSocket] {
        &[]
    }
    fn dirty_policy(&self) -> DirtyPolicy {
        DirtyPolicy::Always
    }
    fn state_type(&self) -> Box<dyn TypeTag> {
        Box::new(NamedTypeTag::new("f32"))
    }
}
impl NodeSpec for DisplayF32 {
    fn properties(&self) -> Vec<(String, Property)> {
        vec![(
            "label".into(),
            Property::String(StringConfig {
                label: "label".into(),
                value: self.label.clone(),
                kind: ze_node_schema::StringKind::Text,
            }),
        )]
    }
    fn apply_property(&mut self, name: &str, value: PropertyValue) -> Result<(), PropertyError> {
        if name == "label" {
            let PropertyValue::String(v) = value else {
                return Err(PropertyError::TypeMismatch {
                    expected: "string".into(),
                    got: "non-string".into(),
                });
            };
            self.label = v;
            Ok(())
        } else {
            Err(PropertyError::UnknownProperty(name.into()))
        }
    }
}

// ---------------------------------------------------------------------------
// Executor: reads properties via the registry (no per-node downcast in the
// dispatch logic; it keys off `meta().type_name` and uses `spec_of`).
// ---------------------------------------------------------------------------

struct PropertyExecutor {
    registry: NodeRegistry,
}

impl NodeExecutor for PropertyExecutor {
    fn execute(&self, node: &dyn Node, ctx: &mut dyn ExecContext) -> Result<(), EvalError> {
        let spec = self.registry.spec_of(node).ok_or_else(|| {
            EvalError::new(
                format!("no NodeSpec registered for `{}`", node.meta().type_name),
                ctx.current_node(),
                None,
            )
        })?;
        let here = ctx.current_node();
        match node.meta().type_name.as_str() {
            "demo.constant.f32" => {
                let v = scalar_prop(&spec.properties(), "value")?;
                ctx.set_output(OutputSocketId::new(here, 0), Box::new(v))?;
            }
            "demo.add.f32" => {
                let input = peek_f32(ctx, here, 0)?;
                let b = scalar_prop(&spec.properties(), "b")?;
                ctx.set_output(OutputSocketId::new(here, 0), Box::new(input + b))?;
            }
            "demo.display.f32" => {
                let input = peek_f32(ctx, here, 0)?;
                ctx.set_state(Box::new(input))?;
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

fn scalar_prop(props: &[(String, Property)], name: &str) -> Result<f32, EvalError> {
    for (n, p) in props {
        if n == name
            && let Property::Scalar(c) = p
        {
            return Ok(c.value);
        }
    }
    Err(EvalError::new(
        format!("missing scalar property `{name}`"),
        NodeId(0),
        None,
    ))
}

fn peek_f32(ctx: &mut dyn ExecContext, node: NodeId, idx: u32) -> Result<f32, EvalError> {
    let v = ctx.peek_input(InputSocketId::new(node, idx))?;
    let f = v
        .downcast_ref::<f32>()
        .ok_or_else(|| EvalError::new("input not f32", node, None))?;
    Ok(*f)
}

// ---------------------------------------------------------------------------
// Graph builder
// ---------------------------------------------------------------------------

fn build_graph() -> (SimpleGraph, NodeId, NodeId, NodeId) {
    let mut g = SimpleGraph::default();
    let c = g.add_node(Box::new(ConstantF32::new(1.0)));
    let a = g.add_node(Box::new(AddF32::new(2.0)));
    let d = g.add_node(Box::new(DisplayF32::new("result")));
    g.quick_connect(OutputSocketId::new(c, 0), InputSocketId::new(a, 0))
        .unwrap();
    g.quick_connect(OutputSocketId::new(a, 0), InputSocketId::new(d, 0))
        .unwrap();
    (g, c, a, d)
}

fn build_registry() -> NodeRegistry {
    let mut reg = NodeRegistry::new();
    reg.register::<ConstantF32>("Source", || Box::new(ConstantF32::new(0.0)));
    reg.register::<AddF32>("Math", || Box::new(AddF32::new(0.0)));
    reg.register::<DisplayF32>("Sink", || Box::new(DisplayF32::new("value")));
    reg
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn property_change_propagates_through_eval() {
    let (mut g, c, a, d) = build_graph();
    let reg = build_registry();
    let exec = PropertyExecutor {
        registry: reg.clone(),
    };
    let mut eval = LazyPullEval::new(Box::new(KahnSorter::new()), EvalErrorPolicy::FailFast);

    // First pass: Constant(1.0) -> Add(+2.0) = 3.0 -> Display.
    eval.evaluate(&g, &exec, Some(OutputSocketId::new(d, 0)))
        .unwrap();
    let out = eval.cached_output(OutputSocketId::new(a, 0)).unwrap();
    assert_eq!(*out.downcast_ref::<f32>().unwrap(), 3.0);

    // Edit the Constant's `value` property via NodeSpec — no downcast at the
    // call site (the registry's spec_of_mut bridges to the impl).
    {
        let node = g.get_node_mut(c).unwrap();
        let spec = reg.spec_of_mut(node).unwrap();
        spec.apply_property("value", PropertyValue::Scalar(5.0))
            .unwrap();
    }
    // Edit the Add node's `b` property too.
    {
        let node = g.get_node_mut(a).unwrap();
        let spec = reg.spec_of_mut(node).unwrap();
        spec.apply_property("b", PropertyValue::Scalar(10.0))
            .unwrap();
    }

    // Re-evaluate; downstream must reflect the new values.
    eval.evaluate(&g, &exec, Some(OutputSocketId::new(d, 0)))
        .unwrap();
    let out = eval.cached_output(OutputSocketId::new(a, 0)).unwrap();
    assert_eq!(*out.downcast_ref::<f32>().unwrap(), 15.0); // 5 + 10
}

#[test]
fn spawn_from_registry_produces_evaluable_node() {
    let reg = build_registry();
    let node = reg.spawn("demo.constant.f32").unwrap();
    assert_eq!(node.meta().type_name, "demo.constant.f32");
    // It is reachable as a NodeSpec via the registry.
    let spec = reg.spec_of(node.as_ref()).unwrap();
    assert_eq!(spec.properties().len(), 1);
}

#[test]
fn categories_match_registration() {
    let reg = build_registry();
    let cats = reg.categories();
    let names: Vec<&str> = cats.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["Source", "Math", "Sink"]);
}

#[test]
fn property_serde_round_trip_via_registry_spawn() {
    let reg = build_registry();
    let mut node = reg.spawn("demo.add.f32").unwrap();
    {
        let node_ref = node.as_mut();
        let spec = reg.spec_of_mut(node_ref).unwrap();
        spec.apply_property("b", PropertyValue::Scalar(-1.5))
            .unwrap();
    }
    // Serialize the property set and round-trip.
    let props = reg.spec_of(node.as_ref()).unwrap().properties();
    let json = serde_json::to_string(&props).unwrap();
    let back: Vec<(String, Property)> = serde_json::from_str(&json).unwrap();
    assert_eq!(props, back);
    assert_eq!(back[0].0, "b");
}
