//! Type tags and socket-level type compatibility.
//!
//! The graph stores values as [`Box<dyn Any + Send>`] (runtime type erasure) so
//! the core layer never needs to know concrete value types. Each socket carries
//! a [`TypeTag`] identifying the logical type it accepts/produces, and a
//! [`TypeCast`] decides whether two tags are connectable and performs implicit
//! conversions.
//!
//! `TypeTag` is a trait (not a concrete enum) so downstream crates can supply
//! their own domain type systems (e.g. a `CoreSocketValue` enum). Because trait
//! objects cannot directly `Clone`/`PartialEq`, the trait exposes the
//! `clone_box` / `dyn_eq` pattern and [`Box<dyn TypeTag>`] gets the standard
//! trait impls forwarded manually.

use std::any::Any;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Logical type identifier carried by a socket.
///
/// Implementors should be cheap to clone and compare; they describe *what kind*
/// of value flows on a socket, not the value itself.
pub trait TypeTag: fmt::Debug + Send + Sync + 'static {
    /// Clone onto the heap (object-safe clone).
    fn clone_box(&self) -> Box<dyn TypeTag>;
    /// Object-safe equality.
    fn dyn_eq(&self, other: &dyn TypeTag) -> bool;
    /// Stable human-readable name (used for diagnostics & serde tagging).
    fn type_name(&self) -> &'static str;
    /// Downcast access for casters that want the concrete tag.
    fn as_any(&self) -> &dyn Any;
}

impl Clone for Box<dyn TypeTag> {
    fn clone(&self) -> Self {
        self.as_ref().clone_box()
    }
}

impl PartialEq for Box<dyn TypeTag> {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref().dyn_eq(other.as_ref())
    }
}

impl Eq for Box<dyn TypeTag> {}

/// A trivial string-named type tag for tests and simple domains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedTypeTag {
    name: &'static str,
}

impl NamedTypeTag {
    pub const fn new(name: &'static str) -> Self {
        Self { name }
    }
    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl TypeTag for NamedTypeTag {
    fn clone_box(&self) -> Box<dyn TypeTag> {
        Box::new(self.clone())
    }
    fn dyn_eq(&self, other: &dyn TypeTag) -> bool {
        match other.as_any().downcast_ref::<Self>() {
            Some(o) => self == o,
            None => false,
        }
    }
    fn type_name(&self) -> &'static str {
        self.name
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Error raised when a value cannot be cast between two type tags.
#[derive(Debug, thiserror::Error)]
pub enum CastError {
    #[error("incompatible types: {from} -> {to}")]
    Incompatible { from: String, to: String },
    #[error("value does not match tag {tag}")]
    ValueTypeMismatch { tag: String },
}

/// Decides type compatibility and performs implicit conversion between two
/// [`TypeTag`]s.
///
/// The blanket default ([`DefaultTypeCast`]) only allows connecting identical
/// tags. Downstream crates supply richer casters (e.g. `f32 -> f64` with scale).
pub trait TypeCast: Send + Sync + 'static {
    /// Can a value of `from` flow into a socket expecting `to`?
    fn is_compatible(&self, from: &dyn TypeTag, to: &dyn TypeTag) -> bool;
    /// Convert `value` (tagged `from`) into a value tagged `to`.
    fn try_cast(
        &self,
        value: Box<dyn Any + Send>,
        from: &dyn TypeTag,
        to: &dyn TypeTag,
    ) -> Result<Box<dyn Any + Send>, CastError>;
}

/// Default caster: identity only.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultTypeCast;

impl TypeCast for DefaultTypeCast {
    fn is_compatible(&self, from: &dyn TypeTag, to: &dyn TypeTag) -> bool {
        from.dyn_eq(to)
    }
    fn try_cast(
        &self,
        value: Box<dyn Any + Send>,
        from: &dyn TypeTag,
        to: &dyn TypeTag,
    ) -> Result<Box<dyn Any + Send>, CastError> {
        if from.dyn_eq(to) {
            Ok(value)
        } else {
            Err(CastError::Incompatible {
                from: from.type_name().to_string(),
                to: to.type_name().to_string(),
            })
        }
    }
}
