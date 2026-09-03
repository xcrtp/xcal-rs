//! `xcal-node-ui` — the property model layer for xcal-node.
//!
//! Sits above [`xc_node`] (the pure node/graph kernel) and below any UI
//! layer. Provides:
//!   * [`Property`] / [`PropertyValue`] — a serializable, UI-neutral vocabulary
//!     of parameter kinds (scalar, vec, color, enum, range, function, dynamic,
//!     list, …);
//!   * [`NodeSpec`] — the declarative property interface a node implements on
//!     top of [`xc_node::Node`];
//!   * [`NodeRegistry`] — a type-erased catalogue that can spawn node instances
//!     and reach their `NodeSpec` impl from a `&dyn Node` without any caller
//!     downcast (the bridge over Rust's missing trait-upcasting);
//!   * [`DynamicObject`] — a runtime-extensible bag of named properties so
//!     users can add fields without recompiling.
//!
//! Hard constraint (plan §1): this crate depends only on `xc_node` + `serde`.
//! It must **not** depend on `egui`, `xcal-node-editor`, `xcal-node-egui(-ui)`
//! or any project-internal crate other than `xcal-node`. That keeps the
//! "backend-swappable / WASM / headless" architecture promise.

#![forbid(unsafe_code)]

pub mod dynamic;
pub mod dynamic_node;
pub mod error;
pub mod property;
pub mod registry;
pub mod spec;

pub use dynamic::{DynamicField, DynamicObject};
pub use dynamic_node::{DynamicNode, DynamicNodeBuilder};
pub use error::PropertyError;
pub use property::{
    BoolConfig, ColorConfig, DoubleConfig, DynamicConfig, EnumConfig, FunctionConfig,
    FunctionHandle, IntegerConfig, ListConfig, Property, PropertyValue, RangeConfig, ScalarConfig,
    StringConfig, StringKind, VEC_MAX_COMPONENTS, VecNConfig,
};
pub use registry::{NodeRegistry, RegistryCategory, RegistryNodeInfo};
pub use spec::{NodeSpec, property_schema, property_value};

/// Re-export of the `#[derive(NodeSpec)]` proc-macro.
///
/// Downstream crates write `use xc_node_schema::NodeSpec;` to get the trait, and
/// `#[derive(NodeSpec)]` resolves to this proc-macro via the trait's name (the
/// derive is invoked in attribute position, which does not conflict with the
/// trait re-export in type position).
pub use xc_node_schema_macros::NodeSpec;
