//! Edges and edge-carried data.
//!
//! An [`Edge`] connects one output socket to one input socket. Each edge
//! carries opaque [`EdgeData`] (a type-erased, cloneable, send+sync payload) so
//! downstream domains can attach per-connection metadata (weights, labels,
//! colour, ...). The default [`UnitEdge`] is zero-cost empty data.
//!
//! Every edge carries a version counter used for dirty detection: mutating the
//! edge data bumps the version so trackers know the downstream is stale.

use std::any::Any;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::id::{InputSocketId, OutputSocketId};

/// Identifier of an edge — the (source, target) socket pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeId {
    pub source: OutputSocketId,
    pub target: InputSocketId,
}

/// Type-erased, cloneable payload carried by an edge.
pub trait EdgeData: fmt::Debug + Send + Sync + 'static {
    fn clone_box(&self) -> Box<dyn EdgeData>;
    fn as_any(&self) -> &dyn Any;
    fn dyn_eq(&self, other: &dyn EdgeData) -> bool;
}

impl Clone for Box<dyn EdgeData> {
    fn clone(&self) -> Self {
        self.as_ref().clone_box()
    }
}

impl PartialEq for Box<dyn EdgeData> {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref().dyn_eq(other.as_ref())
    }
}

impl Eq for Box<dyn EdgeData> {}

/// Default empty edge data (zero overhead).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitEdge;

impl EdgeData for UnitEdge {
    fn clone_box(&self) -> Box<dyn EdgeData> {
        Box::new(*self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn dyn_eq(&self, other: &dyn EdgeData) -> bool {
        other.as_any().downcast_ref::<Self>().is_some()
    }
}

/// A `Box<dyn EdgeData>` defaults to a [`UnitEdge`] so that serde can `skip`
/// the field when serialising the graph topology.
impl Default for Box<dyn EdgeData> {
    fn default() -> Self {
        Box::new(UnitEdge)
    }
}

/// Monotonic version counter for dirty detection.
pub type EdgeVersion = u64;

/// A connection between an output socket and an input socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub source: OutputSocketId,
    pub target: InputSocketId,
    /// Monotonic version; bumped whenever `data` mutates.
    pub version: EdgeVersion,
    /// Opaque per-edge payload. Not serialised; deserialised edges get a
    /// [`UnitEdge`]. Domains with custom data plug their own serde.
    #[serde(skip)]
    pub data: Box<dyn EdgeData>,
}

impl Edge {
    pub fn new(source: OutputSocketId, target: InputSocketId, data: Box<dyn EdgeData>) -> Self {
        Self {
            source,
            target,
            version: 0,
            data,
        }
    }

    /// Quick-connect with empty ([`UnitEdge`]) data.
    pub fn unit(source: OutputSocketId, target: InputSocketId) -> Self {
        Self::new(source, target, Box::new(UnitEdge))
    }

    pub fn id(&self) -> EdgeId {
        EdgeId {
            source: self.source,
            target: self.target,
        }
    }

    /// Bump the version (call after mutating `data`).
    pub fn bump(&mut self) {
        self.version = self.version.wrapping_add(1);
    }
}

impl PartialEq for Edge {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source && self.target == other.target
    }
}

impl Eq for Edge {}
