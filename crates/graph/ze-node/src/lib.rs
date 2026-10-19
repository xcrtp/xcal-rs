//! `zela-node` — node editor core trait layer.
//!
//! Pure interface crate: defines *what the world looks like* (value types,
//! sockets, edges, nodes, graphs, execution context) without committing to any
//! algorithm, storage or execution strategy. It deliberately has **no
//! dependency** on the rest of the zela project (`zela-scene`/`zela-render`/
//! `zela-egui`/`zela-app`/`ze-macro`) so it can be reused as a standalone
//! node-graph kernel.

#![forbid(unsafe_code)]

pub mod algo;
pub mod edge;
pub mod error;
pub mod exec;
pub mod executor;
pub mod executor_factory;
pub mod graph;
pub mod id;
pub mod node;
pub mod simple_graph;
pub mod socket;
pub mod type_tag;
pub mod value;

pub use algo::{CycleDetector, CycleError, DirtyTracker, TopoSort};
pub use edge::{Edge, EdgeData, EdgeId, UnitEdge};
pub use error::{EvalError, EvalErrorPolicy, RecoveryAction, RecoveryHandle};
pub use exec::ExecContext;
pub use executor::{EvalStrategy, NodeExecutor, TypedExecutorAdapter, TypedNodeExecutor};
pub use executor_factory::{DynNodeExecutor, ExecutorFactory};
pub use graph::{ConnectError, Graph};
pub use id::{InputSocketId, NodeId, OutputSocketId, SocketId};
pub use node::{DirtyFn, DirtyPolicy, DowncastWrapper, Node, NodeInteraction, NodeMeta};
pub use simple_graph::SimpleGraph;
pub use socket::{InputSocket, OutputSocket};
pub use type_tag::{DefaultTypeCast, NamedTypeTag, TypeCast, TypeTag};
pub use value::NodeValue;
