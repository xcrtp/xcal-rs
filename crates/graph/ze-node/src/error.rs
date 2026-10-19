//! Evaluation errors and the recovery handle protocol.
//!
//! When a node fails during evaluation, the strategy returns an [`EvalError`]
//! carrying a [`RecoveryHandle`]. The caller decides how to recover by invoking
//! one of the handle's methods (`retry` / `resume_with` / `skip_branch` /
//! `replace_and_retry`), which records a [`RecoveryAction`]. The driving
//! strategy then consumes the chosen action and continues the evaluation
//! accordingly. The error policy ([`EvalErrorPolicy`]) controls how failures
//! propagate across independent branches.

use std::fmt;

use crate::id::{InputSocketId, NodeId, OutputSocketId};
use crate::value::NodeValue;

/// What the caller asked the evaluator to do after a failure.
#[derive(Debug)]
pub enum RecoveryAction {
    /// Re-run the failed node from scratch.
    Retry,
    /// Inject `value` at the failed output and let downstream continue.
    ResumeWith(Box<dyn NodeValue>),
    /// Abort the failed node and its entire downstream subgraph.
    SkipBranch,
    /// Replace the listed inputs, then re-run the failed node.
    ReplaceAndRetry {
        replacements: Vec<(InputSocketId, Box<dyn NodeValue>)>,
    },
}

/// A handle returned with an [`EvalError`] that lets the caller pick a recovery
/// strategy. The chosen action is stored and later consumed by the evaluator.
pub struct RecoveryHandle {
    node: NodeId,
    /// The output socket whose evaluation failed (if applicable).
    socket: Option<OutputSocketId>,
    action: Option<RecoveryAction>,
    /// Whether the handle has already been consumed.
    consumed: bool,
}

impl fmt::Debug for RecoveryHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveryHandle")
            .field("node", &self.node)
            .field("socket", &self.socket)
            .field("action", &self.action)
            .field("consumed", &self.consumed)
            .finish()
    }
}

impl RecoveryHandle {
    pub(crate) fn new(node: NodeId, socket: Option<OutputSocketId>) -> Self {
        Self {
            node,
            socket,
            action: None,
            consumed: false,
        }
    }

    pub fn node(&self) -> NodeId {
        self.node
    }

    pub fn socket(&self) -> Option<OutputSocketId> {
        self.socket
    }

    /// Choose to re-run the failed node.
    pub fn retry(&mut self) {
        self.set(RecoveryAction::Retry);
    }

    /// Inject `value` at the failed output and continue downstream.
    pub fn resume_with(&mut self, value: Box<dyn NodeValue>) {
        self.set(RecoveryAction::ResumeWith(value));
    }

    /// Abort the failed node and its downstream subgraph.
    pub fn skip_branch(&mut self) {
        self.set(RecoveryAction::SkipBranch);
    }

    /// Replace the listed inputs, then re-run the failed node.
    pub fn replace_and_retry(&mut self, replacements: Vec<(InputSocketId, Box<dyn NodeValue>)>) {
        self.set(RecoveryAction::ReplaceAndRetry { replacements });
    }

    fn set(&mut self, action: RecoveryAction) {
        self.action = Some(action);
    }

    /// Take the chosen action (the evaluator calls this once).
    pub fn take_action(&mut self) -> Option<RecoveryAction> {
        if self.consumed {
            return None;
        }
        self.consumed = true;
        self.action.take()
    }

    /// Has the caller chosen an action yet?
    pub fn pending_action(&self) -> bool {
        self.action.is_some()
    }
}

/// A node evaluation failure, bundled with a recovery handle.
#[derive(Debug, thiserror::Error)]
pub struct EvalError {
    /// Human-readable failure reason.
    message: String,
    #[source]
    source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
    handle: RecoveryHandle,
}

impl EvalError {
    pub fn new(message: impl Into<String>, node: NodeId, socket: Option<OutputSocketId>) -> Self {
        Self {
            message: message.into(),
            source: None,
            handle: RecoveryHandle::new(node, socket),
        }
    }

    pub fn with_source(
        message: impl Into<String>,
        node: NodeId,
        socket: Option<OutputSocketId>,
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    ) -> Self {
        Self {
            message: message.into(),
            source: Some(source),
            handle: RecoveryHandle::new(node, socket),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn node(&self) -> NodeId {
        self.handle.node
    }

    pub fn socket(&self) -> Option<OutputSocketId> {
        self.handle.socket
    }

    /// Mutable access to the recovery handle so the caller can pick an action.
    pub fn recovery(&mut self) -> &mut RecoveryHandle {
        &mut self.handle
    }

    /// Take ownership of the recovery handle.
    pub fn into_recovery(self) -> RecoveryHandle {
        self.handle
    }
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "eval error at {}: {}", self.handle.node, self.message)
    }
}

/// How the evaluator should treat failures across independent branches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EvalErrorPolicy {
    /// Stop the whole evaluation on the first error.
    #[default]
    FailFast,
    /// Continue evaluating other branches that do not depend on the failure.
    ContinueOtherBranches,
    /// Evaluate everything possible and aggregate every error.
    CollectAll,
}
