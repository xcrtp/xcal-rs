//! Node card appearance: titles, header colors and per-node body widgets.
//!
//! This trait can only live in the UI layer because [`NodeView::body_ui`]
//! needs `&mut egui::Ui`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use egui::Color32;
use glam::Vec2;
use ze_node_core::Node;

/// How a node type is presented as a card.
pub trait NodeView: Send + Sync + 'static {
    /// Card title text. Defaults to the node's display name.
    fn title(&self, node: &dyn Node) -> String {
        node.meta().display_name.clone()
    }

    /// Header band color (also used as the card's identity color).
    fn header_color(&self, node: &dyn Node) -> Color32;

    /// Parameter-editing region rendered inside the card. The default is an
    /// empty body; demos downcast `node` to concrete types to add widgets.
    fn body_ui(&self, ui: &mut egui::Ui, node: &mut dyn Node) {
        let _ = (ui, node);
    }

    /// Card size estimate in graph-local units (pixels at zoom 1.0).
    fn size_hint(&self, node: &dyn Node) -> Vec2 {
        let _ = node;
        Vec2::new(160.0, 80.0)
    }
}

/// Default view: picks a stable palette color per node type name, so the
/// same type always gets the same color across runs and instances.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultNodeView;

impl DefaultNodeView {
    pub fn new() -> Self {
        Self
    }
}

/// Pleasant, distinguishable header colors.
const PALETTE: [Color32; 8] = [
    Color32::from_rgb(91, 123, 196),  // blue
    Color32::from_rgb(112, 168, 96),  // green
    Color32::from_rgb(176, 112, 196), // purple
    Color32::from_rgb(206, 141, 78),  // orange
    Color32::from_rgb(196, 96, 110),  // red
    Color32::from_rgb(86, 168, 168),  // teal
    Color32::from_rgb(196, 178, 92),  // olive
    Color32::from_rgb(130, 130, 150), // gray-blue
];

fn stable_palette_index(text: &str) -> usize {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    (hasher.finish() as usize) % PALETTE.len()
}

impl NodeView for DefaultNodeView {
    fn header_color(&self, node: &dyn Node) -> Color32 {
        PALETTE[stable_palette_index(&node.meta().type_name)]
    }
}
