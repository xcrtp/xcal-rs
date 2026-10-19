//! [`ExecutorFactory`] — a type-erased node-executor registry.
//!
//! Maps a node's concrete [`std::any::TypeId`] to its [`NodeExecutor`]. This is
//! the "DynNodeExecutor" of the plan: a single erased dispatcher the evaluation
//! strategy consults to run a node without knowing its concrete type. Supports
//! runtime registration (plugin scenario); compile-time-known registrations can
//! be wired up at startup.

use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;

use crate::executor::NodeExecutor;
use crate::node::Node;

/// Type-erased executor registry keyed by node `TypeId`.
#[derive(Default)]
pub struct ExecutorFactory {
    executors: HashMap<TypeId, Box<dyn NodeExecutor>>,
}

impl ExecutorFactory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `executor` for nodes of concrete type `N`.
    pub fn register<N: Node>(&mut self, executor: Box<dyn NodeExecutor>) {
        self.executors.insert(TypeId::of::<N>(), executor);
    }

    /// Register by an explicit `TypeId` (for already-erased node references).
    pub fn register_typeid(&mut self, id: TypeId, executor: Box<dyn NodeExecutor>) {
        self.executors.insert(id, executor);
    }

    /// Look up the executor registered for `node`'s concrete type.
    pub fn get_for_node(&self, node: &dyn Node) -> Option<&dyn NodeExecutor> {
        self.get(node.type_id())
    }

    /// Look up by `TypeId`.
    pub fn get(&self, id: TypeId) -> Option<&dyn NodeExecutor> {
        self.executors.get(&id).map(|e| e.as_ref())
    }

    pub fn len(&self) -> usize {
        self.executors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.executors.is_empty()
    }
}

impl fmt::Debug for ExecutorFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecutorFactory")
            .field("registered", &self.executors.len())
            .finish()
    }
}

/// Type alias honoring the plan's "DynNodeExecutor" naming.
pub type DynNodeExecutor = ExecutorFactory;
