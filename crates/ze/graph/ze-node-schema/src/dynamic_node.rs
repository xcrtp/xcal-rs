//! [`DynamicNode`] — a node whose properties are a runtime [`DynamicObject`].
//!
//! This is the "user-defined node" half of the property model: instead of a
//! fixed `struct` + `#[derive(NodeSpec)]`, the user (or a script / file import)
//! assembles a [`DynamicObject`] field-by-field at runtime and wraps it in a
//! `DynamicNode`. Because `DynamicNode` implements both [`ze_node_core::Node`] and
//! [`crate::NodeSpec`], it is a first-class graph citizen: it can be connected,
//! evaluated by [`crate::LazyPullEval`](ze_graph_eval::LazyPullEval) (via the
//! host's `NodeExecutor`), and edited from the same property UI as declarative
//! nodes — all without recompiling.
//!
//! Evaluation semantics are intentionally host-defined: a `DynamicNode` only
//! carries data (properties + socket layout). The host supplies a
//! `NodeExecutor` that interprets the fields (e.g. "emit the `value` field on
//! output 0"). See `tests/dynamic_coexist.rs` for a worked example where a
//! dynamic node and a static `NodeSpec` node share one graph and one
//! evaluator.

use std::any::Any;

use ze_node_core::type_tag::NamedTypeTag;
use ze_node_core::{DirtyPolicy, InputSocket, Node, NodeMeta, OutputSocket, TypeTag};

use crate::dynamic::DynamicObject;
use crate::error::PropertyError;
use crate::property::{Property, PropertyValue};
use crate::spec::NodeSpec;

/// A node backed by a [`DynamicObject`]: its property set is editable at
/// runtime.
pub struct DynamicNode {
    meta: NodeMeta,
    inputs: Vec<InputSocket>,
    outputs: Vec<OutputSocket>,
    state_tag: Box<dyn TypeTag>,
    object: DynamicObject,
    policy: DirtyPolicy,
}

impl std::fmt::Debug for DynamicNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynamicNode")
            .field("meta", &self.meta)
            .field("inputs", &self.inputs.len())
            .field("outputs", &self.outputs.len())
            .field("fields", &self.object.len())
            .finish_non_exhaustive()
    }
}

impl DynamicNode {
    /// Begin building a dynamic node with the given type metadata.
    pub fn builder(
        type_name: impl Into<String>,
        display_name: impl Into<String>,
    ) -> DynamicNodeBuilder {
        DynamicNodeBuilder {
            meta: NodeMeta::new(type_name, display_name, 1),
            inputs: Vec::new(),
            outputs: Vec::new(),
            state_tag: Box::new(NamedTypeTag::new("()")),
            object: DynamicObject::new(),
            policy: DirtyPolicy::Always,
        }
    }

    /// Borrow the underlying dynamic object.
    pub fn object(&self) -> &DynamicObject {
        &self.object
    }

    /// Mutably borrow the underlying dynamic object (add/remove/rename fields).
    pub fn object_mut(&mut self) -> &mut DynamicObject {
        &mut self.object
    }
}

impl Node for DynamicNode {
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
        self.policy.clone()
    }
    fn state_type(&self) -> Box<dyn TypeTag> {
        self.state_tag.clone_box()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl NodeSpec for DynamicNode {
    fn properties(&self) -> Vec<(String, Property)> {
        self.object.properties()
    }
    fn apply_property(&mut self, name: &str, value: PropertyValue) -> Result<(), PropertyError> {
        self.object.set_value(name, value)
    }
}

/// Builder for [`DynamicNode`].
pub struct DynamicNodeBuilder {
    meta: NodeMeta,
    inputs: Vec<InputSocket>,
    outputs: Vec<OutputSocket>,
    state_tag: Box<dyn TypeTag>,
    object: DynamicObject,
    policy: DirtyPolicy,
}

impl DynamicNodeBuilder {
    pub fn version(mut self, v: u32) -> Self {
        self.meta.version = v;
        self
    }

    /// Add an input socket (indexed in call order).
    pub fn input(mut self, type_tag: Box<dyn TypeTag>) -> Self {
        let idx = self.inputs.len() as u32;
        self.inputs.push(InputSocket::new(type_tag).with_index(idx));
        self
    }

    /// Add an output socket (indexed in call order).
    pub fn output(mut self, type_tag: Box<dyn TypeTag>) -> Self {
        let idx = self.outputs.len() as u32;
        self.outputs
            .push(OutputSocket::new(type_tag).with_index(idx));
        self
    }

    /// Set the state type tag (defaults to `()`).
    pub fn state_type(mut self, tag: Box<dyn TypeTag>) -> Self {
        self.state_tag = tag;
        self
    }

    /// Set the dirty policy (defaults to `Always`).
    pub fn dirty_policy(mut self, policy: DirtyPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Add a runtime property field.
    pub fn field(mut self, name: impl Into<String>, property: Property) -> Self {
        // Builder API is best-effort: duplicate names are ignored to keep
        // construction ergonomic; use `object_mut().add(...)` for checked adds.
        let name = name.into();
        if self.object.get(&name).is_none()
            && let Err(err) = self.object.add(name, property)
        {
            // `get` 已排除重名，走到这里说明契约被破坏：保留 best-effort 语义，
            // 但在 debug 下立即暴露，而不是静默吞掉。
            debug_assert!(
                false,
                "dynamic field add failed after duplicate check: {err}"
            );
        }
        self
    }

    /// Add a required runtime property field.
    pub fn required_field(mut self, name: impl Into<String>, property: Property) -> Self {
        let name = name.into();
        if self.object.get(&name).is_none()
            && let Err(err) = self.object.add_required(name, property)
        {
            debug_assert!(
                false,
                "dynamic required field add failed after duplicate check: {err}"
            );
        }
        self
    }

    /// Take ownership of a pre-built [`DynamicObject`].
    pub fn object(mut self, object: DynamicObject) -> Self {
        self.object = object;
        self
    }

    pub fn build(self) -> DynamicNode {
        DynamicNode {
            meta: self.meta,
            inputs: self.inputs,
            outputs: self.outputs,
            state_tag: self.state_tag,
            object: self.object,
            policy: self.policy,
        }
    }
}
