//! `xcal-node-egui` — egui rendering and interaction for `xcal-node-editor`.
//!
//! This crate is the only layer that knows about viewport transforms
//! (pan/zoom), egui painting and pointer/keyboard input. It translates user
//! input into `xcal_node_editor::Command`s and renders the editor's pure data
//! as node cards and edges.
//!
//! Hard constraint: this crate must never depend on `xcal-core` /
//! `xcal-render` / `xcal-app` / `xcal-macro`.
//!
//! Module map:
//! - `view`: `NodeView` / `DefaultNodeView` card appearance (stage 7)
//! - `edge_render`: `EdgeRenderer`, bezier/orthogonal painters (stage 7)
//! - `painter`: `GraphPainter` full-scene painting (stage 7)
//! - `canvas`: `GraphCanvas` pan/zoom viewport transform (stage 8)
//! - `input`: `InputHandler` pointer/keyboard -> commands (stage 8)
//! - `app`: `NodeGraphApp` per-frame driver (stage 8)

#![forbid(unsafe_code)]

pub mod edge_render;
pub mod painter;
pub mod view;

pub use edge_render::{
    ArrowStyle, BezierRenderer, EdgeRenderer, EdgeStyle, OrthogonalRenderer, StrokeKind,
};
pub use painter::GraphPainter;
pub use view::{DefaultNodeView, NodeView};
