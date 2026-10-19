//! Node placement strategies.
//!
//! A [`LayoutStrategy`] answers "where is node N?" in graph-local
//! coordinates. The editor layer only *stores and consumes* positions: the UI
//! layer measures node sizes and injects them via [`NodeSizeMap`], while
//! automatic strategies (stage 10) may rearrange nodes topologically.

use std::collections::HashMap;

use glam::Vec2;
use ze_node_core::{Graph, NodeId};

/// Measured node card sizes, keyed by node. Populated by the UI layer after
/// laying out each card's body; the editor layer treats it as read-only input.
pub type NodeSizeMap = HashMap<NodeId, Vec2>;

/// Where nodes live in graph space.
pub trait LayoutStrategy: Send + Sync + 'static {
    /// Current position of a node's top-left corner, if known.
    fn position_of(&self, node: NodeId) -> Option<Vec2>;
    /// Record/override a node position (user drag or spawn).
    fn set_position(&mut self, node: NodeId, pos: Vec2);
    /// Forget a node (called when the node is removed).
    fn remove(&mut self, node: NodeId);
    /// Drop all positions.
    fn clear(&mut self);
    /// Automatic layout: rearrange nodes by topology. Free-form layouts
    /// implement this as a no-op.
    fn recompute(&mut self, _graph: &dyn Graph, _sizes: &NodeSizeMap) {}
    /// Mark a node's layout dirty (for future incremental recompute).
    fn invalidate(&mut self, _node: NodeId) {}
    /// `false` for free-form layouts: automatic layout must never overwrite
    /// user-chosen positions.
    fn auto_layout_supported(&self) -> bool {
        true
    }
}

/// Free-form layout: positions are entirely user-driven; `recompute` is a
/// no-op. New nodes land on a diagonal staircase so they never spawn exactly
/// on top of each other.
#[derive(Debug, Clone)]
pub struct FreeLayout {
    positions: HashMap<NodeId, Vec2>,
    spawn_origin: Vec2,
    spawn_step: Vec2,
}

impl Default for FreeLayout {
    fn default() -> Self {
        Self {
            positions: HashMap::new(),
            spawn_origin: Vec2::new(64.0, 64.0),
            spawn_step: Vec2::new(40.0, 40.0),
        }
    }
}

impl FreeLayout {
    pub fn new() -> Self {
        Self::default()
    }

    /// Customize the diagonal spawn staircase.
    pub fn with_spawn(origin: Vec2, step: Vec2) -> Self {
        Self {
            spawn_origin: origin,
            spawn_step: step,
            ..Self::default()
        }
    }

    /// Default landing spot for the next spawned node:
    /// `spawn_origin + n * spawn_step`, where `n` is the number of placed
    /// nodes. Consecutive calls (without nodes being added in between) return
    /// the same spot; the editor calls this once per spawn.
    pub fn next_spawn_position(&self) -> Vec2 {
        self.spawn_origin + self.spawn_step * self.positions.len() as f32
    }
}

impl LayoutStrategy for FreeLayout {
    fn position_of(&self, node: NodeId) -> Option<Vec2> {
        self.positions.get(&node).copied()
    }

    fn set_position(&mut self, node: NodeId, pos: Vec2) {
        self.positions.insert(node, pos);
    }

    fn remove(&mut self, node: NodeId) {
        self.positions.remove(&node);
    }

    fn clear(&mut self) {
        self.positions.clear();
    }

    fn auto_layout_supported(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_layout_set_get_remove_clear() {
        let mut l = FreeLayout::new();
        let a = NodeId(1);
        let b = NodeId(2);
        assert_eq!(l.position_of(a), None);
        l.set_position(a, Vec2::new(10.0, 20.0));
        l.set_position(b, Vec2::new(30.0, 40.0));
        assert_eq!(l.position_of(a), Some(Vec2::new(10.0, 20.0)));
        assert_eq!(l.position_of(b), Some(Vec2::new(30.0, 40.0)));
        l.remove(a);
        assert_eq!(l.position_of(a), None);
        assert_eq!(l.position_of(b), Some(Vec2::new(30.0, 40.0)));
        l.clear();
        assert_eq!(l.position_of(b), None);
    }

    #[test]
    fn free_layout_spawn_positions_staircase_and_never_overlap() {
        let mut l = FreeLayout::new();
        let p0 = l.next_spawn_position();
        l.set_position(NodeId(1), p0);
        let p1 = l.next_spawn_position();
        l.set_position(NodeId(2), p1);
        let p2 = l.next_spawn_position();
        assert_ne!(p0, p1);
        assert_ne!(p1, p2);
        // Strictly increasing along the diagonal.
        assert!(p1.x > p0.x && p1.y > p0.y);
        assert!(p2.x > p1.x && p2.y > p1.y);
    }

    #[test]
    fn free_layout_does_not_support_auto_layout() {
        assert!(!FreeLayout::new().auto_layout_supported());
    }
}
