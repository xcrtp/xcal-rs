//! `xcal-graph-eval` — evaluation strategies for `xcal-node`.
//!
//! Depends only on `xcal-node`. Provides the default [`LazyPullEval`] strategy
//! (reverse-dependency lazy evaluation with dirty propagation and error
//! recovery) plus a concrete [`DefaultExecContext`] backing the
//! [`ExecContext`] trait.

#![forbid(unsafe_code)]

mod context;
mod lazy_eval;

pub use context::DefaultExecContext;
pub use lazy_eval::LazyPullEval;
