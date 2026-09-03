//! Editor-level errors.

use xc_node::{ConnectError, NodeId};

/// Failures returned by [`crate::Editor::apply`] and friends.
#[derive(Debug, thiserror::Error)]
pub enum EditorError {
    #[error("node {0} not found")]
    NodeNotFound(NodeId),
    #[error("connection rejected: {0}")]
    ConnectionRejected(ConnectError),
    #[error("no pending connection")]
    NoPendingConnection,
    #[error("graph mutation failed: {0}")]
    Graph(#[from] ConnectError),
}
