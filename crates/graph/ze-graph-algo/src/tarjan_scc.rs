//! Tarjan's strongly-connected-components algorithm, used for cycle detection.

use std::collections::{HashMap, HashSet};

use ze_node::algo::CycleDetector;
use ze_node::graph::Graph;
use ze_node::id::{InputSocketId, NodeId};

/// Cycle detector based on Tarjan's SCC algorithm.
#[derive(Debug, Default)]
pub struct TarjanSccDetector;

impl TarjanSccDetector {
    pub fn new() -> Self {
        Self
    }

    fn build_successors(graph: &dyn Graph) -> HashMap<NodeId, Vec<NodeId>> {
        let mut succ: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for n in graph.all_nodes() {
            succ.entry(n).or_default();
            let node = match graph.get_node(n) {
                Some(nd) => nd,
                None => continue,
            };
            for i in 0..node.inputs().len() {
                let in_id = InputSocketId::new(n, i as u32);
                for out in graph.upstream(in_id) {
                    let from = out.node();
                    succ.entry(from).or_default().push(n);
                }
            }
        }
        succ
    }
}

impl CycleDetector for TarjanSccDetector {
    fn detect(&self, graph: &dyn Graph) -> Option<Vec<NodeId>> {
        let succ = Self::build_successors(graph);
        let mut indices: HashMap<NodeId, usize> = HashMap::new();
        let mut lowlink: HashMap<NodeId, usize> = HashMap::new();
        let mut on_stack: HashSet<NodeId> = HashSet::new();
        let mut stack: Vec<NodeId> = Vec::new();
        let mut index_counter: usize = 0;
        let mut sccs: Vec<Vec<NodeId>> = Vec::new();

        #[allow(clippy::too_many_arguments)]
        fn strongconnect(
            v: NodeId,
            succ: &HashMap<NodeId, Vec<NodeId>>,
            indices: &mut HashMap<NodeId, usize>,
            lowlink: &mut HashMap<NodeId, usize>,
            on_stack: &mut HashSet<NodeId>,
            stack: &mut Vec<NodeId>,
            index_counter: &mut usize,
            sccs: &mut Vec<Vec<NodeId>>,
        ) {
            indices.insert(v, *index_counter);
            lowlink.insert(v, *index_counter);
            *index_counter += 1;
            stack.push(v);
            on_stack.insert(v);

            let neighbors = succ.get(&v).cloned().unwrap_or_default();
            for w in neighbors {
                if !indices.contains_key(&w) {
                    strongconnect(
                        w,
                        succ,
                        indices,
                        lowlink,
                        on_stack,
                        stack,
                        index_counter,
                        sccs,
                    );
                    let lw = *lowlink.get(&w).unwrap();
                    let lv = lowlink.get(&v).copied().unwrap();
                    lowlink.insert(v, lv.min(lw));
                } else if on_stack.contains(&w) {
                    let iw = *indices.get(&w).unwrap();
                    let lv = lowlink.get(&v).copied().unwrap();
                    lowlink.insert(v, lv.min(iw));
                }
            }

            if lowlink.get(&v) == indices.get(&v) {
                let mut comp = Vec::new();
                loop {
                    let w = stack.pop().unwrap();
                    on_stack.remove(&w);
                    comp.push(w);
                    if w == v {
                        break;
                    }
                }
                sccs.push(comp);
            }
        }

        let roots: Vec<NodeId> = graph.all_nodes().collect();
        for r in roots {
            if !indices.contains_key(&r) {
                strongconnect(
                    r,
                    &succ,
                    &mut indices,
                    &mut lowlink,
                    &mut on_stack,
                    &mut stack,
                    &mut index_counter,
                    &mut sccs,
                );
            }
        }

        // An SCC of size > 1 is a cycle; a singleton with a self-loop is too.
        for comp in sccs {
            if comp.len() > 1 {
                return Some(comp);
            }
            if comp.len() == 1 {
                let n = comp[0];
                if succ.get(&n).map(|v| v.contains(&n)).unwrap_or(false) {
                    return Some(comp);
                }
            }
        }
        None
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
    fn detects_two_node_cycle() {
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
        assert!(TarjanSccDetector::new().detect(&g).is_some());
    }

    #[test]
    fn no_cycle_in_dag() {
        let mut g = SimpleGraph::default();
        let a = g.add_node(Box::new(mk(0, 1)));
        let b = g.add_node(Box::new(mk(1, 0)));
        g.quick_connect(
            ze_node::OutputSocketId::new(a, 0),
            ze_node::InputSocketId::new(b, 0),
        )
        .unwrap();
        assert!(TarjanSccDetector::new().detect(&g).is_none());
    }
}
