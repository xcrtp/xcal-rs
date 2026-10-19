//! [`NodeSpec`] — the declarative property interface a node exposes.
//!
//! A node that wants to be driven by the property UI implements `NodeSpec` on
//! top of [`ze_node_core::Node`]. The trait is intentionally tiny: it describes
//! *which properties exist* and *how to apply a new value*. Everything else
//! (sockets, dirty policy, evaluation) comes from the `Node` impl, so property
//! nodes stay first-class graph citizens.
//!
//! Bridge to `dyn Node`: Rust has no stable trait upcasting, so reaching a
//! node's `NodeSpec` impl from a `&dyn Node` is mediated by
//! [`crate::NodeRegistry`], which stores generic accessor closures per node
//! type. That keeps `downcast` out of the UI layer entirely (the only
//! "escape hatch" lives inside the registry's generic accessor).

use ze_node_core::Node;

use crate::error::PropertyError;
use crate::property::{Property, PropertyValue};

/// A node that exposes a declarative set of properties.
///
/// `NodeSpec: Node` so a property-driven node is still a node — it can live in
/// any graph, be evaluated by any strategy, and be downcast by executors that
/// need concrete access. The trait itself adds only property read/write.
pub trait NodeSpec: Node {
    /// The node's properties as `(name, schema+value)` pairs, in declared
    /// order. Returned by value so callers can iterate without borrow
    /// entanglement with the node.
    fn properties(&self) -> Vec<(String, Property)>;

    /// Apply a new value to the named property. Type/range checked; returns
    /// [`PropertyError`] instead of panicking.
    fn apply_property(&mut self, name: &str, value: PropertyValue) -> Result<(), PropertyError>;
}

/// Blanket helper: read a property value off any `NodeSpec` by name.
pub fn property_value<S: NodeSpec + ?Sized>(node: &S, name: &str) -> Option<PropertyValue> {
    node.properties()
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, p)| p.value())
}

/// Blanket helper: find a property schema by name.
pub fn property_schema<S: NodeSpec + ?Sized>(node: &S, name: &str) -> Option<Property> {
    node.properties()
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, p)| p)
}
