//! Logical events emitted by the editor after applying commands.

use glam::Vec2;
use ze_node::{ConnectError, EdgeId, InputSocketId, NodeId, OutputSocketId};

/// Notifications produced by the editor, drained by the UI layer after each
/// batch of commands.
#[derive(Debug)]
pub enum EditorEvent {
    NodeAdded(NodeId),
    NodeRemoved(NodeId),
    NodeMoved {
        node: NodeId,
        to: Vec2,
    },
    /// A node card was user-resized (already clamped to interaction bounds).
    NodeResized {
        node: NodeId,
        size: Vec2,
    },
    Connected(EdgeId),
    Disconnected(EdgeId),
    /// A connection attempt was rejected (type mismatch, cycle, ...); no
    /// state changed.
    ConnectionRejected {
        from: OutputSocketId,
        to: InputSocketId,
        reason: ConnectError,
    },
    SelectionChanged,
    LayoutChanged,
    EdgeGeometryChanged,
    /// Undo/redo of node insertion may assign a fresh [`NodeId`]; the UI uses
    /// this to migrate any externally held references.
    NodeIdRemapped {
        old: NodeId,
        new: NodeId,
    },
}
