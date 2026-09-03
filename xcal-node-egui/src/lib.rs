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
//! - `view`: `NodeView` / `DefaultNodeView` card appearance
//! - `edge_render`: `EdgeRenderer`, bezier/orthogonal painters
//! - `painter`: `GraphPainter` full-scene painting
//! - `canvas`: `GraphCanvas` pan/zoom viewport transform
//! - `input`: `InputHandler` pointer/keyboard -> commands
//! - `app`: `NodeGraphApp` per-frame driver

#![forbid(unsafe_code)]

pub mod app;
pub mod canvas;
pub mod edge_render;
pub mod input;
pub mod painter;
pub mod view;

pub use app::NodeGraphApp;
pub use canvas::GraphCanvas;
pub use edge_render::{
    ArrowStyle, BezierRenderer, EdgeRenderer, EdgeStyle, OrthogonalRenderer, StrokeKind,
};
pub use input::InputHandler;
pub use painter::GraphPainter;
pub use view::{DefaultNodeView, NodeView};

/// Re-export of the editor's card geometry constants so downstream UI
/// crates (e.g. `xcal-node-egui-ui`) can size node cards consistently with
/// where the editor/painter place pins and widgets, without depending on
/// `xcal-node-editor` directly.
pub use xcal_node_editor::socket_layout;
