//! Graph-algorithm trait interfaces.
//!
//! Per the layering plan, the *interfaces* for graph algorithms live here in
//! `xcal-node` so that the evaluation strategy (and editor) depend only on
//! traits. Concrete implementations (Kahn sort, BFS dirty propagation, Tarjan
//! SCC) live in the separate `xcal-graph-algo` crate and can be swapped without
//! touching the core.

use std::collections::HashSet;

use crate::graph::Graph;
use crate::id::{NodeId, OutputSocketId};

/// Error raised when a topological sort encounters a cycle.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CycleError {
    #[error("cycle detected involving nodes {nodes:?}")]
    Cycle { nodes: Vec<NodeId> },
    #[error("cycle detected (no node detail available)")]
    Opaque,
}

/// Topologically sort the nodes of a graph.
pub trait TopoSort: Send + Sync + 'static {
    /// Returns node ids in dependency order (upstream first).
    fn sort(&self, graph: &dyn Graph) -> Result<Vec<NodeId>, CycleError>;
}

/// Tracks which nodes need recomputation (dirty propagation).
pub trait DirtyTracker: Send + Sync + 'static {
    /// Mark a whole node dirty.
    fn mark_dirty(&mut self, node: NodeId);
    /// Mark an output socket dirty and propagate to its downstream subgraph.
    fn mark_socket_dirty(&mut self, graph: &dyn Graph, socket: OutputSocketId);
    /// Is this node currently dirty?
    fn is_dirty(&self, node: NodeId) -> bool;
    /// Clear the dirty flag for a node.
    fn clear_dirty(&mut self, node: NodeId);
    /// Collect every dirty node in the graph.
    fn collect_dirty(&self) -> HashSet<NodeId>;
}

/// Optional cycle detection (used by editors for live warnings).
pub trait CycleDetector: Send + Sync + 'static {
    /// Returns one cycle's worth of nodes if the graph contains a cycle.
    fn detect(&self, graph: &dyn Graph) -> Option<Vec<NodeId>>;
}
