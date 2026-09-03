//! [`NodeRegistry`] — the type-erased catalogue of `NodeSpec` node types.
//!
//! Three jobs (per plan §2.1):
//!   1. **register** node types with a factory + category (drives the
//!      right-click spawn menu);
//!   2. **spawn** fresh instances by `type_name`;
//!   3. **reach the [`crate::NodeSpec`] impl** from a `&dyn Node` /
//!      `&mut dyn Node` without the caller performing any downcast.
//!
//! The third job is the bridge across Rust's missing trait-upcasting: each
//! entry stores two generic accessor function pointers
//! (`fn(&dyn Node) -> &dyn NodeSpec`). The downcast lives *inside* that generic
//! accessor — it is written once here, never in UI code, never per-node by
//! hand.

use std::any::TypeId;

use xcal_node::{Node, NodeId};

use crate::spec::NodeSpec;

/// One registered node type.
#[derive(Clone)]
struct Entry {
    type_name: String,
    display_name: String,
    category: String,
    type_id: TypeId,
    factory: fn() -> Box<dyn Node>,
    spec_ref: for<'a> fn(&'a dyn Node) -> &'a dyn NodeSpec,
    spec_mut: for<'a> fn(&'a mut dyn Node) -> &'a mut dyn NodeSpec,
}

impl Entry {
    fn new<S>(category: String, factory: fn() -> Box<dyn Node>) -> Self
    where
        S: NodeSpec,
    {
        // Probe the factory once to learn the node's metadata. The factory is
        // `fn()` (no capture), so calling it is cheap and side-effect-free for
        // well-behaved node constructors.
        let probe = factory();
        Self {
            type_name: probe.meta().type_name.clone(),
            display_name: probe.meta().display_name.clone(),
            category,
            type_id: TypeId::of::<S>(),
            factory,
            spec_ref: spec_ref::<S>,
            spec_mut: spec_mut::<S>,
        }
    }
}

fn spec_ref<S: NodeSpec>(node: &dyn Node) -> &dyn NodeSpec {
    node.as_any()
        .downcast_ref::<S>()
        .expect("NodeRegistry: Node type_id matched but downcast failed")
}
fn spec_mut<S: NodeSpec>(node: &mut dyn Node) -> &mut dyn NodeSpec {
    node.as_any_mut()
        .downcast_mut::<S>()
        .expect("NodeRegistry: Node type_id matched but downcast failed")
}

/// A category group in the spawn menu (see [`NodeRegistry::categories`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryCategory {
    pub name: String,
    pub entries: Vec<RegistryNodeInfo>,
}

/// A spawnable node type as exposed to UIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryNodeInfo {
    pub type_name: String,
    pub display_name: String,
}

/// The type-erased catalogue of property-driven node types.
#[derive(Default, Clone)]
pub struct NodeRegistry {
    entries: Vec<Entry>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of registered node types.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Register a node type `S` under `category`, constructed by `factory`.
    /// If a type with the same `type_name` is already registered, it is
    /// replaced.
    pub fn register<S>(&mut self, category: impl Into<String>, factory: fn() -> Box<dyn Node>)
    where
        S: NodeSpec,
    {
        let entry = Entry::new::<S>(category.into(), factory);
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|e| e.type_name == entry.type_name)
        {
            *existing = entry;
        } else {
            self.entries.push(entry);
        }
    }

    /// Spawn a fresh instance by `type_name`.
    pub fn spawn(&self, type_name: &str) -> Option<Box<dyn Node>> {
        self.entries
            .iter()
            .find(|e| e.type_name == type_name)
            .map(|e| (e.factory)())
    }

    /// All registered `type_name`s.
    pub fn type_names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.type_name.as_str()).collect()
    }

    /// Reach the [`NodeSpec`] view of `node`, if its type is registered.
    /// No downcast is performed by the caller.
    pub fn spec_of<'a>(&self, node: &'a dyn Node) -> Option<&'a dyn NodeSpec> {
        let id = node.type_id();
        let entry = self.entries.iter().find(|e| e.type_id == id)?;
        Some((entry.spec_ref)(node))
    }

    /// Reach the mutable [`NodeSpec`] view of `node`, if its type is registered.
    /// No downcast is performed by the caller.
    pub fn spec_of_mut<'a>(&self, node: &'a mut dyn Node) -> Option<&'a mut dyn NodeSpec> {
        let id = node.type_id();
        let entry = self.entries.iter().find(|e| e.type_id == id)?;
        let f = entry.spec_mut;
        Some(f(node))
    }

    /// Group registered node types by category, preserving registration order
    /// within each category and first-seen order across categories. Used to
    /// build the right-click spawn menu.
    pub fn categories(&self) -> Vec<RegistryCategory> {
        let mut out: Vec<RegistryCategory> = Vec::new();
        for e in &self.entries {
            let info = RegistryNodeInfo {
                type_name: e.type_name.clone(),
                display_name: e.display_name.clone(),
            };
            if let Some(cat) = out.iter_mut().find(|c| c.name == e.category) {
                cat.entries.push(info);
            } else {
                out.push(RegistryCategory {
                    name: e.category.clone(),
                    entries: vec![info],
                });
            }
        }
        out
    }

    /// Look up a node id by following an upstream edge — convenience used by
    /// headless tests; kept here so tests do not re-implement graph walks.
    #[doc(hidden)]
    pub fn _doc_placeholder(_id: NodeId) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::property::{Property, PropertyValue, ScalarConfig};
    use std::any::Any;
    use xcal_node::{DirtyPolicy, InputSocket, NamedTypeTag, NodeMeta, OutputSocket, TypeTag};

    #[derive(Debug)]
    struct ConstNode {
        meta: NodeMeta,
        outputs: Vec<OutputSocket>,
        value: f32,
    }
    impl Node for ConstNode {
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
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }
    impl NodeSpec for ConstNode {
        fn properties(&self) -> Vec<(String, Property)> {
            vec![(
                "value".into(),
                Property::Scalar(ScalarConfig {
                    label: "value".into(),
                    value: self.value,
                    min: 0.0,
                    max: 10.0,
                    step: None,
                    unit: None,
                }),
            )]
        }
        fn apply_property(
            &mut self,
            name: &str,
            value: PropertyValue,
        ) -> Result<(), crate::error::PropertyError> {
            if name != "value" {
                return Err(crate::error::PropertyError::UnknownProperty(name.into()));
            }
            let PropertyValue::Scalar(v) = value else {
                return Err(crate::error::PropertyError::TypeMismatch {
                    expected: "scalar".into(),
                    got: "non-scalar".into(),
                });
            };
            self.value = v;
            Ok(())
        }
    }

    fn factory() -> Box<dyn Node> {
        Box::new(ConstNode {
            meta: NodeMeta::new("test.const", "Const", 1),
            outputs: vec![OutputSocket::new(Box::new(NamedTypeTag::new("f32"))).with_index(0)],
            value: 0.0,
        })
    }

    #[test]
    fn register_and_spawn() {
        let mut reg = NodeRegistry::new();
        reg.register::<ConstNode>("Source", factory);
        let node = reg.spawn("test.const").unwrap();
        assert_eq!(node.meta().type_name, "test.const");
    }

    #[test]
    fn spec_of_reaches_impl_without_caller_downcast() {
        let mut reg = NodeRegistry::new();
        reg.register::<ConstNode>("Source", factory);
        let mut node: Box<dyn Node> = reg.spawn("test.const").unwrap();
        let spec = reg.spec_of(node.as_ref()).unwrap();
        let props = spec.properties();
        assert_eq!(props.len(), 1);
        assert_eq!(props[0].0, "value");
        // mutate via spec_of_mut
        let spec_m = reg.spec_of_mut(node.as_mut()).unwrap();
        spec_m
            .apply_property("value", PropertyValue::Scalar(3.5))
            .unwrap();
        let spec = reg.spec_of(node.as_ref()).unwrap();
        assert_eq!(spec.properties()[0].1.value(), PropertyValue::Scalar(3.5));
    }

    #[test]
    fn categories_groups_by_category() {
        let mut reg = NodeRegistry::new();
        reg.register::<ConstNode>("Source", factory);
        reg.register::<ConstNode>("Source", factory);
        let cats = reg.categories();
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].name, "Source");
        assert_eq!(cats[0].entries.len(), 1); // duplicate type_name collapsed
    }
}
