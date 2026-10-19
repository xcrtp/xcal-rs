//! `zela-node-editor` — pure-logic node editor state machine.
//!
//! This crate sits between the domain model (`zela-node`) and any UI layer.
//! It owns **no** viewport / pan / zoom concept: all coordinates are graph
//! local (`glam::Vec2`, origin at the graph's top-left, y down, 1:1 with
//! pixels). The UI layer is responsible for screen <-> graph transforms.
//!
//! Hard constraint: this crate must never depend on `egui`/`eframe` or any
//! `zela-scene`/`zela-render`/`zela-app` crate.
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

pub mod command;
pub mod edge_geom;
pub mod editor;
pub mod error;
pub mod event;
pub mod geometry;
pub mod hit_test;
pub mod layout;
pub mod socket_layout;
pub mod undo;
pub mod validation;

#[cfg(test)]
mod test_util;

pub use command::Command;
pub use edge_geom::{BezierGeometry, EdgeGeometry, EdgePath, OrthogonalGeometry};
pub use editor::{Editor, PendingConnection};
pub use error::EditorError;
pub use event::EditorEvent;
pub use geometry::{
    HitTarget, RESIZE_HIT_RADIUS, Rect, SOCKET_HIT_RADIUS, SOCKET_SNAP_RADIUS, Vec2,
};
pub use hit_test::{
    DEFAULT_RESIZE_HIT_RADIUS, DEFAULT_SOCKET_HIT_RADIUS, DefaultHitTestPolicy, HitTestContext,
    HitTestPolicy,
};
pub use layout::{FreeLayout, LayoutStrategy, NodeSizeMap};
pub use socket_layout::{
    CARD_HEADER_HEIGHT, DefaultSocketLayout, SOCKET_BOTTOM_GAP, SOCKET_ROW_HEIGHT,
    SOCKET_TOP_MARGIN, SocketLayout,
};
pub use undo::UndoStack;
pub use validation::ConnectionValidator;
