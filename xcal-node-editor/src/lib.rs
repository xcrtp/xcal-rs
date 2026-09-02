//! `xcal-node-editor` — pure-logic node editor state machine.
//!
//! This crate sits between the domain model (`xcal-node`) and any UI layer.
//! It owns **no** viewport / pan / zoom concept: all coordinates are graph
//! local (`glam::Vec2`, origin at the graph's top-left, y down, 1:1 with
//! pixels). The UI layer is responsible for screen <-> graph transforms.
//!
//! Hard constraint: this crate must never depend on `egui`/`eframe` or any
//! `xcal-core`/`xcal-render`/`xcal-app` crate.
//!
//! Module map:
//! - `geometry`: `Rect`, `HitTarget`, hit-test constants (stage 1)
//! - `layout`: `LayoutStrategy` / `FreeLayout` node placement (stage 2)
//! - `socket_layout`: `SocketLayout` / `DefaultSocketLayout` port offsets (stage 2)
//! - `edge_geom`: `EdgePath`, `EdgeGeometry`, bezier/orthogonal builders (stage 3)
//! - `command`: atomic, undoable `Command`s (stage 4)
//! - `event`: `EditorEvent` notifications (stage 4)
//! - `error`: `EditorError` (stage 4)
//! - `editor`: the `Editor` state machine (stage 5)
//! - `validation`: type + cycle connection validation (stage 6)
//! - `undo`: `UndoStack` (stage 6)

#![forbid(unsafe_code)]

pub mod geometry;

pub use geometry::{HitTarget, Rect, Vec2};
