//! Pure geometry types shared by the editor state machine and the UI layer.
//!
//! All coordinates are **graph local**: origin at the graph's top-left,
//! y-axis pointing down, 1:1 with pixels. Viewport transforms (pan/zoom) live
//! exclusively in the UI layer; this crate never sees screen coordinates.

pub use glam::Vec2;

use ze_node::{EdgeId, InputSocketId, NodeId, OutputSocketId};

/// Axis-aligned rectangle in graph local coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect {
    /// Build from a top-left corner and a (positive) size.
    pub fn from_min_size(min: Vec2, size: Vec2) -> Self {
        Self {
            min,
            max: min + size,
        }
    }

    /// Width / height.
    pub fn size(self) -> Vec2 {
        self.max - self.min
    }

    /// Center point.
    pub fn center(self) -> Vec2 {
        (self.min + self.max) * 0.5
    }

    /// Point-in-rect test (inclusive on all edges, for hit-testing).
    pub fn contains(self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    /// Grow (or shrink, for negative `r`) the rectangle by `r` on every side.
    /// Used to add hit-test tolerance.
    pub fn expand(self, r: f32) -> Self {
        Self {
            min: self.min - Vec2::splat(r),
            max: self.max + Vec2::splat(r),
        }
    }
}

/// Result of a hit test. Variants are ordered by z-priority: sockets are
/// tested before resize grips, grips before cards, cards before edges;
/// callers match top-down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitTarget {
    Node(NodeId),
    InputSocket(InputSocketId),
    OutputSocket(OutputSocketId),
    /// The bottom-right resize grip of a node card.
    ResizeHandle(NodeId),
    Edge(EdgeId),
    Nothing,
}

/// Hit radius (graph-local units / pixels at zoom 1.0) for socket picking.
/// Generous on purpose: the on-screen dot is small, but clicks anywhere in
/// this disk grab the pin.
pub const SOCKET_HIT_RADIUS: f32 = 12.0;

/// Snap radius (graph-local units): while dragging a connection, the cursor
/// is attracted to the nearest type-compatible input socket within this
/// distance (edge endpoint snaps to the pin and releasing completes the
/// connection even if the pointer is slightly off).
pub const SOCKET_SNAP_RADIUS: f32 = 22.0;

/// Hit radius (graph-local units) for the bottom-right resize grip.
pub const RESIZE_HIT_RADIUS: f32 = 14.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_contains_basic() {
        let r = Rect::from_min_size((0.0, 0.0).into(), (10.0, 20.0).into());
        assert!(r.contains((5.0, 5.0).into()));
        assert!(!r.contains((11.0, 5.0).into()));
    }

    #[test]
    fn rect_size_and_center() {
        let r = Rect::from_min_size((0.0, 0.0).into(), (10.0, 20.0).into());
        assert_eq!(r.size(), Vec2::new(10.0, 20.0));
        assert_eq!(r.center(), Vec2::new(5.0, 10.0));
    }

    #[test]
    fn rect_expand_adds_tolerance() {
        let r = Rect::from_min_size((0.0, 0.0).into(), (10.0, 10.0).into());
        let p = (12.0, 5.0).into();
        assert!(!r.contains(p));
        assert!(r.expand(3.0).contains(p));
    }

    #[test]
    fn rect_contains_edges_inclusively() {
        let r = Rect::from_min_size((0.0, 0.0).into(), (10.0, 10.0).into());
        assert!(r.contains((0.0, 0.0).into()));
        assert!(r.contains((10.0, 10.0).into()));
    }
}
