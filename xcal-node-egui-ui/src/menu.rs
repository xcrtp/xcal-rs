//! Right-click spawn menu built from a [`NodeRegistry`].
//!
//! Renders one button per registered node type, grouped by category. Returns
//! the chosen `type_name` (if any) so the host can call `registry.spawn(name)`
//! and add the new node to its editor.

use egui::Ui;

use xcal_node_ui::NodeRegistry;

/// Render the spawn menu. Returns `Some(type_name)` when the user clicks an
/// entry; the host is responsible for spawning the node and closing the menu.
pub fn spawn_menu(ui: &mut Ui, registry: &NodeRegistry) -> Option<String> {
    let mut chosen: Option<String> = None;
    for cat in registry.categories() {
        ui.label(egui::RichText::new(&cat.name).strong());
        for entry in &cat.entries {
            if ui.button(&entry.display_name).clicked() {
                chosen = Some(entry.type_name.clone());
            }
        }
        ui.separator();
    }
    chosen
}
