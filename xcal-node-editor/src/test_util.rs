//! Minimal node doubles for editor tests. Test-only.

use std::any::Any;

use xcal_node::{DirtyPolicy, InputSocket, NamedTypeTag, Node, NodeMeta, OutputSocket, TypeTag};

/// Configurable test node: `n_in` inputs and `n_out` outputs, all carrying
/// the same named type tag.
#[derive(Debug)]
pub(crate) struct TestNode {
    meta: NodeMeta,
    inputs: Vec<InputSocket>,
    outputs: Vec<OutputSocket>,
}

impl TestNode {
    pub fn new(tag: &'static str, n_in: usize, n_out: usize) -> Self {
        let in_tag: Box<dyn TypeTag> = Box::new(NamedTypeTag::new(tag));
        let out_tag: Box<dyn TypeTag> = Box::new(NamedTypeTag::new(tag));
        Self {
            meta: NodeMeta::new("test.node", "TestNode", 1),
            inputs: (0..n_in)
                .map(|i| InputSocket::new(in_tag.clone()).with_index(i as u32))
                .collect(),
            outputs: (0..n_out)
                .map(|i| OutputSocket::new(out_tag.clone()).with_index(i as u32))
                .collect(),
        }
    }

    /// One f32 input, one f32 output.
    pub fn f32_passthrough() -> Self {
        Self::new("f32", 1, 1)
    }
}

impl Node for TestNode {
    fn meta(&self) -> &NodeMeta {
        &self.meta
    }
    fn inputs(&self) -> &[InputSocket] {
        &self.inputs
    }
    fn outputs(&self) -> &[OutputSocket] {
        &self.outputs
    }
    fn dirty_policy(&self) -> DirtyPolicy {
        DirtyPolicy::Always
    }
    fn state_type(&self) -> Box<dyn TypeTag> {
        Box::new(NamedTypeTag::new("()"))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
