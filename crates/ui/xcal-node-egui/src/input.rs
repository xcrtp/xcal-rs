//! Translates egui pointer/keyboard input into editor `Command`s.
//!
//! State machine:
//! - press: socket → connection drag; resize grip → card resize;
//!   card header → select + node drag (if `interaction.draggable`);
//!   middle/right button → canvas pan; empty area → clear selection
//! - motion: MoveNode / ResizeNode / UpdateConnectionDrag / canvas pan
//! - release: finish or cancel the pending connection
//!
//! Which interactions a node accepts comes from its
//! [`xc_node::NodeMeta::interaction`] metadata (non-draggable cards ignore
//! header drags, non-resizable cards expose no grip); the hit policy itself
//! is pluggable via `Editor::set_hit_policy`.

use glam::Vec2;
use xc_node::{EdgeId, NodeId};
use xcal_node_editor::{Command, Editor, HitTarget};

use crate::canvas::GraphCanvas;

#[derive(Debug, Clone, Copy)]
enum DragState {
    /// Dragging a card by its header.
    Node { node: NodeId, grab_offset: Vec2 },
    /// Dragging the bottom-right resize grip; `anchor` is the card's top-left
    /// corner (fixed for the whole gesture).
    Resize { node: NodeId, anchor: Vec2 },
    /// Dragging a connection out of an output socket.
    Connection,
    /// Panning the canvas with the middle / right mouse button.
    Pan,
}

/// Owns the per-frame pointer interaction state.
#[derive(Default)]
pub struct InputHandler {
    drag: Option<DragState>,
    /// Right-click on empty canvas: graph position the host may turn into a
    /// spawn menu (consumed via [`Self::take_context_menu_pos`]).
    context_menu_at: Option<Vec2>,
}

impl InputHandler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pops a pending right-click-on-empty-canvas request.
    pub fn take_context_menu_pos(&mut self) -> Option<Vec2> {
        self.context_menu_at.take()
    }

    /// Read this frame's input and apply the resulting commands.
    pub fn handle(&mut self, ui: &egui::Ui, canvas: &mut GraphCanvas, editor: &mut Editor) {
        self.handle_keys(ui, editor);
        self.handle_pointer(ui, canvas, editor);
    }

    fn handle_keys(&self, ui: &egui::Ui, editor: &mut Editor) {
        let ctrl = ui.input(|i| i.modifiers.ctrl);
        let shift = ui.input(|i| i.modifiers.shift);

        // Don't delete nodes while the user is typing in a text field.
        let text_focused = ui.memory(|m| m.focused().is_some());
        if !text_focused
            && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
        {
            let selected: Vec<NodeId> = editor.selection().iter().copied().collect();
            for id in selected {
                let _ = editor.apply(Command::RemoveNode(id));
            }
        }

        if ctrl && !shift && ui.input(|i| i.key_pressed(egui::Key::Z)) {
            let _ = editor.undo();
        }
        if ctrl
            && (ui.input(|i| i.key_pressed(egui::Key::Y))
                || shift && ui.input(|i| i.key_pressed(egui::Key::Z)))
        {
            let _ = editor.redo();
        }
    }

    fn handle_pointer(&mut self, ui: &egui::Ui, canvas: &mut GraphCanvas, editor: &mut Editor) {
        let Some(pos) = ui.input(|i| i.pointer.latest_pos()) else {
            return;
        };
        let graph_pos = canvas.to_graph(pos);

        let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
        let pan_pressed = ui.input(|i| {
            i.pointer.secondary_pressed() || i.pointer.button_pressed(egui::PointerButton::Middle)
        });

        // Pan gestures can start anywhere, even outside the viewport.
        if pan_pressed {
            self.drag = Some(DragState::Pan);
            // Right-click on empty canvas requests the spawn menu.
            if ui.input(|i| i.pointer.secondary_pressed())
                && canvas.viewport().contains(pos)
                && matches!(editor.hit_test(graph_pos), HitTarget::Nothing)
            {
                self.context_menu_at = Some(graph_pos);
            }
        }

        // Left-button presses only start interactions inside the canvas.
        if primary_pressed && canvas.viewport().contains(pos) {
            match editor.hit_test(graph_pos) {
                HitTarget::OutputSocket(id) => {
                    if editor.apply(Command::BeginConnection(id)).is_ok() {
                        self.drag = Some(DragState::Connection);
                    }
                }
                HitTarget::InputSocket(id) => {
                    // Grab the existing incoming edge and re-drag it.
                    if let Some(source) = editor.graph().upstream(id).pop() {
                        let edge = EdgeId { source, target: id };
                        let _ = editor.apply(Command::Disconnect(edge));
                        if editor.apply(Command::BeginConnection(source)).is_ok() {
                            self.drag = Some(DragState::Connection);
                        }
                    }
                }
                HitTarget::ResizeHandle(id) => {
                    // Grab the resize grip: select the card and start a resize
                    // gesture. The grip only exists for interaction-resizable
                    // nodes (the hit policy already filtered them), this is
                    // the defense-in-depth check.
                    let resizable = editor
                        .graph()
                        .get_node(id)
                        .is_some_and(|n| n.meta().interaction.resizable);
                    if resizable && let Some(rect) = editor.node_rect(id) {
                        let additive = ui.input(|i| i.modifiers.shift);
                        let _ = editor.apply(Command::Select { node: id, additive });
                        self.drag = Some(DragState::Resize {
                            node: id,
                            anchor: rect.min,
                        });
                    }
                }
                HitTarget::Node(id) => {
                    let interaction = editor
                        .graph()
                        .get_node(id)
                        .map(|n| n.meta().interaction.clone());
                    let selectable = interaction.as_ref().is_none_or(|i| i.selectable);
                    let draggable = interaction.as_ref().is_none_or(|i| i.draggable);
                    if selectable {
                        let additive = ui.input(|i| i.modifiers.shift);
                        let _ = editor.apply(Command::Select { node: id, additive });
                    }
                    // Only the header band of a draggable card starts a move;
                    // body clicks fall through to the card's body widgets.
                    let header_height = editor.socket_layout().header_height();
                    if draggable
                        && let Some(rect) = editor.node_rect(id)
                        && graph_pos.y <= rect.min.y + header_height
                    {
                        self.drag = Some(DragState::Node {
                            node: id,
                            grab_offset: graph_pos - rect.min,
                        });
                    }
                }
                HitTarget::Edge(_) => {
                    // Edge click-to-delete is deferred to stage 10.
                }
                HitTarget::Nothing => {
                    let _ = editor.apply(Command::ClearSelection);
                }
            }
        }

        // --- hover cursor feedback (no button pressed) ---
        if !ui.input(|i| i.pointer.any_down())
            && canvas.viewport().contains(pos)
            && matches!(editor.hit_test(graph_pos), HitTarget::ResizeHandle(_))
        {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeNwSe);
        }

        // --- active drag motion ---
        if ui.input(|i| i.pointer.any_down()) {
            match self.drag {
                Some(DragState::Node { node, grab_offset }) => {
                    let _ = editor.apply(Command::MoveNode {
                        node,
                        to: graph_pos - grab_offset,
                    });
                }
                Some(DragState::Resize { node, anchor }) => {
                    // Card's top-left stays anchored; size tracks the pointer.
                    // The editor clamps to the node's min/max bounds.
                    let _ = editor.apply(Command::ResizeNode {
                        node,
                        size: graph_pos - anchor,
                    });
                }
                Some(DragState::Connection) => {
                    let _ = editor.apply(Command::UpdateConnectionDrag(graph_pos));
                }
                Some(DragState::Pan) => {
                    let delta = ui.input(|i| i.pointer.delta());
                    canvas.pan += Vec2::new(delta.x, delta.y);
                }
                None => {}
            }
        }

        // --- release ---
        let released = ui.input(|i| {
            i.pointer.primary_released()
                || i.pointer.secondary_released()
                || i.pointer.button_released(egui::PointerButton::Middle)
        });
        if released {
            if matches!(self.drag, Some(DragState::Connection)) && canvas.viewport().contains(pos) {
                // Prefer the magnetic snap target (the pin the wire is locked
                // onto); fall back to a direct hit test for a precise drop.
                let target = editor
                    .pending()
                    .and_then(|p| p.hover_target)
                    .or_else(|| match editor.hit_test(graph_pos) {
                        HitTarget::InputSocket(id) => Some(id),
                        _ => None,
                    });
                match target {
                    Some(target) => {
                        let _ = editor.apply(Command::CompleteConnection(target));
                    }
                    None => {
                        let _ = editor.apply(Command::CancelConnection);
                    }
                }
            }
            self.drag = None;
        }
    }
}
