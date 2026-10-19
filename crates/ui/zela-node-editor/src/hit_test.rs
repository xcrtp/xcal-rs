//! Pluggable hit-testing: resolving a graph-local pointer position to a
//! [`HitTarget`].
//!
//! The editor delegates every pick to a [`HitTestPolicy`] trait object. The
//! built-in [`DefaultHitTestPolicy`] implements the standard priority
//! (output sockets → input sockets → resize grips → cards → edges) and is
//! data-driven by each node type's [`ze_node_core::NodeMeta::interaction`]
//! metadata (e.g. non-resizable nodes expose no grip). Host applications can
//! swap in a custom policy via `Editor::set_hit_policy` to add extra hit
//! regions, change priorities or disable interactions globally — without
//! touching the editor state machine.

use glam::Vec2;
use ze_node_core::{EdgeId, Graph, InputSocketId, NodeId, OutputSocketId};

use crate::edge_geom::EdgePath;
use crate::geometry::{HitTarget, RESIZE_HIT_RADIUS, Rect, SOCKET_HIT_RADIUS};

/// Read-only view of the editor state a policy needs to resolve picks.
///
/// Geometry is exposed through closures (rather than handing out the whole
/// `Editor`) so policies stay decoupled from the state machine and remain
/// testable with hand-built contexts.
pub struct HitTestContext<'a> {
    /// The graph being edited (nodes, sockets, edges).
    pub graph: &'a dyn Graph,
    /// Picking radius for input/output sockets (graph units).
    pub socket_hit_radius: f32,
    /// Picking radius along edge paths (graph units).
    pub edge_hit_radius: f32,
    /// Picking radius for the bottom-right resize grip (graph units).
    pub resize_hit_radius: f32,
    /// Card rect lookup (`None` for unknown/not-yet-laid-out nodes).
    pub node_rect: &'a dyn Fn(NodeId) -> Option<Rect>,
    /// Absolute input socket position lookup.
    pub input_socket_pos: &'a dyn Fn(InputSocketId) -> Option<Vec2>,
    /// Absolute output socket position lookup.
    pub output_socket_pos: &'a dyn Fn(OutputSocketId) -> Option<Vec2>,
    /// Committed-edge path lookup.
    pub edge_path: &'a dyn Fn(OutputSocketId, InputSocketId) -> Option<EdgePath>,
}

/// Strategy object deciding what UI element sits under a graph-local point.
///
/// Implementors must be `Send + Sync + 'static` so the editor (and its undo
/// stack, which is `Send`) can own them as trait objects.
pub trait HitTestPolicy: Send + Sync + 'static {
    /// Resolve `pos` (graph-local coordinates) to a [`HitTarget`].
    fn hit_test(&self, ctx: &HitTestContext<'_>, pos: Vec2) -> HitTarget;
}

/// Built-in policy: sockets first, then resize grips, then cards, then edges.
///
/// Node interaction is metadata-driven: nodes whose
/// [`ze_node_core::NodeInteraction`] marks `resizable = false` expose no grip.
/// Newest nodes (highest [`NodeId`]) win overlaps, matching draw order.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultHitTestPolicy;

impl DefaultHitTestPolicy {
    pub fn new() -> Self {
        Self
    }

    /// Nodes in top-most-first order (latest inserted on top).
    fn ordered_nodes(ctx: &HitTestContext<'_>) -> Vec<NodeId> {
        let mut nodes: Vec<NodeId> = ctx.graph.all_nodes().collect();
        nodes.sort_unstable_by(|a, b| b.cmp(a));
        nodes
    }

    fn pick_outputs(ctx: &HitTestContext<'_>, pos: Vec2, nodes: &[NodeId]) -> HitTarget {
        for &node in nodes {
            let Some(n) = ctx.graph.get_node(node) else {
                continue;
            };
            for i in 0..n.outputs().len() {
                let id = OutputSocketId::new(node, i as u32);
                if let Some(p) = (ctx.output_socket_pos)(id)
                    && (p - pos).length() <= ctx.socket_hit_radius
                {
                    return HitTarget::OutputSocket(id);
                }
            }
        }
        HitTarget::Nothing
    }

    fn pick_inputs(ctx: &HitTestContext<'_>, pos: Vec2, nodes: &[NodeId]) -> HitTarget {
        for &node in nodes {
            let Some(n) = ctx.graph.get_node(node) else {
                continue;
            };
            for i in 0..n.inputs().len() {
                let id = InputSocketId::new(node, i as u32);
                if let Some(p) = (ctx.input_socket_pos)(id)
                    && (p - pos).length() <= ctx.socket_hit_radius
                {
                    return HitTarget::InputSocket(id);
                }
            }
        }
        HitTarget::Nothing
    }

    fn pick_resize_grips(ctx: &HitTestContext<'_>, pos: Vec2, nodes: &[NodeId]) -> HitTarget {
        for &node in nodes {
            let Some(n) = ctx.graph.get_node(node) else {
                continue;
            };
            if !n.meta().interaction.resizable {
                continue;
            }
            if let Some(rect) = (ctx.node_rect)(node) {
                // Grip sits at the card's bottom-right corner.
                if (rect.max - pos).length() <= ctx.resize_hit_radius {
                    return HitTarget::ResizeHandle(node);
                }
            }
        }
        HitTarget::Nothing
    }

    fn pick_cards(ctx: &HitTestContext<'_>, pos: Vec2, nodes: &[NodeId]) -> HitTarget {
        for &node in nodes {
            if let Some(rect) = (ctx.node_rect)(node)
                && rect.contains(pos)
            {
                return HitTarget::Node(node);
            }
        }
        HitTarget::Nothing
    }

    fn pick_edges(ctx: &HitTestContext<'_>, pos: Vec2, nodes: &[NodeId]) -> HitTarget {
        for &node in nodes {
            let Some(n) = ctx.graph.get_node(node) else {
                continue;
            };
            for i in 0..n.outputs().len() {
                let source = OutputSocketId::new(node, i as u32);
                for target in ctx.graph.downstream(source) {
                    if let Some(path) = (ctx.edge_path)(source, target)
                        && path.distance_to(pos) < ctx.edge_hit_radius
                    {
                        return HitTarget::Edge(EdgeId { source, target });
                    }
                }
            }
        }
        HitTarget::Nothing
    }
}

impl HitTestPolicy for DefaultHitTestPolicy {
    fn hit_test(&self, ctx: &HitTestContext<'_>, pos: Vec2) -> HitTarget {
        let nodes = Self::ordered_nodes(ctx);
        // Priority chain: each picker returns Nothing on a miss.
        for pick in [
            Self::pick_outputs,
            Self::pick_inputs,
            Self::pick_resize_grips,
            Self::pick_cards,
            Self::pick_edges,
        ] {
            let hit = pick(ctx, pos, &nodes);
            if !matches!(hit, HitTarget::Nothing) {
                return hit;
            }
        }
        HitTarget::Nothing
    }
}

/// Default picking radii used by the editor when constructing a context.
pub const DEFAULT_SOCKET_HIT_RADIUS: f32 = SOCKET_HIT_RADIUS;
pub const DEFAULT_RESIZE_HIT_RADIUS: f32 = RESIZE_HIT_RADIUS;

#[cfg(test)]
mod tests {
    use super::*;
    use ze_node_core::SimpleGraph;

    use crate::socket_layout::{DefaultSocketLayout, SocketLayout};
    use crate::test_util::TestNode;

    /// Minimal harness wiring a graph + layout + socket geometry to a context.
    struct Harness {
        graph: SimpleGraph,
        positions: std::collections::HashMap<NodeId, Vec2>,
        sizes: std::collections::HashMap<NodeId, Vec2>,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                graph: SimpleGraph::default(),
                positions: std::collections::HashMap::new(),
                sizes: std::collections::HashMap::new(),
            }
        }

        fn add(&mut self, inputs: u32, outputs: u32, at: Vec2, size: Vec2) -> NodeId {
            let id = self.graph.add_node(Box::new(TestNode::new(
                "f32",
                inputs as usize,
                outputs as usize,
            )));
            self.positions.insert(id, at);
            self.sizes.insert(id, size);
            id
        }

        fn node_rect(&self, node: NodeId) -> Option<Rect> {
            let min = *self.positions.get(&node)?;
            let size = *self.sizes.get(&node)?;
            Some(Rect::from_min_size(min, size))
        }

        fn socket_pos(
            &self,
            layout: &DefaultSocketLayout,
            node: NodeId,
            idx: u32,
            output: bool,
        ) -> Option<Vec2> {
            let rect = self.node_rect(node)?;
            let n = if output {
                self.graph.get_node(node)?.outputs().len() as u32
            } else {
                self.graph.get_node(node)?.inputs().len() as u32
            };
            let off = if output {
                layout.output_offset(idx, n, rect.size())
            } else {
                layout.input_offset(idx, n, rect.size())
            };
            Some(rect.min + off)
        }

        fn policy(&self) -> DefaultHitTestPolicy {
            DefaultHitTestPolicy::new()
        }
    }

    #[test]
    fn resize_grip_hit_wins_over_card_body() {
        let mut h = Harness::new();
        let id = h.add(1, 1, Vec2::new(0.0, 0.0), Vec2::new(160.0, 120.0));
        let layout = DefaultSocketLayout::default();
        let rect = h.node_rect(id).unwrap();

        let ctx = HitTestContext {
            graph: &h.graph,
            socket_hit_radius: DEFAULT_SOCKET_HIT_RADIUS,
            edge_hit_radius: DEFAULT_SOCKET_HIT_RADIUS,
            resize_hit_radius: DEFAULT_RESIZE_HIT_RADIUS,
            node_rect: &|n| h.node_rect(n),
            input_socket_pos: &|s| h.socket_pos(&layout, s.node(), s.index(), false),
            output_socket_pos: &|s| h.socket_pos(&layout, s.node(), s.index(), true),
            edge_path: &|_, _| None,
        };

        // Exactly on the corner → grip.
        assert_eq!(
            h.policy().hit_test(&ctx, rect.max),
            HitTarget::ResizeHandle(id)
        );
        // A few pixels off the corner but inside radius → grip.
        assert_eq!(
            h.policy().hit_test(&ctx, rect.max - Vec2::new(8.0, 4.0)),
            HitTarget::ResizeHandle(id)
        );
        // Mid card → plain node hit.
        assert_eq!(
            h.policy().hit_test(&ctx, rect.center()),
            HitTarget::Node(id)
        );
    }

    #[test]
    fn non_resizable_node_has_no_grip() {
        let mut h = Harness::new();
        // Node whose metadata disables resize.
        let node = TestNode::new("f32", 1, 1)
            .with_interaction(ze_node_core::NodeInteraction::default().resizable(false));
        let id = h.graph.add_node(Box::new(node));
        h.positions.insert(id, Vec2::new(0.0, 0.0));
        h.sizes.insert(id, Vec2::new(160.0, 120.0));
        let layout = DefaultSocketLayout::default();
        let rect = h.node_rect(id).unwrap();

        let ctx = HitTestContext {
            graph: &h.graph,
            socket_hit_radius: DEFAULT_SOCKET_HIT_RADIUS,
            edge_hit_radius: DEFAULT_SOCKET_HIT_RADIUS,
            resize_hit_radius: DEFAULT_RESIZE_HIT_RADIUS,
            node_rect: &|n| h.node_rect(n),
            input_socket_pos: &|s| h.socket_pos(&layout, s.node(), s.index(), false),
            output_socket_pos: &|s| h.socket_pos(&layout, s.node(), s.index(), true),
            edge_path: &|_, _| None,
        };

        // Corner falls through to a card hit.
        assert_eq!(h.policy().hit_test(&ctx, rect.max), HitTarget::Node(id));
    }

    #[test]
    fn sockets_take_priority_over_grips_and_cards() {
        // Pins sit on the card edges; verify the chain order resolves to the
        // socket pick (the highest-priority picker) rather than a card hit.
        let mut h = Harness::new();
        let id = h.add(1, 1, Vec2::new(0.0, 0.0), Vec2::new(160.0, 120.0));
        let layout = DefaultSocketLayout::default();
        let pin = h.socket_pos(&layout, id, 0, true).unwrap();

        let ctx = HitTestContext {
            graph: &h.graph,
            socket_hit_radius: DEFAULT_SOCKET_HIT_RADIUS,
            edge_hit_radius: DEFAULT_SOCKET_HIT_RADIUS,
            resize_hit_radius: DEFAULT_RESIZE_HIT_RADIUS,
            node_rect: &|n| h.node_rect(n),
            input_socket_pos: &|s| h.socket_pos(&layout, s.node(), s.index(), false),
            output_socket_pos: &|s| h.socket_pos(&layout, s.node(), s.index(), true),
            edge_path: &|_, _| None,
        };
        assert_eq!(
            h.policy().hit_test(&ctx, pin),
            HitTarget::OutputSocket(OutputSocketId::new(id, 0))
        );
    }
}
