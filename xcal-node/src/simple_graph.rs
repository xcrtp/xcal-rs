//! [`SimpleGraph`] — the reference `Graph` implementation.
//!
//! Backed by a `HashMap<NodeId, Box<dyn Node>>` plus adjacency vectors. It is
//! not the fastest possible storage but it is correct, self-contained and a
//! conformance target for the `Graph` trait. Edge data is stored per edge with
//! a version counter for dirty detection.

use std::collections::HashMap;
use std::fmt;

use crate::edge::{Edge, EdgeData, EdgeId};
use crate::graph::{ConnectError, Graph};
use crate::id::{InputSocketId, NodeId, OutputSocketId};
use crate::node::Node;
use crate::type_tag::{DefaultTypeCast, TypeCast};

/// Reference in-memory graph storage.
pub struct SimpleGraph {
    nodes: HashMap<NodeId, Box<dyn Node>>,
    next_id: u64,
    /// output -> downstream inputs (in insertion order).
    downstream: HashMap<OutputSocketId, Vec<InputSocketId>>,
    /// input -> upstream outputs (usually 0 or 1, but Vec for multi).
    upstream: HashMap<InputSocketId, Vec<OutputSocketId>>,
    /// edge data keyed by (source, target).
    edges: HashMap<EdgeId, Edge>,
    caster: Box<dyn TypeCast>,
}

impl Default for SimpleGraph {
    fn default() -> Self {
        Self::with_caster(Box::new(DefaultTypeCast))
    }
}

impl SimpleGraph {
    /// Construct with a custom type caster.
    pub fn with_caster(caster: Box<dyn TypeCast>) -> Self {
        Self {
            nodes: HashMap::new(),
            next_id: 0,
            downstream: HashMap::new(),
            upstream: HashMap::new(),
            edges: HashMap::new(),
            caster,
        }
    }

    fn alloc_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    fn output_socket_desc(
        &self,
        socket: OutputSocketId,
    ) -> Result<&crate::socket::OutputSocket, ConnectError> {
        let node = self
            .nodes
            .get(&socket.node())
            .ok_or(ConnectError::SourceNotFound(socket))?;
        node.outputs()
            .get(socket.index() as usize)
            .ok_or(ConnectError::SourceNotFound(socket))
    }

    fn input_socket_desc(
        &self,
        socket: InputSocketId,
    ) -> Result<&crate::socket::InputSocket, ConnectError> {
        let node = self
            .nodes
            .get(&socket.node())
            .ok_or(ConnectError::TargetNotFound(socket))?;
        node.inputs()
            .get(socket.index() as usize)
            .ok_or(ConnectError::TargetNotFound(socket))
    }

    /// Manually bump an edge's version after mutating its data through
    /// [`Graph::edge_data_mut`]. (The trait returns a raw `&mut dyn EdgeData`,
    /// so the version bump cannot be automated on drop — callers must call this.)
    pub fn bump_edge_version(
        &mut self,
        source: OutputSocketId,
        target: InputSocketId,
    ) -> Option<u64> {
        let id = EdgeId { source, target };
        let edge = self.edges.get_mut(&id)?;
        edge.bump();
        Some(edge.version)
    }

    /// Number of edges.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

impl Graph for SimpleGraph {
    fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn get_node(&self, node: NodeId) -> Option<&dyn Node> {
        self.nodes.get(&node).map(|n| n.as_ref() as &dyn Node)
    }

    fn upstream(&self, input: InputSocketId) -> Vec<OutputSocketId> {
        self.upstream
            .get(&input)
            .cloned()
            .unwrap_or_default()
    }

    fn downstream(&self, output: OutputSocketId) -> Vec<InputSocketId> {
        self.downstream
            .get(&output)
            .cloned()
            .unwrap_or_default()
    }

    fn all_nodes(&self) -> Box<dyn Iterator<Item = NodeId> + '_> {
        Box::new(self.nodes.keys().copied())
    }

    fn add_node(&mut self, mut node: Box<dyn Node>) -> NodeId {
        let id = self.alloc_id();
        node.on_add();
        self.nodes.insert(id, node);
        id
    }

    fn remove_node(&mut self, node: NodeId) -> Option<Box<dyn Node>> {
        let mut removed = self.nodes.remove(&node)?;
        removed.on_remove();
        // Drop edges touching this node.
        let out_sockets: Vec<OutputSocketId> = self
            .downstream
            .keys()
            .filter(|s| s.node() == node)
            .copied()
            .collect();
        for s in out_sockets {
            for t in self.downstream.remove(&s).unwrap_or_default() {
                self.edges.remove(&EdgeId { source: s, target: t });
                if let Some(v) = self.upstream.get_mut(&t) {
                    v.retain(|o| *o != s);
                }
            }
        }
        let in_sockets: Vec<InputSocketId> = self
            .upstream
            .keys()
            .filter(|s| s.node() == node)
            .copied()
            .collect();
        for t in in_sockets {
            for s in self.upstream.remove(&t).unwrap_or_default() {
                self.edges.remove(&EdgeId { source: s, target: t });
                if let Some(v) = self.downstream.get_mut(&s) {
                    v.retain(|i| *i != t);
                }
            }
        }
        Some(removed)
    }

    fn connect_with_data(
        &mut self,
        source: OutputSocketId,
        target: InputSocketId,
        data: Box<dyn EdgeData>,
    ) -> Result<(), ConnectError> {
        let out_desc = self.output_socket_desc(source)?;
        let in_desc = self.input_socket_desc(target)?;

        // Type compatibility.
        if !self
            .caster
            .is_compatible(out_desc.type_tag.as_ref(), in_desc.type_tag.as_ref())
        {
            return Err(ConnectError::TypeIncompatible {
                from: out_desc.type_tag.type_name().to_string(),
                to: in_desc.type_tag.type_name().to_string(),
            });
        }

        // Single-connection input.
        if !in_desc.allow_multi {
            if self
                .upstream
                .get(&target)
                .map(|v| !v.is_empty())
                .unwrap_or(false)
            {
                return Err(ConnectError::InputAlreadyConnected(target));
            }
        }

        // Output connection limit.
        if let Some(max) = out_desc.max_connections {
            let current = self
                .downstream
                .get(&source)
                .map(|v| v.len())
                .unwrap_or(0);
            if current >= max as usize {
                return Err(ConnectError::OutputConnectionLimitReached(source));
            }
        }

        let id = EdgeId { source, target };
        if self.edges.contains_key(&id) {
            return Err(ConnectError::DuplicateEdge(id));
        }

        self.edges.insert(id, Edge::new(source, target, data));
        self.downstream.entry(source).or_default().push(target);
        self.upstream.entry(target).or_default().push(source);
        Ok(())
    }

    fn disconnect(
        &mut self,
        source: OutputSocketId,
        target: InputSocketId,
    ) -> Option<Box<dyn EdgeData>> {
        let id = EdgeId { source, target };
        let edge = self.edges.remove(&id)?;
        if let Some(v) = self.downstream.get_mut(&source) {
            v.retain(|i| *i != target);
        }
        if let Some(v) = self.upstream.get_mut(&target) {
            v.retain(|o| *o != source);
        }
        Some(edge.data)
    }

    fn edge_data_mut(
        &mut self,
        source: OutputSocketId,
        target: InputSocketId,
    ) -> Option<&mut dyn EdgeData> {
        let id = EdgeId { source, target };
        let edge = self.edges.get_mut(&id)?;
        Some(edge.data.as_mut())
    }

    fn edge_data(
        &self,
        source: OutputSocketId,
        target: InputSocketId,
    ) -> Option<&dyn EdgeData> {
        let id = EdgeId { source, target };
        let edge = self.edges.get(&id)?;
        Some(edge.data.as_ref())
    }

    fn edge_version(
        &self,
        source: OutputSocketId,
        target: InputSocketId,
    ) -> Option<u64> {
        self.edges.get(&EdgeId { source, target }).map(|e| e.version)
    }

    fn type_cast(&self) -> &dyn TypeCast {
        self.caster.as_ref()
    }
}

impl fmt::Debug for SimpleGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SimpleGraph")
            .field("node_count", &self.nodes.len())
            .field("edge_count", &self.edges.len())
            .field("next_id", &self.next_id)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;

    use super::*;
    use crate::edge::UnitEdge;
    use crate::node::{DirtyPolicy, NodeMeta};
    use crate::socket::{InputSocket, OutputSocket};
    use crate::type_tag::NamedTypeTag;

    /// Helper: the two required `as_any` bodies every `Node` impl must supply.
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

    /// Minimal node: one f32 input, one f32 output.
    #[derive(Debug)]
    struct PassthroughNode {
        meta: NodeMeta,
        inputs: Vec<InputSocket>,
        outputs: Vec<OutputSocket>,
    }

    impl PassthroughNode {
        fn new() -> Self {
            let tag: Box<dyn crate::type_tag::TypeTag> =
                Box::new(NamedTypeTag::new("f32"));
            Self {
                meta: NodeMeta::new("test.passthrough", "Passthrough", 1),
                inputs: vec![InputSocket::new(tag.clone()).with_index(0)],
                outputs: vec![OutputSocket::new(tag).with_index(0)],
            }
        }
    }

    impl Node for PassthroughNode {
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
        fn state_type(&self) -> Box<dyn crate::type_tag::TypeTag> {
            Box::new(NamedTypeTag::new("()"))
        }
    }

    fn make_graph() -> (SimpleGraph, NodeId, NodeId, NodeId) {
        let mut g = SimpleGraph::default();
        let a = g.add_node(Box::new(PassthroughNode::new()));
        let b = g.add_node(Box::new(PassthroughNode::new()));
        let c = g.add_node(Box::new(PassthroughNode::new()));
        (g, a, b, c)
    }

    #[test]
    fn add_and_remove_node() {
        let (mut g, a, _b, _c) = make_graph();
        assert_eq!(g.node_count(), 3);
        assert!(g.get_node(a).is_some());
        assert!(g.remove_node(a).is_some());
        assert_eq!(g.node_count(), 2);
        assert!(g.get_node(a).is_none());
    }

    #[test]
    fn connect_and_query_adjacency() {
        let (mut g, a, b, c) = make_graph();
        let a_out = OutputSocketId::new(a, 0);
        let b_in = InputSocketId::new(b, 0);
        let b_out = OutputSocketId::new(b, 0);
        let c_in = InputSocketId::new(c, 0);

        g.quick_connect(a_out, b_in).unwrap();
        g.quick_connect(b_out, c_in).unwrap();

        assert_eq!(g.upstream(b_in), vec![a_out]);
        assert_eq!(g.downstream(a_out), vec![b_in]);
        assert_eq!(g.upstream(c_in), vec![b_out]);
        assert_eq!(g.edge_count(), 2);
    }

    #[test]
    fn single_input_rejects_second_connection() {
        let (mut g, a, b, c) = make_graph();
        let a_out = OutputSocketId::new(a, 0);
        let b_out = OutputSocketId::new(b, 0);
        let c_in = InputSocketId::new(c, 0);
        g.quick_connect(a_out, c_in).unwrap();
        let err = g.quick_connect(b_out, c_in).unwrap_err();
        assert!(matches!(err, ConnectError::InputAlreadyConnected(_)));
    }

    #[test]
    fn type_incompatible_rejected() {
        let mut g = SimpleGraph::default();
        // node with f32 in, node with i32 out -> incompatible with default caster.
        let n_in = Box::new(PassthroughNode::new()); // f32
        let a = g.add_node(n_in);
        // build an i32-output node
        #[derive(Debug)]
        struct IntOut {
            meta: NodeMeta,
            inputs: Vec<InputSocket>,
            outputs: Vec<OutputSocket>,
        }
        impl Node for IntOut {
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
            fn state_type(&self) -> Box<dyn crate::type_tag::TypeTag> {
                Box::new(NamedTypeTag::new("()"))
            }
        }
        let int_out = IntOut {
            meta: NodeMeta::new("test.intout", "IntOut", 1),
            inputs: vec![],
            outputs: vec![OutputSocket::new(Box::new(NamedTypeTag::new("i32")))],
        };
        let b = g.add_node(Box::new(int_out));
        let err = g
            .quick_connect(OutputSocketId::new(b, 0), InputSocketId::new(a, 0))
            .unwrap_err();
        assert!(matches!(err, ConnectError::TypeIncompatible { .. }));
    }

    #[test]
    fn output_connection_limit_enforced() {
        let mut g = SimpleGraph::default();
        // source with max 1 connection
        #[derive(Debug)]
        struct LimitedOut {
            meta: NodeMeta,
            inputs: Vec<InputSocket>,
            outputs: Vec<OutputSocket>,
        }
        impl Node for LimitedOut {
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
            fn state_type(&self) -> Box<dyn crate::type_tag::TypeTag> {
                Box::new(NamedTypeTag::new("()"))
            }
        }
        let tag: Box<dyn crate::type_tag::TypeTag> = Box::new(NamedTypeTag::new("f32"));
        let src = LimitedOut {
            meta: NodeMeta::new("test.limited", "Limited", 1),
            inputs: vec![],
            outputs: vec![OutputSocket::new(tag.clone()).with_max_connections(1)],
        };
        let s = g.add_node(Box::new(src));
        let t1 = g.add_node(Box::new(PassthroughNode::new()));
        let t2 = g.add_node(Box::new(PassthroughNode::new()));
        let s_out = OutputSocketId::new(s, 0);
        g.quick_connect(s_out, InputSocketId::new(t1, 0)).unwrap();
        let err = g
            .quick_connect(s_out, InputSocketId::new(t2, 0))
            .unwrap_err();
        assert!(matches!(err, ConnectError::OutputConnectionLimitReached(_)));
    }

    #[test]
    fn disconnect_returns_data_and_updates_adjacency() {
        let (mut g, a, b, _c) = make_graph();
        let a_out = OutputSocketId::new(a, 0);
        let b_in = InputSocketId::new(b, 0);
        g.connect_with_data(a_out, b_in, Box::new(UnitEdge)).unwrap();
        let data = g.disconnect(a_out, b_in);
        assert!(data.is_some());
        assert!(g.upstream(b_in).is_empty());
        assert!(g.downstream(a_out).is_empty());
    }

    #[test]
    fn edge_data_mut_and_bump() {
        let (mut g, a, b, _c) = make_graph();
        let a_out = OutputSocketId::new(a, 0);
        let b_in = InputSocketId::new(b, 0);
        g.quick_connect(a_out, b_in).unwrap();
        let v0 = g.edge_version(a_out, b_in).unwrap();
        {
            let d = g.edge_data_mut(a_out, b_in).unwrap();
            let _ = d; // would mutate here
        }
        g.bump_edge_version(a_out, b_in);
        let v1 = g.edge_version(a_out, b_in).unwrap();
        assert_ne!(v0, v1);
    }

    #[test]
    fn remove_node_drops_incident_edges() {
        let (mut g, a, b, _c) = make_graph();
        let a_out = OutputSocketId::new(a, 0);
        let b_in = InputSocketId::new(b, 0);
        g.quick_connect(a_out, b_in).unwrap();
        assert_eq!(g.edge_count(), 1);
        g.remove_node(a);
        assert_eq!(g.edge_count(), 0);
        assert!(g.upstream(b_in).is_empty());
    }
}
