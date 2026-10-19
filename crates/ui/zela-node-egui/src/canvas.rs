//! Viewport transform: pan/zoom lives exclusively in the UI layer.
//!
//! Screen coordinates are egui points relative to the whole window; graph
//! coordinates are the editor's local units (1 graph unit = 1 pixel at
//! zoom 1.0). The transform is:
//!
//! ```text
//! screen = viewport.min + pan + graph * zoom
//! ```

use egui::Pos2;
use glam::Vec2;
use zela_node_editor::Editor;

/// Zoom clamp range.
pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 4.0;

/// Pan/zoom state plus the on-screen rect the canvas occupies.
#[derive(Debug, Clone, Copy)]
pub struct GraphCanvas {
    /// Offset of graph origin from the viewport's top-left corner.
    pub pan: Vec2,
    pub zoom: f32,
    viewport: egui::Rect,
}

impl Default for GraphCanvas {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
            viewport: egui::Rect::NOTHING,
        }
    }
}

impl GraphCanvas {
    pub fn new() -> Self {
        Self::default()
    }

    /// The screen rect the canvas currently occupies.
    pub fn viewport(&self) -> egui::Rect {
        self.viewport
    }

    /// Screen point → graph-local point.
    pub fn to_graph(&self, screen: Pos2) -> Vec2 {
        Vec2::new(
            (screen.x - self.viewport.min.x - self.pan.x) / self.zoom,
            (screen.y - self.viewport.min.y - self.pan.y) / self.zoom,
        )
    }

    /// Graph-local point → screen point.
    pub fn to_screen(&self, graph: Vec2) -> Pos2 {
        Pos2::new(
            self.viewport.min.x + self.pan.x + graph.x * self.zoom,
            self.viewport.min.y + self.pan.y + graph.y * self.zoom,
        )
    }

    /// Per-frame update: refresh the viewport rect and apply
    /// pointer-anchored wheel zoom.
    pub fn handle_input(&mut self, ui: &egui::Ui) {
        self.viewport = ui.max_rect();

        let (scroll, pointer) = ui.input(|i| (i.smooth_scroll_delta.y, i.pointer.latest_pos()));
        if scroll.abs() > f32::EPSILON
            && let Some(pos) = pointer
            && self.viewport.contains(pos)
        {
            let anchor_before = self.to_graph(pos);
            let factor = 1.0 + scroll * 0.01;
            let new_zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
            self.zoom = new_zoom;
            // Keep the graph point under the pointer fixed on screen.
            self.pan.x = pos.x - self.viewport.min.x - anchor_before.x * new_zoom;
            self.pan.y = pos.y - self.viewport.min.y - anchor_before.y * new_zoom;
        }
    }

    /// Center and scale so every node fits inside the viewport.
    pub fn fit_to_content(&mut self, editor: &Editor) {
        if self.viewport.width() <= 0.0 || self.viewport.height() <= 0.0 {
            return;
        }
        let mut min = Vec2::splat(f32::MAX);
        let mut max = Vec2::splat(f32::MIN);
        let mut any = false;
        for id in editor.graph().all_nodes() {
            if let Some(rect) = editor.node_rect(id) {
                any = true;
                min = min.min(rect.min);
                max = max.max(rect.max);
            }
        }
        if !any {
            return;
        }
        let content = (max - min).max(Vec2::splat(1.0));
        let margin = 60.0;
        let avail = Vec2::new(
            (self.viewport.width() - margin * 2.0).max(50.0),
            (self.viewport.height() - margin * 2.0).max(50.0),
        );
        let zoom = (avail.x / content.x)
            .min(avail.y / content.y)
            .clamp(MIN_ZOOM, 1.0);
        self.zoom = zoom;
        let center = (min + max) * 0.5;
        self.pan = Vec2::new(
            self.viewport.width() * 0.5 - center.x * zoom,
            self.viewport.height() * 0.5 - center.y * zoom,
        );
    }
}
