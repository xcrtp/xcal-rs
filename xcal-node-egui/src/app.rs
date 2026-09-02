//! Per-frame driver tying together viewport, input handling and painting.

use glam::Vec2;
use xcal_node_editor::{Editor, EditorEvent};

use crate::canvas::GraphCanvas;
use crate::edge_render::EdgeRenderer;
use crate::input::InputHandler;
use crate::painter::GraphPainter;
use crate::view::NodeView;

/// Owns everything needed to render and interact with a node graph inside a
/// host egui `Ui`.
pub struct NodeGraphApp {
    editor: Editor,
    canvas: GraphCanvas,
    input: InputHandler,
    view: Box<dyn NodeView>,
    edge_renderer: Box<dyn EdgeRenderer>,
    /// Fit the content on the first frame (and after [`Self::request_fit`]).
    auto_fit: bool,
}

impl NodeGraphApp {
    pub fn new(
        editor: Editor,
        view: Box<dyn NodeView>,
        edge_renderer: Box<dyn EdgeRenderer>,
    ) -> Self {
        Self {
            editor,
            canvas: GraphCanvas::new(),
            input: InputHandler::new(),
            view,
            edge_renderer,
            auto_fit: true,
        }
    }

    /// Render one frame into `ui`. Returns the editor events emitted while
    /// handling input this frame (the host may ignore them).
    pub fn show(&mut self, ui: &mut egui::Ui) -> Vec<EditorEvent> {
        self.canvas.handle_input(ui);
        if self.auto_fit && self.canvas.viewport().width() > 0.0 {
            self.canvas.fit_to_content(&self.editor);
            self.auto_fit = false;
        }
        self.input.handle(ui, &mut self.canvas, &mut self.editor);

        let painter = GraphPainter::new(self.view.as_ref(), self.edge_renderer.as_ref());
        let canvas = &self.canvas;
        painter.paint(ui, &mut self.editor, |p| canvas.to_screen(p));

        self.editor.drain_events().collect()
    }

    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    pub fn canvas_mut(&mut self) -> &mut GraphCanvas {
        &mut self.canvas
    }

    /// Pops a right-click-on-empty-canvas request (graph-local position).
    pub fn take_context_menu_pos(&mut self) -> Option<Vec2> {
        self.input.take_context_menu_pos()
    }

    /// Re-arm the one-shot content fit (e.g. for a "reset view" button).
    pub fn request_fit(&mut self) {
        self.auto_fit = true;
    }
}
