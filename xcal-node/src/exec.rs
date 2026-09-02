//! Execution context — the interface a node executor sees while a node runs.
//!
//! [`ExecContext`] is intentionally a trait so different evaluation strategies
//! can back it. Values are type-erased [`crate::NodeValue`] boxes: cloneable
//! (for fan-out) and downcastable (for typed reads inside an executor).
//!
//! The context exposes:
//!   * [`ExecContext::pull_output`] — lazily retrieve (computing if needed) the
//!     value of an upstream output socket (owned clone);
//!   * [`ExecContext::peek_input`] — borrow the value arriving at an input
//!     socket (no clone; the primary read path for executors);
//!   * [`ExecContext::push_input`] / [`ExecContext::set_output`] — inject input
//!     values / write a node's computed output;
//!   * per-node state (`get_state` / `set_state`) surviving across evaluations;
//!   * `edge_data` access for the connection currently feeding an input;
//!   * progress reporting.
//!
//! A per-socket global value cache lives behind the trait and survives across
//! `evaluate` calls so unchanged upstream values are not recomputed.

use std::any::Any;

use crate::edge::EdgeData;
use crate::error::EvalError;
use crate::id::{InputSocketId, NodeId, OutputSocketId};
use crate::type_tag::TypeTag;
use crate::value::NodeValue;

/// What a node executor can do with the world while a node evaluates.
pub trait ExecContext: Send + Sync {
    /// The node currently being evaluated (set by the driving strategy).
    fn current_node(&self) -> NodeId;

    /// Lazily pull (computing if needed) an owned clone of an output's value.
    fn pull_output(&mut self, socket: OutputSocketId) -> Result<Box<dyn NodeValue>, EvalError>;

    /// Borrow the value arriving at an input socket (no clone). Primary read
    /// path for executors.
    fn peek_input(&self, socket: InputSocketId) -> Result<&dyn Any, EvalError>;

    /// Inject a value into a node's input cache (e.g. defaults, recovery).
    fn push_input(&mut self, socket: InputSocketId, value: Box<dyn NodeValue>) -> Result<(), EvalError>;

    /// Write the value a node produced at one of its output sockets.
    fn set_output(
        &mut self,
        socket: OutputSocketId,
        value: Box<dyn NodeValue>,
    ) -> Result<(), EvalError>;

    /// Read the node's persistent state (borrowed).
    fn get_state(&self) -> Result<&dyn Any, EvalError>;

    /// Replace the node's persistent state.
    fn set_state(&mut self, state: Box<dyn NodeValue>) -> Result<(), EvalError>;

    /// The type tag describing the node's persistent state.
    fn state_type(&self) -> Box<dyn TypeTag>;

    /// Report evaluation progress in `[0.0, 1.0]`.
    fn report_progress(&self, progress: f32);

    /// Edge data flowing into `input`, if any edge is connected.
    fn edge_data(&self, input: InputSocketId) -> Option<&dyn EdgeData>;
}
