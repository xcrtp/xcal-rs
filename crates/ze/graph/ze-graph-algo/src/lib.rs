//! `zela-graph-algo` — concrete graph algorithms for `zela-node`.
//!
//! Depends only on `zela-node` (the trait layer). Implements
//! [`ze_node_core::algo::TopoSort`], [`ze_node_core::algo::DirtyTracker`] and
//! [`ze_node_core::algo::CycleDetector`]; the algorithms are swappable — any
//! other implementation of the same traits can be plugged into the evaluation
//! pipeline without changes here.

#![forbid(unsafe_code)]

mod bfs_dirty;
mod kahn_sort;
mod tarjan_scc;

pub use bfs_dirty::BfsDirtyTracker;
pub use kahn_sort::KahnSorter;
pub use tarjan_scc::TarjanSccDetector;
