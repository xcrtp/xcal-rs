//! Phase B integration: a runtime-assembled [`DynamicNode`] and a static
//! `NodeSpec` node share one graph and one [`LazyPullEval`].
//!
//! Demonstrates:
//!   * fields added at runtime (no Rust recompile) drive evaluation;
//!   * type-mismatched writes return an error, never panic;
//!   * a dynamic node and a static node are processed by the same evaluator
//!     and the same property UI path (`NodeRegistry::spec_of`).

use std::any::Any;

use ze_graph_algo::KahnSorter;
use ze_graph_eval::LazyPullEval;
use ze_node_core::{
    DirtyPolicy, EvalError, EvalErrorPolicy, EvalStrategy, ExecContext, Graph, InputSocket,
    InputSocketId, NamedTypeTag, Node, NodeExecutor, NodeId, NodeMeta, OutputSocket,
    OutputSocketId, SimpleGraph, TypeTag,
};
use ze_node_schema::{
    DynamicNode, NodeRegistry, NodeSpec, Property, PropertyError, PropertyValue, ScalarConfig,
};

fn f32_tag() -> Box<dyn TypeTag> {
    Box::new(NamedTypeTag::new("f32"))
}

// ---------------------------------------------------------------------------
// Static node: a plain Constant (NodeSpec) reused from the headless test.
// ---------------------------------------------------------------------------

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

#[derive(Debug)]
struct StaticConst {
    meta: NodeMeta,
    outputs: Vec<OutputSocket>,
    value: f32,
}
impl StaticConst {
    fn new(v: f32) -> Self {
        Self {
            meta: NodeMeta::new("static.const", "StaticConst", 1),
            outputs: vec![OutputSocket::new(f32_tag()).with_index(0)],
            value: v,
        }
    }
}
impl Node for StaticConst {
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
impl NodeSpec for StaticConst {
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

// ---------------------------------------------------------------------------
// Dynamic node: a "Scale" node whose `factor` field is added at runtime.
// Output = input * factor.
// ---------------------------------------------------------------------------

fn build_scale_node(factor: f32) -> DynamicNode {
    DynamicNode::builder("dynamic.scale", "Scale")
        .input(f32_tag())
        .output(f32_tag())
        .field(
            "factor",
            Property::Scalar(ScalarConfig {
                label: "factor".into(),
                value: factor,
                min: -10.0,
                max: 10.0,
                step: Some(0.1),
                unit: None,
            }),
        )
        .build()
}

// ---------------------------------------------------------------------------
// Executor: dispatches by `meta().type_name`. For the dynamic node it reads
// the `factor` field through `NodeSpec::properties()` (no downcast); for the
// static node likewise.
// ---------------------------------------------------------------------------

struct MixedExecutor {
    registry: NodeRegistry,
}

impl NodeExecutor for MixedExecutor {
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
            "static.const" => {
                let v = scalar_field(&spec.properties(), "value")?;
                ctx.set_output(OutputSocketId::new(here, 0), Box::new(v))?;
            }
            "dynamic.scale" => {
                let input = peek_f32(ctx, here, 0)?;
                let factor = scalar_field(&spec.properties(), "factor")?;
                ctx.set_output(OutputSocketId::new(here, 0), Box::new(input * factor))?;
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
// Tests
// ---------------------------------------------------------------------------

fn registry() -> NodeRegistry {
    let mut reg = NodeRegistry::new();
    reg.register::<StaticConst>("Source", || Box::new(StaticConst::new(0.0)));
    reg.register::<DynamicNode>("Math", || {
        // Default dynamic node spawned by the menu; users customize fields after.
        Box::new(
            DynamicNode::builder("dynamic.scale", "Scale")
                .input(f32_tag())
                .output(f32_tag())
                .field(
                    "factor",
                    Property::Scalar(ScalarConfig {
                        label: "factor".into(),
                        value: 1.0,
                        min: -10.0,
                        max: 10.0,
                        step: Some(0.1),
                        unit: None,
                    }),
                )
                .build(),
        )
    });
    reg
}

#[test]
fn dynamic_and_static_nodes_share_one_evaluator() {
    let mut g = SimpleGraph::default();
    let c = g.add_node(Box::new(StaticConst::new(4.0)));
    let s = g.add_node(Box::new(build_scale_node(2.0)));
    g.quick_connect(OutputSocketId::new(c, 0), InputSocketId::new(s, 0))
        .unwrap();

    let reg = registry();
    let exec = MixedExecutor {
        registry: reg.clone(),
    };
    let mut eval = LazyPullEval::new(Box::new(KahnSorter::new()), EvalErrorPolicy::FailFast);

    eval.evaluate(&g, &exec, Some(OutputSocketId::new(s, 0)))
        .unwrap();
    let out = eval.cached_output(OutputSocketId::new(s, 0)).unwrap();
    assert_eq!(*out.downcast_ref::<f32>().unwrap(), 8.0); // 4 * 2

    // Runtime: change the dynamic node's `factor` field (no recompile).
    {
        let node = g.get_node_mut(s).unwrap();
        let spec = reg.spec_of_mut(node).unwrap();
        spec.apply_property("factor", PropertyValue::Scalar(0.5))
            .unwrap();
    }
    eval.evaluate(&g, &exec, Some(OutputSocketId::new(s, 0)))
        .unwrap();
    let out = eval.cached_output(OutputSocketId::new(s, 0)).unwrap();
    assert_eq!(*out.downcast_ref::<f32>().unwrap(), 2.0); // 4 * 0.5

    // Runtime: add a brand-new field to the dynamic node (not in any struct).
    // Reach the underlying DynamicObject via downcast to DynamicNode — this is
    // the one place a downcast is allowed (host-side customization of a dynamic
    // node), and it is NOT in a UI body_ui.
    {
        let node = g.get_node_mut(s).unwrap();
        let dn = node
            .as_any_mut()
            .downcast_mut::<DynamicNode>()
            .expect("scale node is a DynamicNode");
        dn.object_mut()
            .add(
                "note",
                Property::String(ze_node_schema::StringConfig {
                    label: "note".into(),
                    value: "user-added at runtime".into(),
                    kind: ze_node_schema::StringKind::Text,
                }),
            )
            .unwrap();
    }
    // The new field is now visible through NodeSpec (read back separately so
    // the mutable borrow above is released first).
    {
        let node = g.get_node_mut(s).unwrap();
        let spec = reg.spec_of_mut(node).unwrap();
        assert_eq!(spec.properties().len(), 2);
        assert!(spec.properties().iter().any(|(n, _)| n == "note"));
    }
}

#[test]
fn type_mismatch_returns_error_without_panic() {
    let mut node = build_scale_node(1.0);
    // Writing a Bool into a Scalar field must error, not panic.
    let err = node
        .apply_property("factor", PropertyValue::Bool(false))
        .unwrap_err();
    assert!(matches!(err, PropertyError::TypeMismatch { .. }));
    // Writing to a non-existent field must error.
    let err = node
        .apply_property("nope", PropertyValue::Scalar(1.0))
        .unwrap_err();
    assert!(matches!(err, PropertyError::UnknownProperty(_)));
}

#[test]
fn dynamic_node_reachable_via_registry_spec_of() {
    let reg = registry();
    let node = build_scale_node(3.0);
    let spec = reg.spec_of(&node).unwrap();
    let factor = scalar_field(&spec.properties(), "factor").unwrap();
    assert_eq!(factor, 3.0);
}

#[test]
fn dynamic_object_persists_through_serde() {
    let mut node = build_scale_node(7.0);
    node.object_mut()
        .add(
            "tag",
            Property::String(ze_node_schema::StringConfig {
                label: "tag".into(),
                value: "hello".into(),
                kind: ze_node_schema::StringKind::Text,
            }),
        )
        .unwrap();
    let props = node.properties();
    let json = serde_json::to_string(&props).unwrap();
    let back: Vec<(String, Property)> = serde_json::from_str(&json).unwrap();
    assert_eq!(props, back);
}
