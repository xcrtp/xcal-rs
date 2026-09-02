//! The [`Editor`] state machine: graph, layout, edge geometry, selection and
//! the in-flight connection, driven exclusively by [`Command`]s and observable
//! through [`EditorEvent`]s.
//!
//! All coordinates in and out of this module are graph-local (`glam::Vec2`).
//! The editor has no notion of pan/zoom.

use std::collections::{HashSet, VecDeque};

use glam::Vec2;
use xcal_node::{
    ConnectError, EdgeId, Graph, InputSocketId, NodeId, OutputSocketId, SimpleGraph, UnitEdge,
};

use crate::command::Command;
use crate::edge_geom::{EdgeGeometry, EdgePath};
use crate::error::EditorError;
use crate::event::EditorEvent;
use crate::geometry::{HitTarget, Rect, SOCKET_HIT_RADIUS};
use crate::layout::{LayoutStrategy, NodeSizeMap};
use crate::socket_layout::{DefaultSocketLayout, SocketLayout};
use crate::undo::UndoStack;
use crate::validation::ConnectionValidator;

/// Default card size when the UI layer has not measured a node yet.
pub const DEFAULT_NODE_SIZE: Vec2 = Vec2::new(160.0, 80.0);

/// A connection currently being dragged from an output socket.
#[derive(Debug, Clone, Copy)]
pub struct PendingConnection {
    pub from: OutputSocketId,
    pub cursor: Vec2,
    /// Legal drop target under the cursor (computed during validation, stage
    /// 6); used by the UI to highlight valid inputs.
    pub hover_target: Option<InputSocketId>,
}

/// The node editor state machine.
pub struct Editor {
    graph: Box<dyn Graph>,
    layout: Box<dyn LayoutStrategy>,
    edge_geom: Box<dyn EdgeGeometry>,
    socket_layout: Box<dyn SocketLayout>,
    sizes: NodeSizeMap,
    selection: HashSet<NodeId>,
    pending: Option<PendingConnection>,
    events: VecDeque<EditorEvent>,
    undo_stack: UndoStack,
    /// Source sockets parallel to the undo stack's redo entries: a replayed
    /// `CompleteConnection` has no live drag, so its source is carried here.
    redo_source_hint: Vec<Option<OutputSocketId>>,
}

impl Editor {
    pub fn new(
        graph: Box<dyn Graph>,
        layout: Box<dyn LayoutStrategy>,
        edge_geom: Box<dyn EdgeGeometry>,
    ) -> Self {
        Self {
            graph,
            layout,
            edge_geom,
            socket_layout: Box::new(DefaultSocketLayout::default()),
            sizes: NodeSizeMap::new(),
            selection: HashSet::new(),
            pending: None,
            events: VecDeque::new(),
            undo_stack: UndoStack::new(),
            redo_source_hint: Vec::new(),
        }
    }

    // --- accessors ---

    pub fn graph(&self) -> &dyn Graph {
        self.graph.as_ref()
    }

    pub fn graph_mut(&mut self) -> &mut dyn Graph {
        self.graph.as_mut()
    }

    pub fn layout(&self) -> &dyn LayoutStrategy {
        self.layout.as_ref()
    }

    pub fn edge_geom(&self) -> &dyn EdgeGeometry {
        self.edge_geom.as_ref()
    }

    pub fn socket_layout(&self) -> &dyn SocketLayout {
        self.socket_layout.as_ref()
    }

    pub fn selection(&self) -> &HashSet<NodeId> {
        &self.selection
    }

    pub fn pending(&self) -> Option<&PendingConnection> {
        self.pending.as_ref()
    }

    /// Measured node size map (UI layer writes through `SetNodeSize`).
    pub fn sizes(&self) -> &NodeSizeMap {
        &self.sizes
    }

    /// The single write entry point: apply a command, emit events and record
    /// the inverse on the undo stack (clearing redo on new actions).
    pub fn apply(&mut self, cmd: Command) -> Result<(), EditorError> {
        let inverse = self.execute(cmd)?;
        if let Some(inv) = inverse {
            self.undo_stack.push(inv);
            self.undo_stack.clear_redo();
            self.redo_source_hint.clear();
        }
        Ok(())
    }

    /// Undo the most recent history-worthy action.
    pub fn undo(&mut self) -> Result<(), EditorError> {
        let Some(cmd) = self.undo_stack.undo() else {
            return Ok(());
        };
        // Undoing a connect replays a `Disconnect(edge)`; its source feeds
        // the redo-side CompleteConnection (no live drag available).
        let connect_source = match &cmd {
            Command::Disconnect(edge) => Some(edge.source),
            _ => None,
        };
        let inverse = self.execute(cmd)?;
        if let Some(inv) = inverse {
            let hint = match &inv {
                Command::CompleteConnection(_) => connect_source,
                _ => None,
            };
            self.undo_stack.push_redo(inv);
            self.redo_source_hint.push(hint);
        }
        Ok(())
    }

    /// Redo the most recently undone action.
    pub fn redo(&mut self) -> Result<(), EditorError> {
        let Some(cmd) = self.undo_stack.redo() else {
            return Ok(());
        };
        let hint = self.redo_source_hint.pop().flatten();
        // Replaying a connection has no live drag: re-arm pending with the
        // original source before executing CompleteConnection.
        if let Command::CompleteConnection(to) = &cmd
            && self.pending.is_none()
            && let Some(from) = hint
        {
            self.pending = Some(PendingConnection {
                from,
                cursor: self.input_socket_pos(*to).unwrap_or(Vec2::ZERO),
                hover_target: None,
            });
        }
        let inverse = self.execute(cmd)?;
        if let Some(inv) = inverse {
            self.undo_stack.push(inv);
        }
        Ok(())
    }

    pub fn can_undo(&self) -> bool {
        self.undo_stack.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.undo_stack.can_redo()
    }

    /// Execute a command and return its inverse command (if the command is
    /// undoable). Connection failures are reported as `ConnectionRejected`
    /// events rather than errors so the UI never has to panic.
    fn execute(&mut self, cmd: Command) -> Result<Option<Command>, EditorError> {
        match cmd {
            // TODO(阶段10): 节点增删撤销需快照 Box<dyn Node> 并处理 NodeIdRemapped
            Command::AddNode { node, at } => {
                let id = self.graph.add_node(node);
                self.layout.set_position(id, at);
                self.events.push_back(EditorEvent::NodeAdded(id));
                Ok(None)
            }
            Command::RemoveNode(node) => {
                if self.graph.remove_node(node).is_some() {
                    self.layout.remove(node);
                    self.sizes.remove(&node);
                    self.selection.remove(&node);
                    self.events.push_back(EditorEvent::NodeRemoved(node));
                }
                Ok(None)
            }
            Command::MoveNode { node, to } => {
                let from = self.layout.position_of(node);
                self.layout.set_position(node, to);
                self.events.push_back(EditorEvent::NodeMoved { node, to });
                // Only record when the position actually changed.
                Ok(from
                    .filter(|p| *p != to)
                    .map(|p| Command::MoveNode { node, to: p }))
            }
            Command::SetNodeSize { node, size } => {
                let old = self.sizes.get(&node).copied();
                self.sizes.insert(node, size);
                Ok(old
                    .filter(|p| *p != size)
                    .map(|p| Command::SetNodeSize { node, size: p }))
            }
            Command::BeginConnection(source) => {
                let exists = self
                    .graph
                    .get_node(source.node())
                    .map(|n| (source.index() as usize) < n.outputs().len())
                    .unwrap_or(false);
                if !exists {
                    return Err(EditorError::NodeNotFound(source.node()));
                }
                self.pending = Some(PendingConnection {
                    from: source,
                    cursor: self.output_socket_pos(source).unwrap_or(Vec2::ZERO),
                    hover_target: None,
                });
                Ok(None)
            }
            Command::UpdateConnectionDrag(pos) => {
                if let Some(pending) = self.pending.as_mut() {
                    pending.cursor = pos;
                }
                // Highlight a legal drop target under the cursor.
                let from = self.pending.map(|p| p.from);
                let hover = from.and_then(|src| {
                    if let HitTarget::InputSocket(id) = self.hit_test(pos) {
                        ConnectionValidator::check_types(self.graph.as_ref(), src, id)
                            .ok()
                            .map(|_| id)
                    } else {
                        None
                    }
                });
                if let Some(pending) = self.pending.as_mut() {
                    pending.hover_target = hover;
                }
                Ok(None)
            }
            Command::CompleteConnection(target) => {
                let from = self
                    .pending
                    .take()
                    .ok_or(EditorError::NoPendingConnection)?
                    .from;
                self.connect_sockets(from, target)
            }
            Command::CancelConnection => {
                self.pending = None;
                Ok(None)
            }
            Command::Disconnect(edge) => {
                if self.graph.disconnect(edge.source, edge.target).is_some() {
                    self.events.push_back(EditorEvent::Disconnected(edge));
                    // Inverse replays CompleteConnection(edge.target); the
                    // source is re-armed by the undo/redo driver.
                    Ok(Some(Command::CompleteConnection(edge.target)))
                } else {
                    Ok(None)
                }
            }
            Command::Select { node, additive } => {
                if !additive {
                    self.selection.clear();
                }
                self.selection.insert(node);
                self.events.push_back(EditorEvent::SelectionChanged);
                Ok(None)
            }
            Command::ClearSelection => {
                self.selection.clear();
                self.events.push_back(EditorEvent::SelectionChanged);
                Ok(None)
            }
            Command::SetLayout(strategy) => {
                self.layout = strategy;
                self.events.push_back(EditorEvent::LayoutChanged);
                Ok(None)
            }
            Command::SetEdgeGeometry(geom) => {
                self.edge_geom = geom;
                self.events.push_back(EditorEvent::EdgeGeometryChanged);
                Ok(None)
            }
            Command::RecomputeLayout => {
                self.layout.recompute(self.graph.as_ref(), &self.sizes);
                self.events.push_back(EditorEvent::LayoutChanged);
                Ok(None)
            }
        }
    }

    /// Validate (type + cycle + graph rules) and commit a connection. Any
    /// failure emits `ConnectionRejected` and leaves the graph untouched.
    fn connect_sockets(
        &mut self,
        from: OutputSocketId,
        to: InputSocketId,
    ) -> Result<Option<Command>, EditorError> {
        let reject = |reason: ConnectError| EditorEvent::ConnectionRejected { from, to, reason };

        if let Err(reason) = ConnectionValidator::check_types(self.graph.as_ref(), from, to) {
            self.events.push_back(reject(reason));
            return Ok(None);
        }
        if let Err(reason) = ConnectionValidator::check_no_cycle(self.graph.as_mut(), from, to) {
            self.events.push_back(reject(reason));
            return Ok(None);
        }
        if let Err(reason) = self.graph.connect_with_data(from, to, Box::new(UnitEdge)) {
            self.events.push_back(reject(reason));
            return Ok(None);
        }
        let edge = EdgeId {
            source: from,
            target: to,
        };
        self.events.push_back(EditorEvent::Connected(edge));
        Ok(Some(Command::Disconnect(edge)))
    }

    /// Drain queued events (typically once per UI frame).
    pub fn drain_events(&mut self) -> impl Iterator<Item = EditorEvent> + '_ {
        self.events.drain(..)
    }

    // --- geometry queries (graph-local) ---

    /// Card rectangle for a node: layout position + measured (or default)
    /// size.
    pub fn node_rect(&self, node: NodeId) -> Option<Rect> {
        let min = self.layout.position_of(node)?;
        let size = self.sizes.get(&node).copied().unwrap_or(DEFAULT_NODE_SIZE);
        Some(Rect::from_min_size(min, size))
    }

    /// Absolute graph-local position of an input socket.
    pub fn input_socket_pos(&self, socket: InputSocketId) -> Option<Vec2> {
        let node = self.graph.get_node(socket.node())?;
        let rect = self.node_rect(socket.node())?;
        let count = node.inputs().len() as u32;
        if socket.index() >= count {
            return None;
        }
        Some(
            rect.min
                + self
                    .socket_layout
                    .input_offset(socket.index(), count, rect.size()),
        )
    }

    /// Absolute graph-local position of an output socket.
    pub fn output_socket_pos(&self, socket: OutputSocketId) -> Option<Vec2> {
        let node = self.graph.get_node(socket.node())?;
        let rect = self.node_rect(socket.node())?;
        let count = node.outputs().len() as u32;
        if socket.index() >= count {
            return None;
        }
        Some(
            rect.min
                + self
                    .socket_layout
                    .output_offset(socket.index(), count, rect.size()),
        )
    }

    /// Path of the edge between two sockets, if both exist.
    pub fn connection_path(
        &self,
        source: OutputSocketId,
        target: InputSocketId,
    ) -> Option<EdgePath> {
        let from = self.output_socket_pos(source)?;
        let to = self.input_socket_pos(target)?;
        Some(self.edge_geom.build_path(from, to))
    }

    /// All edges in the graph. Socket ids are constructed from
    /// `(node, enumerate-index)`: descriptor `node`/`index` fields are
    /// advisory and never trusted for identity.
    pub fn all_edges(&self) -> Vec<EdgeId> {
        let mut edges = Vec::new();
        for node_id in self.graph.all_nodes() {
            let Some(node) = self.graph.get_node(node_id) else {
                continue;
            };
            for i in 0..node.outputs().len() {
                let src = OutputSocketId::new(node_id, i as u32);
                for tgt in self.graph.downstream(src) {
                    edges.push(EdgeId {
                        source: src,
                        target: tgt,
                    });
                }
            }
        }
        edges.sort_unstable_by_key(|e| (e.source, e.target));
        edges.dedup();
        edges
    }

    /// Path of the in-flight dragged connection, if any.
    pub fn pending_path(&self) -> Option<EdgePath> {
        let pending = self.pending.as_ref()?;
        let from = self.output_socket_pos(pending.from)?;
        Some(self.edge_geom.build_path(from, pending.cursor))
    }

    /// Hit test in graph-local coordinates. Priority (high to low):
    /// output sockets → input sockets → node cards → edges.
    pub fn hit_test(&self, pos: Vec2) -> HitTarget {
        // Newest node on top: iterate ids in descending order.
        let mut nodes: Vec<NodeId> = self.graph.all_nodes().collect();
        nodes.sort_unstable_by(|a, b| b.cmp(a));

        for &node in &nodes {
            let Some(n) = self.graph.get_node(node) else {
                continue;
            };
            for i in 0..n.outputs().len() {
                let id = OutputSocketId::new(node, i as u32);
                if let Some(p) = self.output_socket_pos(id)
                    && (p - pos).length() <= SOCKET_HIT_RADIUS
                {
                    return HitTarget::OutputSocket(id);
                }
            }
        }

        for &node in &nodes {
            let Some(n) = self.graph.get_node(node) else {
                continue;
            };
            for i in 0..n.inputs().len() {
                let id = InputSocketId::new(node, i as u32);
                if let Some(p) = self.input_socket_pos(id)
                    && (p - pos).length() <= SOCKET_HIT_RADIUS
                {
                    return HitTarget::InputSocket(id);
                }
            }
        }

        for &node in &nodes {
            if let Some(rect) = self.node_rect(node)
                && rect.contains(pos)
            {
                return HitTarget::Node(node);
            }
        }

        for edge in self.all_edges() {
            if let Some(path) = self.connection_path(edge.source, edge.target)
                && path.distance_to(pos) < SOCKET_HIT_RADIUS
            {
                return HitTarget::Edge(edge);
            }
        }

        HitTarget::Nothing
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new(
            Box::new(SimpleGraph::default()),
            Box::new(crate::layout::FreeLayout::new()),
            Box::new(crate::edge_geom::BezierGeometry::default()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TestNode;

    fn make_editor() -> Editor {
        Editor::new(
            Box::new(SimpleGraph::default()),
            Box::new(crate::layout::FreeLayout::new()),
            Box::new(crate::edge_geom::BezierGeometry::default()),
        )
    }

    fn add_node(editor: &mut Editor, at: Vec2) -> NodeId {
        editor
            .apply(Command::AddNode {
                node: Box::new(TestNode::f32_passthrough()),
                at,
            })
            .unwrap();
        editor
            .drain_events()
            .find_map(|e| match e {
                EditorEvent::NodeAdded(id) => Some(id),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn new_editor_has_no_edges() {
        let editor = make_editor();
        assert!(editor.all_edges().is_empty());
        assert_eq!(editor.graph().node_count(), 0);
    }

    #[test]
    fn add_node_emits_event_and_registers_in_graph() {
        let mut editor = make_editor();
        editor
            .apply(Command::AddNode {
                node: Box::new(TestNode::f32_passthrough()),
                at: Vec2::new(10.0, 20.0),
            })
            .unwrap();
        assert_eq!(editor.graph().node_count(), 1);
        let events: Vec<_> = editor.drain_events().collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, EditorEvent::NodeAdded(_)))
        );
    }

    #[test]
    fn move_node_updates_rect_and_emits_event() {
        let mut editor = make_editor();
        let id = add_node(&mut editor, Vec2::new(0.0, 0.0));
        let target = Vec2::new(50.0, 60.0);
        editor
            .apply(Command::MoveNode {
                node: id,
                to: target,
            })
            .unwrap();
        assert_eq!(editor.node_rect(id).unwrap().min, target);
        assert!(editor.drain_events().any(
            |e| matches!(e, EditorEvent::NodeMoved { node, to } if node == id && to == target)
        ));
    }

    #[test]
    fn connect_two_nodes_creates_edge_and_path() {
        let mut editor = make_editor();
        let a = add_node(&mut editor, Vec2::new(0.0, 0.0));
        let b = add_node(&mut editor, Vec2::new(400.0, 0.0));

        let a_out = OutputSocketId::new(a, 0);
        let b_in = InputSocketId::new(b, 0);
        editor.apply(Command::BeginConnection(a_out)).unwrap();
        editor.apply(Command::CompleteConnection(b_in)).unwrap();

        let edges = editor.all_edges();
        assert_eq!(edges.len(), 1);
        assert_eq!(
            edges[0],
            EdgeId {
                source: a_out,
                target: b_in,
            }
        );
        assert!(editor.connection_path(a_out, b_in).is_some());
        assert!(
            editor
                .drain_events()
                .any(|e| matches!(e, EditorEvent::Connected(_)))
        );
    }

    #[test]
    fn hit_test_finds_sockets_node_and_void() {
        let mut editor = make_editor();
        let id = add_node(&mut editor, Vec2::new(100.0, 100.0));

        // Output socket: right edge, first row → (260, 140) with default size.
        let out = OutputSocketId::new(id, 0);
        let out_pos = editor.output_socket_pos(out).unwrap();
        assert_eq!(out_pos, Vec2::new(260.0, 140.0));
        assert_eq!(editor.hit_test(out_pos), HitTarget::OutputSocket(out));

        // Input socket: left edge.
        let inp = InputSocketId::new(id, 0);
        let in_pos = editor.input_socket_pos(inp).unwrap();
        assert_eq!(in_pos, Vec2::new(100.0, 140.0));
        assert_eq!(editor.hit_test(in_pos), HitTarget::InputSocket(inp));

        // Card interior → node.
        assert_eq!(
            editor.hit_test(Vec2::new(180.0, 150.0)),
            HitTarget::Node(id)
        );

        // Empty space → nothing.
        assert_eq!(editor.hit_test(Vec2::new(0.0, 0.0)), HitTarget::Nothing);
    }

    #[test]
    fn pending_path_updates_with_cursor() {
        let mut editor = make_editor();
        let a = add_node(&mut editor, Vec2::new(0.0, 0.0));
        let a_out = OutputSocketId::new(a, 0);
        assert!(editor.pending_path().is_none());
        editor.apply(Command::BeginConnection(a_out)).unwrap();
        editor
            .apply(Command::UpdateConnectionDrag(Vec2::new(500.0, 500.0)))
            .unwrap();
        let path = editor.pending_path().expect("pending path exists");
        match path {
            EdgePath::Bezier { to, .. } => assert_eq!(to, Vec2::new(500.0, 500.0)),
            other => panic!("expected bezier, got {other:?}"),
        }
        editor.apply(Command::CancelConnection).unwrap();
        assert!(editor.pending_path().is_none());
    }

    // ---------- Stage 6: validation + undo/redo ----------

    /// Helper: connect output 0 of `from` to input 0 of `to` via the full
    /// drag command sequence, draining events.
    fn connect(editor: &mut Editor, from: NodeId, to: NodeId) {
        editor
            .apply(Command::BeginConnection(OutputSocketId::new(from, 0)))
            .unwrap();
        editor
            .apply(Command::CompleteConnection(InputSocketId::new(to, 0)))
            .unwrap();
        editor.drain_events().for_each(drop);
    }

    fn rejection_reason(events: &[EditorEvent]) -> Option<&ConnectError> {
        events.iter().find_map(|e| match e {
            EditorEvent::ConnectionRejected { reason, .. } => Some(reason),
            _ => None,
        })
    }

    #[test]
    fn connect_compatible_types_succeeds() {
        let mut editor = make_editor();
        let a = add_node(&mut editor, Vec2::new(0.0, 0.0));
        let b = add_node(&mut editor, Vec2::new(400.0, 0.0));
        connect(&mut editor, a, b);
        assert_eq!(editor.all_edges().len(), 1);
    }

    #[test]
    fn connect_type_mismatch_is_rejected() {
        let mut editor = make_editor();
        // f32 source: 1 input + 1 output.
        let src_id = add_node(&mut editor, Vec2::new(0.0, 0.0));
        // vec2 sink: 1 input, no output.
        editor
            .apply(Command::AddNode {
                node: Box::new(TestNode::new("vec2", 1, 0)),
                at: Vec2::new(400.0, 0.0),
            })
            .unwrap();
        let sink_id = editor
            .drain_events()
            .find_map(|e| match e {
                EditorEvent::NodeAdded(id) => Some(id),
                _ => None,
            })
            .unwrap();

        editor
            .apply(Command::BeginConnection(OutputSocketId::new(src_id, 0)))
            .unwrap();
        editor
            .apply(Command::CompleteConnection(InputSocketId::new(sink_id, 0)))
            .unwrap();

        let events: Vec<_> = editor.drain_events().collect();
        match rejection_reason(&events) {
            Some(ConnectError::TypeIncompatible { from, to }) => {
                assert_eq!(from, "f32");
                assert_eq!(to, "vec2");
            }
            other => panic!("expected TypeIncompatible, got {other:?}"),
        }
        assert!(editor.all_edges().is_empty());
    }

    #[test]
    fn connect_creating_cycle_is_rejected() {
        let mut editor = make_editor();
        let a = add_node(&mut editor, Vec2::new(0.0, 0.0));
        let b = add_node(&mut editor, Vec2::new(300.0, 0.0));
        let c = add_node(&mut editor, Vec2::new(600.0, 0.0));
        connect(&mut editor, a, b);
        connect(&mut editor, b, c);
        assert_eq!(editor.all_edges().len(), 2);

        // c -> a would close the loop a -> b -> c -> a.
        editor
            .apply(Command::BeginConnection(OutputSocketId::new(c, 0)))
            .unwrap();
        editor
            .apply(Command::CompleteConnection(InputSocketId::new(a, 0)))
            .unwrap();
        let events: Vec<_> = editor.drain_events().collect();
        assert!(matches!(
            rejection_reason(&events),
            Some(ConnectError::CycleDetected)
        ));
        assert_eq!(editor.all_edges().len(), 2);
    }

    #[test]
    fn second_connection_into_single_input_is_rejected() {
        let mut editor = make_editor();
        let a = add_node(&mut editor, Vec2::new(0.0, 0.0));
        let b = add_node(&mut editor, Vec2::new(300.0, 0.0));
        let c = add_node(&mut editor, Vec2::new(600.0, 0.0));
        connect(&mut editor, a, b);

        // c -> b: b's input already carries a -> b.
        editor
            .apply(Command::BeginConnection(OutputSocketId::new(c, 0)))
            .unwrap();
        editor
            .apply(Command::CompleteConnection(InputSocketId::new(b, 0)))
            .unwrap();
        let events: Vec<_> = editor.drain_events().collect();
        assert!(matches!(
            rejection_reason(&events),
            Some(ConnectError::InputAlreadyConnected(_))
        ));
        // The original edge must survive the trial rollback.
        let edges = editor.all_edges();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, OutputSocketId::new(a, 0));
        assert_eq!(edges[0].target, InputSocketId::new(b, 0));
    }

    #[test]
    fn drag_hover_highlights_only_compatible_inputs() {
        let mut editor = make_editor();
        let src = add_node(&mut editor, Vec2::new(0.0, 0.0));
        let f32_sink = add_node(&mut editor, Vec2::new(300.0, 200.0));
        editor
            .apply(Command::AddNode {
                node: Box::new(TestNode::new("vec2", 1, 0)),
                at: Vec2::new(0.0, 300.0),
            })
            .unwrap();
        let vec2_sink = editor
            .drain_events()
            .find_map(|e| match e {
                EditorEvent::NodeAdded(id) => Some(id),
                _ => None,
            })
            .unwrap();

        editor
            .apply(Command::BeginConnection(OutputSocketId::new(src, 0)))
            .unwrap();

        // Over the f32 input → hover armed.
        let p = editor
            .input_socket_pos(InputSocketId::new(f32_sink, 0))
            .unwrap();
        editor.apply(Command::UpdateConnectionDrag(p)).unwrap();
        assert_eq!(
            editor.pending().unwrap().hover_target,
            Some(InputSocketId::new(f32_sink, 0))
        );

        // Over the vec2 input → no hover (type mismatch).
        let p = editor
            .input_socket_pos(InputSocketId::new(vec2_sink, 0))
            .unwrap();
        editor.apply(Command::UpdateConnectionDrag(p)).unwrap();
        assert_eq!(editor.pending().unwrap().hover_target, None);
    }

    #[test]
    fn undo_redo_move_node_restores_positions() {
        let mut editor = make_editor();
        let id = add_node(&mut editor, Vec2::new(30.0, 40.0));
        let new_pos = Vec2::new(150.0, 90.0);
        editor
            .apply(Command::MoveNode {
                node: id,
                to: new_pos,
            })
            .unwrap();
        assert_eq!(editor.node_rect(id).unwrap().min, new_pos);
        assert!(editor.can_undo());

        editor.undo().unwrap();
        assert_eq!(editor.node_rect(id).unwrap().min, Vec2::new(30.0, 40.0));
        assert!(editor.can_redo());

        editor.redo().unwrap();
        assert_eq!(editor.node_rect(id).unwrap().min, new_pos);
    }

    #[test]
    fn undo_redo_connection_removes_and_restores_edge() {
        let mut editor = make_editor();
        let a = add_node(&mut editor, Vec2::new(0.0, 0.0));
        let b = add_node(&mut editor, Vec2::new(400.0, 0.0));
        connect(&mut editor, a, b);
        assert_eq!(editor.all_edges().len(), 1);

        editor.undo().unwrap();
        editor.drain_events().for_each(drop);
        assert!(editor.all_edges().is_empty());
        assert!(editor.can_redo());

        editor.redo().unwrap();
        let edges = editor.all_edges();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, OutputSocketId::new(a, 0));
        assert_eq!(edges[0].target, InputSocketId::new(b, 0));
        assert!(
            editor
                .drain_events()
                .any(|e| matches!(e, EditorEvent::Connected(_)))
        );
    }

    #[test]
    fn new_action_clears_redo_stack() {
        let mut editor = make_editor();
        let a = add_node(&mut editor, Vec2::new(0.0, 0.0));
        editor
            .apply(Command::MoveNode {
                node: a,
                to: Vec2::new(10.0, 10.0),
            })
            .unwrap();
        editor.undo().unwrap();
        assert!(editor.can_redo());
        // A new move (not the redone one) invalidates redo history.
        editor
            .apply(Command::MoveNode {
                node: a,
                to: Vec2::new(20.0, 20.0),
            })
            .unwrap();
        assert!(!editor.can_redo());
        assert!(editor.can_undo());
    }
}
