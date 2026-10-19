//! `PropertyNodeView` — a generic [`NodeView`] that renders every property of
//! any registered `NodeSpec` node via [`draw_property`]. No per-node downcast.

use egui::Color32;
use glam::Vec2;

use ze_node::Node;
use zela_node_egui::NodeView;
use zela_node_egui::socket_layout::{
    CARD_HEADER_HEIGHT, SOCKET_BOTTOM_GAP, SOCKET_ROW_HEIGHT, SOCKET_TOP_MARGIN,
};
use ze_node_schema::NodeRegistry;

use crate::draw::draw_property;

/// Estimated card width (graph units = pixels at zoom 1.0).
const CARD_WIDTH: f32 = 200.0;
/// Vertical space reserved per property widget (slider / checkbox / field).
const PROPERTY_ROW_HEIGHT: f32 = 24.0;
/// Bottom padding under the property-widget band.
const BODY_BOTTOM_PAD: f32 = 8.0;

/// A `NodeView` that auto-renders a node's body from its `NodeSpec::properties()`
/// via the registry. Holds a [`NodeRegistry`] clone (cheap — it's a `Vec` of
/// small entries) so it can reach the `NodeSpec` impl from a `&mut dyn Node`
/// without any caller-side downcast.
pub struct PropertyNodeView {
    pub registry: NodeRegistry,
}

impl PropertyNodeView {
    pub fn new(registry: NodeRegistry) -> Self {
        Self { registry }
    }
}

impl NodeView for PropertyNodeView {
    fn header_color(&self, _node: &dyn Node) -> Color32 {
        // The default palette already hashes on `type_name`, so callers who want
        // type-specific colors can subclass; here we just pick a neutral teal
        // for any property-driven node.
        Color32::from_rgb(86, 168, 168)
    }

    fn body_ui(&self, ui: &mut egui::Ui, node: &mut dyn Node) {
        // No `downcast_mut` here: we reach the NodeSpec impl via the registry
        // bridge, mutate the property in place through `draw_property`, then
        // commit the new value back via `apply_property` (the node's source
        // of truth).
        let Some(spec) = self.registry.spec_of_mut(node) else {
            ui.label("(unregistered node type)");
            return;
        };
        let mut props = spec.properties();
        for (name, prop) in &mut props {
            if let Some(new_value) = draw_property(ui, prop) {
                // Re-fetch the spec mutably and apply. The first `spec_of_mut`
                // borrow ended when we collected `props` by value.
                if let Some(spec) = self.registry.spec_of_mut(node) {
                    let _ = spec.apply_property(name, new_value);
                }
            }
        }
    }

    fn size_hint(&self, node: &dyn Node) -> Vec2 {
        // Card height = header + pin band (one row per socket, inputs/outputs
        // share rows) + property widgets + bottom padding, matching the
        // painter's layout so sliders never overflow the card.
        let n_in = node.inputs().len() as f32;
        let n_out = node.outputs().len() as f32;
        let pin_band = SOCKET_TOP_MARGIN + n_in.max(n_out) * SOCKET_ROW_HEIGHT + SOCKET_BOTTOM_GAP;
        let n_props = self
            .registry
            .spec_of(node)
            .map(|spec| spec.properties().len())
            .unwrap_or(0) as f32;
        let height =
            CARD_HEADER_HEIGHT + pin_band + n_props * PROPERTY_ROW_HEIGHT + BODY_BOTTOM_PAD;
        Vec2::new(CARD_WIDTH, height)
    }
}
