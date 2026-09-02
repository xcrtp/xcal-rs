//! Breadth-first dirty propagation.

use std::collections::{HashSet, VecDeque};

use xcal_node::algo::DirtyTracker;
use xcal_node::graph::Graph;
use xcal_node::id::{NodeId, OutputSocketId};

/// Dirty tracker that propagates marks along downstream edges via BFS.
#[derive(Debug, Default)]
pub struct BfsDirtyTracker {
    dirty: HashSet<NodeId>,
}

impl BfsDirtyTracker {
    pub fn new() -> Self {
        Self::default()
    }
}

impl DirtyTracker for BfsDirtyTracker {
    fn mark_dirty(&mut self, node: NodeId) {
        self.dirty.insert(node);
    }

    fn mark_socket_dirty(&mut self, graph: &dyn Graph, socket: OutputSocketId) {
        // Walk downstream: output -> inputs -> their nodes -> their outputs -> ...
        let mut queue: VecDeque<OutputSocketId> = VecDeque::new();
        let mut visited: HashSet<OutputSocketId> = HashSet::new();
        queue.push_back(socket);
        visited.insert(socket);

        while let Some(out) = queue.pop_front() {
            for input in graph.downstream(out) {
                let node = input.node();
                self.dirty.insert(node);
                // enqueue every output of the now-dirty node
                if let Some(n) = graph.get_node(node) {
                    for i in 0..n.outputs().len() {
                        let next = OutputSocketId::new(node, i as u32);
                        if visited.insert(next) {
                            queue.push_back(next);
                        }
                    }
                }
            }
        }
    }

    fn is_dirty(&self, node: NodeId) -> bool {
        self.dirty.contains(&node)
    }

    fn clear_dirty(&mut self, node: NodeId) {
        self.dirty.remove(&node);
    }

    fn collect_dirty(&self) -> HashSet<NodeId> {
        self.dirty.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;

    use super::*;
    use xcal_node::node::{DirtyPolicy, Node, NodeMeta};
    use xcal_node::simple_graph::SimpleGraph;
    use xcal_node::socket::{InputSocket, OutputSocket};
    use xcal_node::type_tag::NamedTypeTag;

    macro_rules! impl_as_any {
        () => {
            fn as_any(&self) -> &dyn Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn Any {
                self
            }
        };
    }

    #[derive(Debug)]
    struct N {
        meta: NodeMeta,
        inputs: Vec<InputSocket>,
        outputs: Vec<OutputSocket>,
    }
    impl Node for N {
        impl_as_any!();
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
        fn state_type(&self) -> Box<dyn xcal_node::TypeTag> {
            Box::new(NamedTypeTag::new("()"))
        }
    }
    fn mk(n_in: usize, n_out: usize) -> N {
        let tag: Box<dyn xcal_node::TypeTag> = Box::new(NamedTypeTag::new("f32"));
        N {
            meta: NodeMeta::new("t", "t", 1),
            inputs: (0..n_in)
                .map(|i| InputSocket::new(tag.clone()).with_index(i as u32))
                .collect(),
            outputs: (0..n_out)
                .map(|i| OutputSocket::new(tag.clone()).with_index(i as u32))
                .collect(),
        }
    }

    #[test]
    fn propagates_to_all_downstream() {
        let mut g = SimpleGraph::default();
        let a = g.add_node(Box::new(mk(0, 1)));
        let b = g.add_node(Box::new(mk(1, 1)));
        let c = g.add_node(Box::new(mk(1, 0)));
        g.quick_connect(xcal_node::OutputSocketId::new(a, 0), xcal_node::InputSocketId::new(b, 0))
            .unwrap();
        g.quick_connect(xcal_node::OutputSocketId::new(b, 0), xcal_node::InputSocketId::new(c, 0))
            .unwrap();

        let mut t = BfsDirtyTracker::new();
        t.mark_socket_dirty(&g, xcal_node::OutputSocketId::new(a, 0));
        assert!(t.is_dirty(b));
        assert!(t.is_dirty(c));
        assert!(!t.is_dirty(a)); // source not marked, only downstream
    }
}
