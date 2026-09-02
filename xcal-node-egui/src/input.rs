//! Translates egui pointer/keyboard input into editor `Command`s.
//!
//! State machine:
//! - press: socket → connection drag; card header → select + node drag;
//!   middle/right button → canvas pan; empty area → clear selection
//! - motion: apply MoveNode / UpdateConnectionDrag / adjust canvas pan
//! - release: finish or cancel the pending connection

use glam::Vec2;
use xcal_node::{EdgeId, NodeId};
use xcal_node_editor::{Command, Editor, HitTarget};

use crate::canvas::GraphCanvas;

/// Header band height; matches `GraphPainter` /
/// `DefaultSocketLayout::default().header_height`.
const HEADER_HEIGHT: f32 = 24.0;

#[derive(Debug, Clone, Copy)]
enum DragState {
    /// Dragging a card by its header.
    Node { node: NodeId, grab_offset: Vec2 },
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
                HitTarget::Node(id) => {
                    let additive = ui.input(|i| i.modifiers.shift);
                    let _ = editor.apply(Command::Select { node: id, additive });
                    // Only the header band starts a drag; body clicks fall
                    // through to the card's body widgets.
                    if let Some(rect) = editor.node_rect(id)
                        && graph_pos.y <= rect.min.y + HEADER_HEIGHT
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

        // --- active drag motion ---
        if ui.input(|i| i.pointer.any_down()) {
            match self.drag {
                Some(DragState::Node { node, grab_offset }) => {
                    let _ = editor.apply(Command::MoveNode {
                        node,
                        to: graph_pos - grab_offset,
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
                match editor.hit_test(graph_pos) {
                    HitTarget::InputSocket(target) => {
                        let _ = editor.apply(Command::CompleteConnection(target));
                    }
                    _ => {
                        let _ = editor.apply(Command::CancelConnection);
                    }
                }
            }
            self.drag = None;
        }
    }
}
