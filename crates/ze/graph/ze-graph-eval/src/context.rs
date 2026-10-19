//! Concrete [`ExecContext`] backing used by [`crate::LazyPullEval`].
//!
//! [`DefaultExecContext`] is a *view* handed to a node executor for the
//! duration of one node's evaluation. It owns the node's pre-pulled input
//! values and pre-loaded state (so `peek_input` / `get_state` can return
//! borrows without escaping a `Mutex` guard), and reaches into the shared
//! evaluation caches (behind `Mutex`) for `set_output` / `pull_output`.

use std::any::Any;
use std::collections::HashMap;

use ze_node_core::edge::EdgeData;
use ze_node_core::error::EvalError;
use ze_node_core::exec::ExecContext;
use ze_node_core::id::{InputSocketId, NodeId, OutputSocketId};
use ze_node_core::type_tag::TypeTag;
use ze_node_core::value::NodeValue;

use crate::lazy_eval::EvalRefs;

/// The concrete execution context used by [`crate::LazyPullEval`].
pub struct DefaultExecContext<'a> {
    refs: &'a EvalRefs<'a>,
    current: NodeId,
    inputs: HashMap<InputSocketId, Box<dyn NodeValue>>,
    state: Option<Box<dyn NodeValue>>,
    state_type: Box<dyn TypeTag>,
}

impl<'a> DefaultExecContext<'a> {
    pub(crate) fn new(
        refs: &'a EvalRefs<'a>,
        current: NodeId,
        inputs: HashMap<InputSocketId, Box<dyn NodeValue>>,
        state: Option<Box<dyn NodeValue>>,
        state_type: Box<dyn TypeTag>,
    ) -> Self {
        Self {
            refs,
            current,
            inputs,
            state,
            state_type,
        }
    }

    /// Take the (possibly updated) state so the driver can flush it back to the
    /// shared per-node state cache.
    pub(crate) fn take_state(&mut self) -> Option<Box<dyn NodeValue>> {
        self.state.take()
    }
}

impl<'a> ExecContext for DefaultExecContext<'a> {
    fn current_node(&self) -> NodeId {
        self.current
    }

    fn pull_output(&mut self, socket: OutputSocketId) -> Result<Box<dyn NodeValue>, EvalError> {
        // Ensure the upstream output is computed, then clone it out.
        crate::lazy_eval::ensure_output(socket, self.refs)?;
        let val = self
            .refs
            .eval
            .outputs
            .lock()
            .unwrap()
            .get(&socket)
            .cloned()
            .ok_or_else(|| {
                EvalError::new("no value at output socket", self.current, Some(socket))
            })?;
        Ok(val)
    }

    fn peek_input(&self, socket: InputSocketId) -> Result<&dyn Any, EvalError> {
        self.inputs
            .get(&socket)
            .map(|v| v.as_ref().as_any())
            .ok_or_else(|| EvalError::new("no value at input socket", self.current, None))
    }

    fn push_input(
        &mut self,
        socket: InputSocketId,
        value: Box<dyn NodeValue>,
    ) -> Result<(), EvalError> {
        self.inputs.insert(socket, value);
        Ok(())
    }

    fn set_output(
        &mut self,
        socket: OutputSocketId,
        value: Box<dyn NodeValue>,
    ) -> Result<(), EvalError> {
        self.refs.eval.outputs.lock().unwrap().insert(socket, value);
        Ok(())
    }

    fn get_state(&self) -> Result<&dyn Any, EvalError> {
        self.state
            .as_ref()
            .map(|v| v.as_ref().as_any())
            .ok_or_else(|| EvalError::new("node has no state", self.current, None))
    }

    fn set_state(&mut self, state: Box<dyn NodeValue>) -> Result<(), EvalError> {
        self.state = Some(state);
        Ok(())
    }

    fn state_type(&self) -> Box<dyn TypeTag> {
        TypeTag::clone_box(self.state_type.as_ref())
    }

    fn report_progress(&self, _progress: f32) {
        // no-op by default
    }

    fn edge_data(&self, input: InputSocketId) -> Option<&dyn EdgeData> {
        let upstream = self.refs.graph.upstream(input);
        let out = upstream.first()?;
        self.refs.graph.edge_data(*out, input)
    }
}
