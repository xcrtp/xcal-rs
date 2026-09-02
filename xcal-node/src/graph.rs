//! The abstract [`Graph`] trait — node/edge topology queries and mutations.
//!
//! `Graph` is object-safe (storable as `&dyn Graph` / `Box<dyn Graph>`) so the
//! algorithm and editor layers can program against *any* storage. Socket
//! identity is the pair `(NodeId, local-index)`: the local index is the
//! position of the socket descriptor in [`crate::Node::inputs`] /
//! [`crate::Node::outputs`]. The `node`/`index` fields on the socket structs
//! are advisory convenience; the graph itself never trusts them for identity.
//!
//! `upstream` / `downstream` uniformly return `Vec`s (never single `Option`s)
//! to keep the trait uniform across multi- and single-connection sockets.

use std::fmt;

use crate::edge::EdgeData;
use crate::id::{InputSocketId, NodeId, OutputSocketId};
use crate::node::Node;

/// Why a `connect` attempt was rejected.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("source output socket {0:?} not found")]
    SourceNotFound(OutputSocketId),
    #[error("target input socket {0:?} not found")]
    TargetNotFound(InputSocketId),
    #[error("type incompatibility: {from} cannot feed {to}")]
    TypeIncompatible { from: String, to: String },
    #[error("input socket {0:?} already connected and does not allow multi-connection")]
    InputAlreadyConnected(InputSocketId),
    #[error("output socket {0:?} reached its connection limit")]
    OutputConnectionLimitReached(OutputSocketId),
    #[error("connection would create a cycle")]
    CycleDetected,
    #[error("duplicate edge {0:?}")]
    DuplicateEdge(crate::edge::EdgeId),
}

/// Abstract node graph. Query + mutation in one object-safe trait.
pub trait Graph: Send + Sync + fmt::Debug {
    // --- queries ---
    fn node_count(&self) -> usize;
    fn get_node(&self, node: NodeId) -> Option<&dyn Node>;
    /// Output sockets feeding `input`. Empty if none.
    fn upstream(&self, input: InputSocketId) -> Vec<OutputSocketId>;
    /// Input sockets fed by `output`. Empty if none.
    fn downstream(&self, output: OutputSocketId) -> Vec<InputSocketId>;
    /// All node handles (order unspecified).
    fn all_nodes(&self) -> Box<dyn Iterator<Item = NodeId> + '_>;

    // --- mutations ---
    /// Insert a node, returning its assigned handle.
    fn add_node(&mut self, node: Box<dyn Node>) -> NodeId;
    /// Remove a node (and its edges), returning the node.
    fn remove_node(&mut self, node: NodeId) -> Option<Box<dyn Node>>;
    /// Connect `source` -> `target` carrying `data`, validating type/rule.
    fn connect_with_data(
        &mut self,
        source: OutputSocketId,
        target: InputSocketId,
        data: Box<dyn EdgeData>,
    ) -> Result<(), ConnectError>;
    /// Connect with default ([`crate::UnitEdge`]) data.
    fn quick_connect(
        &mut self,
        source: OutputSocketId,
        target: InputSocketId,
    ) -> Result<(), ConnectError> {
        self.connect_with_data(source, target, Box::new(crate::edge::UnitEdge))
    }
    /// Disconnect, returning the edge data that was carried.
    fn disconnect(
        &mut self,
        source: OutputSocketId,
        target: InputSocketId,
    ) -> Option<Box<dyn EdgeData>>;
    /// Mutable access to the data carried by an edge (bump version on drop).
    fn edge_data_mut(
        &mut self,
        source: OutputSocketId,
        target: InputSocketId,
    ) -> Option<&mut dyn EdgeData>;
    /// Immutable access to the data carried by an edge.
    fn edge_data(&self, source: OutputSocketId, target: InputSocketId) -> Option<&dyn EdgeData>;

    /// Monotonic version of an edge (bumped when its data mutates), for dirty
    /// detection. `None` if no such edge.
    fn edge_version(&self, source: OutputSocketId, target: InputSocketId) -> Option<u64>;

    /// The type caster used for connect-time compatibility checks.
    fn type_cast(&self) -> &dyn crate::type_tag::TypeCast;
}
