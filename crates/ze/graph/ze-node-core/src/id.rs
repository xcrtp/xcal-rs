//! Stable identifiers for nodes and sockets.
//!
//! [`NodeId`] is a flat 64-bit handle assigned by the graph when a node is
//! inserted. [`SocketId`] pairs a node handle with a *local* index so socket
//! identity is stable across node reordering. [`InputSocketId`] /
//! [`OutputSocketId`] are newtyped wrappers that make the direction explicit at
//! the type level, preventing accidental cross-wiring.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Handle to a node inside a graph.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
pub struct NodeId(pub u64);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeId({})", self.0)
    }
}

impl NodeId {
    /// Raw integer value.
    pub const fn to_u64(self) -> u64 {
        self.0
    }
}

/// A node-local socket address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SocketId {
    /// Owning node.
    pub node: NodeId,
    /// Local socket index (per direction: inputs and outputs are indexed
    /// independently starting from 0).
    pub index: u32,
}

impl SocketId {
    pub const fn new(node: NodeId, index: u32) -> Self {
        Self { node, index }
    }
}

/// Address of an *input* socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct InputSocketId(pub SocketId);

impl InputSocketId {
    pub const fn new(node: NodeId, index: u32) -> Self {
        Self(SocketId::new(node, index))
    }
    pub const fn socket(self) -> SocketId {
        self.0
    }
    pub const fn node(self) -> NodeId {
        self.0.node
    }
    pub const fn index(self) -> u32 {
        self.0.index
    }
}

/// Address of an *output* socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OutputSocketId(pub SocketId);

impl OutputSocketId {
    pub const fn new(node: NodeId, index: u32) -> Self {
        Self(SocketId::new(node, index))
    }
    pub const fn socket(self) -> SocketId {
        self.0
    }
    pub const fn node(self) -> NodeId {
        self.0.node
    }
    pub const fn index(self) -> u32 {
        self.0.index
    }
}
