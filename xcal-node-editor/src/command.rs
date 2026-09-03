//! Atomic state-change commands — the only contract between the UI layer and
//! the editor. The UI constructs commands; the editor applies them, emits
//! events and records inverse commands for undo.
//!
//! Note: `Command` holds `Box<dyn Node>` / `Box<dyn LayoutStrategy>` and is
//! therefore not `Clone`. The undo stack moves commands by value.

use glam::Vec2;
use xcal_node::{EdgeId, InputSocketId, Node, NodeId, OutputSocketId};

use crate::edge_geom::EdgeGeometry;
use crate::layout::LayoutStrategy;

/// All editor state changes expressed as one atomic command.
pub enum Command {
    /// Insert a node at a graph-local position.
    AddNode {
        node: Box<dyn Node>,
        at: Vec2,
    },
    /// Remove a node (cascades to its edges).
    RemoveNode(NodeId),
    /// Move a node's top-left corner.
    MoveNode {
        node: NodeId,
        to: Vec2,
    },
    /// Record a node card's measured size (from the UI layer).
    SetNodeSize {
        node: NodeId,
        size: Vec2,
    },
    /// User-driven resize of a node card via the grip. The editor clamps
    /// `size` to the node type's `NodeInteraction` min/max bounds; live
    /// drags are coalesced onto one undo entry.
    ResizeNode {
        node: NodeId,
        size: Vec2,
    },

    /// Start dragging a connection out of an output socket.
    BeginConnection(OutputSocketId),
    /// Update the in-flight connection's cursor position.
    UpdateConnectionDrag(Vec2),
    /// Drop onto an input socket to complete the connection.
    CompleteConnection(InputSocketId),
    /// Abort the in-flight connection (dropped on empty space).
    CancelConnection,

    /// Remove an existing edge.
    Disconnect(EdgeId),

    /// Set the selection to `node` (clearing others unless `additive`).
    Select {
        node: NodeId,
        additive: bool,
    },
    ClearSelection,

    /// Replace the layout strategy.
    SetLayout(Box<dyn LayoutStrategy>),
    /// Replace the edge geometry builder.
    SetEdgeGeometry(Box<dyn EdgeGeometry>),
    /// Request an automatic layout pass (no-op for free layouts).
    RecomputeLayout,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::AddNode { at, .. } => f
                .debug_struct("AddNode")
                .field("at", at)
                .finish_non_exhaustive(),
            Command::RemoveNode(n) => f.debug_tuple("RemoveNode").field(n).finish(),
            Command::MoveNode { node, to } => f
                .debug_struct("MoveNode")
                .field("node", node)
                .field("to", to)
                .finish(),
            Command::SetNodeSize { node, size } => f
                .debug_struct("SetNodeSize")
                .field("node", node)
                .field("size", size)
                .finish(),
            Command::ResizeNode { node, size } => f
                .debug_struct("ResizeNode")
                .field("node", node)
                .field("size", size)
                .finish(),
            Command::BeginConnection(s) => f.debug_tuple("BeginConnection").field(s).finish(),
            Command::UpdateConnectionDrag(p) => {
                f.debug_tuple("UpdateConnectionDrag").field(p).finish()
            }
            Command::CompleteConnection(s) => f.debug_tuple("CompleteConnection").field(s).finish(),
            Command::CancelConnection => f.write_str("CancelConnection"),
            Command::Disconnect(e) => f.debug_tuple("Disconnect").field(e).finish(),
            Command::Select { node, additive } => f
                .debug_struct("Select")
                .field("node", node)
                .field("additive", additive)
                .finish(),
            Command::ClearSelection => f.write_str("ClearSelection"),
            Command::SetLayout(_) => f.debug_tuple("SetLayout").finish_non_exhaustive(),
            Command::SetEdgeGeometry(_) => f.debug_tuple("SetEdgeGeometry").finish_non_exhaustive(),
            Command::RecomputeLayout => f.write_str("RecomputeLayout"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EditorEvent;

    #[test]
    fn command_and_event_fields_are_readable() {
        let node = NodeId(7);
        let cmd = Command::MoveNode {
            node,
            to: Vec2::new(12.0, 34.0),
        };
        match cmd {
            Command::MoveNode { node: n, to } => {
                assert_eq!(n, node);
                assert_eq!(to, Vec2::new(12.0, 34.0));
            }
            _ => panic!("wrong variant"),
        }

        let ev = EditorEvent::NodeMoved {
            node,
            to: Vec2::new(12.0, 34.0),
        };
        match ev {
            EditorEvent::NodeMoved { node: n, to } => {
                assert_eq!(n, node);
                assert_eq!(to.y, 34.0);
            }
            _ => panic!("wrong variant"),
        }
    }
}
