//! Socket descriptors attached to nodes.
//!
//! Sockets are *descriptors*: they describe a slot's type and connection
//! rules. They are owned by the node and returned by reference from
//! [`crate::Node::inputs`] / [`crate::Node::outputs`]. Identity of a socket
//! (for graph wiring) is given by [`crate::SocketId`] — a node handle plus a
//! local index — not by these descriptors.

use crate::id::{InputSocketId, NodeId, OutputSocketId};
use crate::type_tag::TypeTag;
use crate::value::NodeValue;

/// A typed input slot on a node.
#[derive(Debug, Clone)]
pub struct InputSocket {
    /// Owning node (filled in when the node is inserted).
    pub node: NodeId,
    /// Local index within the node's input list.
    pub index: u32,
    /// Type this input accepts.
    pub type_tag: Box<dyn TypeTag>,
    /// Optional default value used when no edge is connected.
    pub default_value: Option<Box<dyn NodeValue>>,
    /// Whether more than one incoming edge may connect here.
    pub allow_multi: bool,
}

impl InputSocket {
    pub fn new(type_tag: Box<dyn TypeTag>) -> Self {
        Self {
            node: NodeId(0),
            index: 0,
            type_tag,
            default_value: None,
            allow_multi: false,
        }
    }

    pub fn with_index(mut self, index: u32) -> Self {
        self.index = index;
        self
    }

    pub fn with_node(mut self, node: NodeId) -> Self {
        self.node = node;
        self
    }

    pub fn with_default(mut self, value: Box<dyn NodeValue>) -> Self {
        self.default_value = Some(value);
        self
    }

    pub fn allow_multi(mut self, allow: bool) -> Self {
        self.allow_multi = allow;
        self
    }

    pub fn id(&self) -> InputSocketId {
        InputSocketId::new(self.node, self.index)
    }
}

/// A typed output slot on a node.
#[derive(Debug, Clone)]
pub struct OutputSocket {
    pub node: NodeId,
    pub index: u32,
    pub type_tag: Box<dyn TypeTag>,
    /// Max number of downstream edges; `None` = unlimited.
    pub max_connections: Option<u32>,
}

impl OutputSocket {
    pub fn new(type_tag: Box<dyn TypeTag>) -> Self {
        Self {
            node: NodeId(0),
            index: 0,
            type_tag,
            max_connections: None,
        }
    }

    pub fn with_index(mut self, index: u32) -> Self {
        self.index = index;
        self
    }

    pub fn with_node(mut self, node: NodeId) -> Self {
        self.node = node;
        self
    }

    pub fn with_max_connections(mut self, max: u32) -> Self {
        self.max_connections = Some(max);
        self
    }

    pub fn id(&self) -> OutputSocketId {
        OutputSocketId::new(self.node, self.index)
    }
}
