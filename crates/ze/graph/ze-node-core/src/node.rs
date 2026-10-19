//! The [`Node`] trait and related metadata / dirty policy.
//!
//! Design note on object safety: the plan sketches `Node` with an associated
//! `type Data`. Associated types (without defaults, which are unstable on
//! stable Rust) make a trait non-object-safe, so `Box<dyn Node>` would not
//! compile. Node *state* is already dynamically typed through [`crate::ExecContext`]
//! (`get_state`/`set_state` use `Box<dyn Any>` and [`Node::state_type`] returns
//! a [`crate::TypeTag`]). Therefore `Node` is kept object-safe and storable as
//! `Box<dyn Node>`; [`DowncastWrapper`] provides `as_any`-based downcast helpers
//! for callers that need the concrete node back.

use std::any::Any;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::id::InputSocketId;
use crate::socket::{InputSocket, OutputSocket};
use crate::type_tag::TypeTag;

/// Static, human-facing metadata for a node type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeMeta {
    /// Stable, unique type identifier (e.g. `"math.constant.f32"`).
    pub type_name: String,
    /// Localised display name.
    pub display_name: String,
    /// Schema version of this node type.
    pub version: u32,
    /// Editor interaction policy for this node type (draggable / selectable /
    /// resizable + size bounds). Defaults are permissive; old serialized
    /// metadata without this field gets `NodeInteraction::default()`.
    #[serde(default)]
    pub interaction: NodeInteraction,
}

impl NodeMeta {
    pub fn new(
        type_name: impl Into<String>,
        display_name: impl Into<String>,
        version: u32,
    ) -> Self {
        Self {
            type_name: type_name.into(),
            display_name: display_name.into(),
            version,
            interaction: NodeInteraction::default(),
        }
    }

    /// Builder: override the interaction policy.
    pub fn with_interaction(mut self, interaction: NodeInteraction) -> Self {
        self.interaction = interaction;
        self
    }
}

/// Per-node-type editor interaction policy, carried in [`NodeMeta`] so the
/// UI/hit-test layer can be data-driven instead of hard-coded. Sizes are in
/// graph-local units (1 unit = 1 pixel at zoom 1.0); this crate stays free of
/// vector-math dependencies by storing them as `[width, height]` pairs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeInteraction {
    /// The card can be dragged by grabbing its title band.
    pub draggable: bool,
    /// Clicking the card adds it to the selection.
    pub selectable: bool,
    /// The card exposes a resize grip and accepts resize gestures.
    pub resizable: bool,
    /// Minimum card size `[width, height]`.
    pub min_size: [f32; 2],
    /// Maximum card size `[width, height]`.
    pub max_size: [f32; 2],
}

impl Default for NodeInteraction {
    fn default() -> Self {
        Self {
            draggable: true,
            selectable: true,
            resizable: true,
            min_size: [120.0, 64.0],
            max_size: [1200.0, 900.0],
        }
    }
}

impl NodeInteraction {
    /// A fully locked node: no drag, no selection, no resize (fixed card).
    pub const fn locked() -> Self {
        Self {
            draggable: false,
            selectable: false,
            resizable: false,
            min_size: [120.0, 64.0],
            max_size: [1200.0, 900.0],
        }
    }

    pub fn draggable(mut self, draggable: bool) -> Self {
        self.draggable = draggable;
        self
    }

    pub fn selectable(mut self, selectable: bool) -> Self {
        self.selectable = selectable;
        self
    }

    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    pub fn min_size(mut self, width: f32, height: f32) -> Self {
        self.min_size = [width, height];
        self
    }

    pub fn max_size(mut self, width: f32, height: f32) -> Self {
        self.max_size = [width, height];
        self
    }

    /// Clamp a proposed card size into `[min_size, max_size]` (componentwise).
    pub fn clamp_size(&self, width: f32, height: f32) -> [f32; 2] {
        [
            width.clamp(self.min_size[0], self.max_size[0]),
            height.clamp(self.min_size[1], self.max_size[1]),
        ]
    }
}

/// When should a node be considered dirty (and thus re-evaluated)?
#[derive(Debug)]
pub enum DirtyPolicy {
    /// Never recompute unless explicitly marked dirty.
    Never,
    /// Recompute when an input snapshot changed since the last evaluation.
    OnInputChange,
    /// Always recompute (samplers, time-dependent nodes).
    Always,
    /// User-supplied predicate.
    Custom(Box<dyn DirtyFn>),
}

impl fmt::Debug for Box<dyn DirtyFn> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DirtyFn").finish_non_exhaustive()
    }
}

impl Clone for Box<dyn DirtyFn> {
    fn clone(&self) -> Self {
        self.as_ref().clone_box()
    }
}

/// Predicate used by [`DirtyPolicy::Custom`].
pub trait DirtyFn: Send + Sync + 'static {
    /// `true` if the node should be treated as dirty this frame.
    fn is_dirty(&self) -> bool;
    /// Object-safe clone.
    fn clone_box(&self) -> Box<dyn DirtyFn>;
}

impl Clone for DirtyPolicy {
    fn clone(&self) -> Self {
        match self {
            DirtyPolicy::Never => DirtyPolicy::Never,
            DirtyPolicy::OnInputChange => DirtyPolicy::OnInputChange,
            DirtyPolicy::Always => DirtyPolicy::Always,
            DirtyPolicy::Custom(f) => DirtyPolicy::Custom(f.clone()),
        }
    }
}

/// A node in the graph.
///
/// Object-safe: storable as `Box<dyn Node>`. Sockets are borrowed (`&[...]`)
/// so the node owns its socket table. Lifecycle hooks default to no-ops.
///
/// `as_any` / `as_any_mut` are **required** (not default) because a default
/// body would need `Self: Sized`, breaking `dyn Node`. Implementors write the
/// trivial `{ self }` body (concrete types are `Sized + 'static`, hence `Any`);
/// a future derive macro can automate this. `type_id` *is* given a default
/// since [`std::any::TypeId::of`] accepts `?Sized` types.
pub trait Node: fmt::Debug + Send + Sync + 'static {
    /// Static type metadata.
    fn meta(&self) -> &NodeMeta;
    /// Input socket descriptors (indexed by the `index` field of [`InputSocketId`]).
    fn inputs(&self) -> &[InputSocket];
    /// Output socket descriptors.
    fn outputs(&self) -> &[OutputSocket];
    /// Recompute policy.
    fn dirty_policy(&self) -> DirtyPolicy;
    /// Type tag describing the per-node state value (see [`crate::ExecContext`]).
    fn state_type(&self) -> Box<dyn TypeTag>;

    /// `&dyn Any` view of the concrete node (for downcasting).
    fn as_any(&self) -> &dyn Any;
    /// `&mut dyn Any` view of the concrete node (for downcasting).
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// Concrete [`std::any::TypeId`] of this node (default works for `?Sized`).
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    // --- lifecycle hooks (default no-ops) ---
    fn on_add(&mut self) {}
    fn on_remove(&mut self) {}
    /// Called after an input socket gets a new connection.
    fn on_connect(&mut self, _socket: InputSocketId) {}
    /// Called after an input socket loses a connection.
    fn on_disconnect(&mut self, _socket: InputSocketId) {}
}

/// Downcast helper trait. Auto-implemented for every `Node` (including the
/// unsized trait object `dyn Node`); lets callers recover the concrete node
/// behind a `Box<dyn Node>` / `&dyn Node`.
pub trait DowncastWrapper: Node {
    fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }
    fn downcast_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.as_any_mut().downcast_mut::<T>()
    }
}

impl<N: Node + ?Sized> DowncastWrapper for N {}

impl dyn Node {
    /// Convenience downcast on a trait object.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }
    pub fn downcast_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.as_any_mut().downcast_mut::<T>()
    }
}
