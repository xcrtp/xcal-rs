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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeMeta {
    /// Stable, unique type identifier (e.g. `"math.constant.f32"`).
    pub type_name: String,
    /// Localised display name.
    pub display_name: String,
    /// Schema version of this node type.
    pub version: u32,
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
        }
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
