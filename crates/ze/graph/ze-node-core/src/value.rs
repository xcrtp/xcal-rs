//! Type-erased, cloneable value container.
//!
//! The plan calls for `Box<dyn Any>` as the runtime value type. Pure `Any` is
//! not cloneable, but a single output socket may fan out to many inputs — so
//! cached values must be shareable/cloneable (the plan's open decision #4:
//! "Arc share vs borrow"). [`NodeValue`] extends `Any + Send + Sync` with a
//! `clone_box` so fan-out is cheap and downcast stays possible via [`Any`].
//!
//! A blanket impl covers every `T: Clone + Any + Send + Sync + 'static`, so
//! downstream value types need no manual plumbing.

use std::any::Any;
use std::fmt;

/// Cloneable, type-erased value carried on sockets.
pub trait NodeValue: Any + Send + Sync + 'static {
    fn clone_box(&self) -> Box<dyn NodeValue>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_box(self: Box<Self>) -> Box<dyn Any>;
}

impl<T: Clone + Any + Send + Sync + 'static> NodeValue for T {
    fn clone_box(&self) -> Box<dyn NodeValue> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_box(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

impl Clone for Box<dyn NodeValue> {
    fn clone(&self) -> Self {
        self.as_ref().clone_box()
    }
}

impl fmt::Debug for Box<dyn NodeValue> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = self.as_ref().as_any();
        f.debug_tuple("NodeValue")
            .field(&format_args!("{:p}", inner))
            .finish()
    }
}

impl dyn NodeValue {
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }
}
