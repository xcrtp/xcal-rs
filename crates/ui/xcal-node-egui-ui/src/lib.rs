//! `xcal-node-egui-ui` — egui widget mappings for `xcal-node-ui` property nodes.
//!
//! Sits above [`xc_node_schema`] (property model) and [`xcal_node_egui`] (canvas
//! / `NodeView` trait) and provides:
//!   * [`draw_property`] — enum-dispatched `Property -> egui widget` mapping;
//!     returns `Option<PropertyValue>` so the host can call
//!     `NodeSpec::apply_property` (no `downcast` anywhere).
//!   * [`PropertyNodeView`] — a generic `NodeView` that auto-renders any
//!     registered `NodeSpec` node's body from its `properties()` list.
//!   * [`spawn_menu`] — builds a right-click spawn menu from a
//!     [`NodeRegistry::categories`] list.
//!
//! Dependency direction (plan §1): this crate depends on `xcal-node-ui` +
//! `xcal-node-egui` + `egui`. egui only appears at this layer and above.

#![forbid(unsafe_code)]

pub mod draw;
pub mod menu;
pub mod view;

pub use draw::draw_property;
pub use menu::spawn_menu;
pub use view::PropertyNodeView;
