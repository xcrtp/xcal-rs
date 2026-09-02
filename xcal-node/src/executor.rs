//! Node executors and evaluation strategies.
//!
//! A [`NodeExecutor`] knows *how to run one node* given an [`ExecContext`]. It
//! is object-safe and stored as `Box<dyn NodeExecutor>`. A [`TypedNodeExecutor`]
//! is the typed flavour: bound to a concrete `Node` type so it can skip the
//! downcast — adapted to the erased interface by [`TypedExecutorAdapter`].
//!
//! An [`EvalStrategy`] orchestrates a whole evaluation pass over a graph: it
//! decides ordering, dirty propagation, caching and error recovery. Concrete
//! strategies live in `xcal-graph-eval` (e.g. lazy pull evaluation).

use std::marker::PhantomData;

use crate::error::EvalError;
use crate::exec::ExecContext;
use crate::id::OutputSocketId;
use crate::node::Node;

/// Object-safe executor: run `node` using `ctx`.
pub trait NodeExecutor: Send + Sync + 'static {
    fn execute(&self, node: &dyn Node, ctx: &mut dyn ExecContext) -> Result<(), EvalError>;
}

/// Typed executor bound to a concrete node type `N` (no downcast cost).
pub trait TypedNodeExecutor<N: Node>: Send + Sync + 'static {
    fn execute_typed(&self, node: &N, ctx: &mut dyn ExecContext) -> Result<(), EvalError>;
}

/// Adapts a [`TypedNodeExecutor`] to the erased [`NodeExecutor`] interface via
/// a single downcast per execution.
pub struct TypedExecutorAdapter<N: Node, E: TypedNodeExecutor<N>> {
    executor: E,
    _phantom: PhantomData<N>,
}

impl<N: Node, E: TypedNodeExecutor<N>> TypedExecutorAdapter<N, E> {
    pub fn new(executor: E) -> Self {
        Self {
            executor,
            _phantom: PhantomData,
        }
    }
}

impl<N, E> NodeExecutor for TypedExecutorAdapter<N, E>
where
    N: Node,
    E: TypedNodeExecutor<N>,
{
    fn execute(&self, node: &dyn Node, ctx: &mut dyn ExecContext) -> Result<(), EvalError> {
        match node.downcast_ref::<N>() {
            Some(typed) => self.executor.execute_typed(typed, ctx),
            None => Err(EvalError::new(
                "node type mismatch for executor",
                ctx.current_node(),
                None,
            )),
        }
    }
}

/// A whole-graph evaluation pass. `target` optionally restricts evaluation to
/// the subgraph feeding a single output socket.
pub trait EvalStrategy: Send + Sync + 'static {
    fn evaluate(
        &mut self,
        graph: &dyn crate::graph::Graph,
        executor: &dyn NodeExecutor,
        target: Option<OutputSocketId>,
    ) -> Result<(), EvalError>;
}
