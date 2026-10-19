//! `zela-graph-eval` — evaluation strategies for `zela-node`.
//!
//! Depends only on `zela-node`. Provides the default [`LazyPullEval`] strategy
//! (reverse-dependency lazy evaluation with dirty propagation and error
//! recovery) plus a concrete [`DefaultExecContext`] backing the
//! [`ExecContext`] trait.

#![forbid(unsafe_code)]

mod context;
mod lazy_eval;

pub use context::DefaultExecContext;
pub use lazy_eval::LazyPullEval;
