//! Kahn's algorithm topological sort.

use std::collections::{HashMap, HashSet, VecDeque};

use ze_node::algo::{CycleError, TopoSort};
use ze_node::graph::Graph;
use ze_node::id::{InputSocketId, NodeId};

/// Topological sort via Kahn's algorithm (BFS over in-degrees).
#[derive(Debug, Default)]
pub struct KahnSorter;

impl KahnSorter {
    pub fn new() -> Self {
        Self
    }
}

impl TopoSort for KahnSorter {
    fn sort(&self, graph: &dyn Graph) -> Result<Vec<NodeId>, CycleError> {
        // Build node-level dependency edges (deduplicated).
        let mut succ: HashMap<NodeId, HashSet<NodeId>> = HashMap::new();
        let mut in_degree: HashMap<NodeId, usize> = HashMap::new();
        let all: HashSet<NodeId> = graph.all_nodes().collect();

        for n in graph.all_nodes() {
            in_degree.entry(n).or_insert(0);
            succ.entry(n).or_default();
            let node = match graph.get_node(n) {
                Some(nd) => nd,
                None => continue,
            };
            for i in 0..node.inputs().len() {
                let in_id = InputSocketId::new(n, i as u32);
                for out in graph.upstream(in_id) {
                    let from = out.node();
                    if from == n {
                        continue; // ignore self-loops for degree; cycle caught later
                    }
                    if succ.entry(from).or_default().insert(n) {
                        *in_degree.entry(n).or_insert(0) += 1;
                    }
                }
            }
        }

        let mut queue: VecDeque<NodeId> = in_degree
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(k, _)| *k)
            .collect();
        // Deterministic-ish: sort initial queue by id.
        let mut start: Vec<NodeId> = queue.drain(..).collect();
        start.sort_unstable();
        for n in start {
            queue.push_back(n);
        }

        let mut order = Vec::with_capacity(all.len());
        while let Some(n) = queue.pop_front() {
            order.push(n);
            if let Some(succs) = succ.get(&n) {
                let mut next_zero: Vec<NodeId> = Vec::new();
                for &s in succs {
                    let d = in_degree.get_mut(&s).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        next_zero.push(s);
                    }
                }
                next_zero.sort_unstable();
                for s in next_zero {
                    queue.push_back(s);
                }
            }
        }

        if order.len() == all.len() {
            Ok(order)
        } else {
            let remaining: Vec<NodeId> =
                all.iter().filter(|n| !order.contains(n)).copied().collect();
            Err(CycleError::Cycle { nodes: remaining })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;

    use super::*;
    use ze_node::node::{DirtyPolicy, Node, NodeMeta};
    use ze_node::simple_graph::SimpleGraph;
    use ze_node::socket::{InputSocket, OutputSocket};
    use ze_node::type_tag::NamedTypeTag;

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
        fn state_type(&self) -> Box<dyn ze_node::TypeTag> {
            Box::new(NamedTypeTag::new("()"))
        }
    }
    fn mk(n_in: usize, n_out: usize) -> N {
        let tag: Box<dyn ze_node::TypeTag> = Box::new(NamedTypeTag::new("f32"));
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
    fn dag_sorts_upstream_first() {
        let mut g = SimpleGraph::default();
        let a = g.add_node(Box::new(mk(0, 1)));
        let b = g.add_node(Box::new(mk(1, 1)));
        let c = g.add_node(Box::new(mk(1, 0)));
        g.quick_connect(
            ze_node::OutputSocketId::new(a, 0),
            ze_node::InputSocketId::new(b, 0),
        )
        .unwrap();
        g.quick_connect(
            ze_node::OutputSocketId::new(b, 0),
            ze_node::InputSocketId::new(c, 0),
        )
        .unwrap();
        let order = KahnSorter::new().sort(&g).unwrap();
        let pos = |id: NodeId| order.iter().position(|&x| x == id).unwrap();
        assert!(pos(a) < pos(b));
        assert!(pos(b) < pos(c));
    }

    #[test]
    fn cycle_returns_error() {
        let mut g = SimpleGraph::default();
        let a = g.add_node(Box::new(mk(1, 1)));
        let b = g.add_node(Box::new(mk(1, 1)));
        g.quick_connect(
            ze_node::OutputSocketId::new(a, 0),
            ze_node::InputSocketId::new(b, 0),
        )
        .unwrap();
        g.quick_connect(
            ze_node::OutputSocketId::new(b, 0),
            ze_node::InputSocketId::new(a, 0),
        )
        .unwrap();
        let err = KahnSorter::new().sort(&g);
        assert!(matches!(err, Err(CycleError::Cycle { .. })));
    }
}
