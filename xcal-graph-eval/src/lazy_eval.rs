//! Lazy pull evaluation strategy.
//!
//! [`LazyPullEval`] evaluates a graph in topological order, recomputing only
//! dirty nodes. Dirty policy is honoured per node:
//!   * [`DirtyPolicy::Always`] — recompute every pass;
//!   * [`DirtyPolicy::Never`] — compute once, reuse (unless explicitly marked
//!     dirty via [`LazyPullEval::mark_dirty`]);
//!   * [`DirtyPolicy::OnInputChange`] — recompute when the versions of the
//!     incoming edges changed since the last successful evaluation;
//!   * [`DirtyPolicy::Custom`] — delegate to the user predicate.
//!
//! Values are cached per output socket across passes. Errors carry a
//! [`RecoveryHandle`]; the caller picks an action and re-runs evaluation
//! (recovery) — see [`LazyPullEval::apply_recovery`].

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use xcal_node::algo::TopoSort;
use xcal_node::error::{EvalError, EvalErrorPolicy, RecoveryAction};
use xcal_node::executor::NodeExecutor;
use xcal_node::graph::Graph;
use xcal_node::id::{InputSocketId, NodeId, OutputSocketId};
use xcal_node::value::NodeValue;

use crate::context::DefaultExecContext;

/// Shared, interior-mutable references threaded through the recursive pull.
pub(crate) struct EvalRefs<'a> {
    pub graph: &'a dyn Graph,
    pub executor: &'a dyn NodeExecutor,
    pub eval: &'a LazyPullEval,
}

/// Lazy pull evaluation strategy.
pub struct LazyPullEval {
    sorter: Box<dyn TopoSort>,
    policy: EvalErrorPolicy,
    pub(crate) outputs: Mutex<HashMap<OutputSocketId, Box<dyn NodeValue>>>,
    states: Mutex<HashMap<NodeId, Box<dyn NodeValue>>>,
    snapshots: Mutex<HashMap<NodeId, Vec<(u32, Option<u64>)>>>,
    evaluated: Mutex<HashSet<NodeId>>,
    dirty: Mutex<HashSet<NodeId>>,
    in_progress: Mutex<HashSet<NodeId>>,
    errors: Mutex<Vec<EvalError>>,
}

impl LazyPullEval {
    pub fn new(sorter: Box<dyn TopoSort>, policy: EvalErrorPolicy) -> Self {
        Self {
            sorter,
            policy,
            outputs: Mutex::new(HashMap::new()),
            states: Mutex::new(HashMap::new()),
            snapshots: Mutex::new(HashMap::new()),
            evaluated: Mutex::new(HashSet::new()),
            dirty: Mutex::new(HashSet::new()),
            in_progress: Mutex::new(HashSet::new()),
            errors: Mutex::new(Vec::new()),
        }
    }

    /// Explicitly mark a node dirty (forces recompute even for `Never` policy).
    pub fn mark_dirty(&self, node: NodeId) {
        self.dirty.lock().unwrap().insert(node);
    }

    /// Clear a cached output (forces recompute of its node on next pass).
    pub fn invalidate_output(&self, socket: OutputSocketId) {
        self.outputs.lock().unwrap().remove(&socket);
        self.dirty.lock().unwrap().insert(socket.node());
    }

    /// Read a cached output value (owned clone), if present.
    pub fn cached_output(&self, socket: OutputSocketId) -> Option<Box<dyn NodeValue>> {
        self.outputs.lock().unwrap().get(&socket).cloned()
    }

    /// Errors collected under `ContinueOtherBranches` / `CollectAll`.
    pub fn take_errors(&self) -> Vec<EvalError> {
        std::mem::take(&mut *self.errors.lock().unwrap())
    }

    /// Apply a recovery action chosen on an error handle, then continue
    /// evaluating the (sub)graph so downstream propagates.
    pub fn apply_recovery(
        &self,
        mut err: EvalError,
        graph: &dyn Graph,
        executor: &dyn NodeExecutor,
        target: Option<OutputSocketId>,
    ) -> Result<(), EvalError> {
        let refs = EvalRefs {
            graph,
            executor,
            eval: self,
        };
        match err.recovery().take_action() {
            Some(RecoveryAction::Retry) => self.run_pass(&refs, target),
            Some(RecoveryAction::ResumeWith(value)) => {
                let socket = err.socket();
                let node = err.node();
                if let Some(s) = socket {
                    self.outputs.lock().unwrap().insert(s, value);
                }
                self.evaluated.lock().unwrap().insert(node);
                self.dirty.lock().unwrap().remove(&node);
                // Use ensure_output (not run_pass) so the failed node is not
                // re-evaluated: its output was just injected into the cache.
                match target {
                    Some(t) => ensure_output(t, &refs),
                    None => Ok(()),
                }
            }
            Some(RecoveryAction::SkipBranch) => {
                let node = err.node();
                self.evaluated.lock().unwrap().insert(node);
                self.dirty.lock().unwrap().remove(&node);
                // The failed node and its downstream are skipped. Other branches
                // were already evaluated during the original evaluate() call.
                Ok(())
            }
            Some(RecoveryAction::ReplaceAndRetry { replacements: _ }) => {
                let node = err.node();
                self.evaluated.lock().unwrap().remove(&node);
                self.dirty.lock().unwrap().insert(node);
                self.outputs.lock().unwrap().retain(|s, _| s.node() != node);
                self.run_pass(&refs, target)
            }
            None => Err(err),
        }
    }

    fn run_pass(&self, refs: &EvalRefs<'_>, target: Option<OutputSocketId>) -> Result<(), EvalError> {
        let order = self
            .sorter
            .sort(refs.graph)
            .map_err(|_e| EvalError::new("graph contains a cycle", NodeId(0), None))?;

        let scope: HashSet<NodeId> = match target {
            Some(t) => collect_upstream(t.node(), refs.graph),
            None => order.iter().copied().collect(),
        };

        for node in order {
            if !scope.contains(&node) {
                continue;
            }
            eval_node(node, refs)?;
        }
        Ok(())
    }
}

impl xcal_node::executor::EvalStrategy for LazyPullEval {
    fn evaluate(
        &mut self,
        graph: &dyn Graph,
        executor: &dyn NodeExecutor,
        target: Option<OutputSocketId>,
    ) -> Result<(), EvalError> {
        let refs = EvalRefs {
            graph,
            executor,
            eval: self,
        };
        let res = self.run_pass(&refs, target);
        match self.policy {
            EvalErrorPolicy::FailFast => res,
            EvalErrorPolicy::ContinueOtherBranches => res,
            EvalErrorPolicy::CollectAll => {
                if res.is_err() {
                    return res;
                }
                let n = self.errors.lock().unwrap().len();
                if n > 0 {
                    Err(EvalError::new(
                        format!("collected {n} evaluation errors"),
                        NodeId(0),
                        None,
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// Recursively ensure the output `socket` is present in the cache, evaluating
/// its node if necessary.
pub(crate) fn ensure_output(socket: OutputSocketId, refs: &EvalRefs<'_>) -> Result<(), EvalError> {
    if refs.eval.outputs.lock().unwrap().contains_key(&socket) {
        return Ok(());
    }
    eval_node(socket.node(), refs)?;
    Ok(())
}

/// Evaluate a single node (dirty-aware), writing its outputs to the cache.
fn eval_node(node_id: NodeId, refs: &EvalRefs<'_>) -> Result<(), EvalError> {
    if refs.eval.in_progress.lock().unwrap().contains(&node_id) {
        return Err(EvalError::new("evaluation cycle detected", node_id, None));
    }

    let node = match refs.graph.get_node(node_id) {
        Some(n) => n,
        None => return Err(EvalError::new("unknown node", node_id, None)),
    };

    let already = refs.eval.evaluated.lock().unwrap().contains(&node_id);
    let explicit_dirty = refs.eval.dirty.lock().unwrap().contains(&node_id);
    let is_dirty = match node.dirty_policy() {
        xcal_node::DirtyPolicy::Always => true,
        xcal_node::DirtyPolicy::Never => explicit_dirty || !already,
        xcal_node::DirtyPolicy::OnInputChange => input_changed(node_id, refs) || !already,
        xcal_node::DirtyPolicy::Custom(f) => f.is_dirty() || !already,
    };

    if !is_dirty {
        return Ok(());
    }

    refs.eval.in_progress.lock().unwrap().insert(node_id);

    // Pre-pull inputs (owned clones from the upstream output cache).
    let mut inputs: HashMap<InputSocketId, Box<dyn NodeValue>> = HashMap::new();
    let n_in = node.inputs().len();
    for i in 0..n_in {
        let in_id = InputSocketId::new(node_id, i as u32);
        let upstream = refs.graph.upstream(in_id);
        if let Some(out) = upstream.first().copied() {
            let need_compute = !refs.eval.outputs.lock().unwrap().contains_key(&out);
            if need_compute {
                eval_node(out.node(), refs)?;
            }
            let val = refs
                .eval
                .outputs
                .lock()
                .unwrap()
                .get(&out)
                .cloned()
                .ok_or_else(|| EvalError::new("upstream produced no value", node_id, None))?;
            inputs.insert(in_id, val);
        } else if let Some(def) = node.inputs()[i].default_value.as_ref() {
            inputs.insert(in_id, def.clone());
        }
        // else: leave absent; the node may error on peek or not need it.
    }

    // Pre-load state.
    let state = refs.eval.states.lock().unwrap().get(&node_id).cloned();
    let state_type = node.state_type();

    let mut cx = DefaultExecContext::new(refs, node_id, inputs, state, state_type);
    let result = refs.executor.execute(node, &mut cx);

    // Flush state back.
    if let Some(st) = cx.take_state() {
        refs.eval.states.lock().unwrap().insert(node_id, st);
    }

    refs.eval.in_progress.lock().unwrap().remove(&node_id);

    match result {
        Ok(()) => {
            refs.eval.evaluated.lock().unwrap().insert(node_id);
            refs.eval.dirty.lock().unwrap().remove(&node_id);
            let snap = collect_versions(node_id, refs);
            refs.eval.snapshots.lock().unwrap().insert(node_id, snap);
            Ok(())
        }
        Err(e) => {
            // Keep dirty so a retry recomputes.
            match refs.eval.policy {
                EvalErrorPolicy::FailFast => Err(e),
                EvalErrorPolicy::ContinueOtherBranches | EvalErrorPolicy::CollectAll => {
                    refs.eval.errors.lock().unwrap().push(e);
                    Ok(())
                }
            }
        }
    }
}

fn collect_versions(node_id: NodeId, refs: &EvalRefs<'_>) -> Vec<(u32, Option<u64>)> {
    let node = match refs.graph.get_node(node_id) {
        Some(n) => n,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for i in 0..node.inputs().len() {
        let in_id = InputSocketId::new(node_id, i as u32);
        let ver = refs
            .graph
            .upstream(in_id)
            .first()
            .copied()
            .and_then(|o| refs.graph.edge_version(o, in_id));
        out.push((i as u32, ver));
    }
    out
}

fn input_changed(node_id: NodeId, refs: &EvalRefs<'_>) -> bool {
    let cur = collect_versions(node_id, refs);
    let prev = refs.eval.snapshots.lock().unwrap().get(&node_id).cloned();
    match prev {
        Some(p) => cur != p,
        None => true,
    }
}

fn collect_upstream(start: NodeId, graph: &dyn Graph) -> HashSet<NodeId> {
    let mut set = HashSet::new();
    let mut stack = vec![start];
    while let Some(n) = stack.pop() {
        if !set.insert(n) {
            continue;
        }
        if let Some(node) = graph.get_node(n) {
            for i in 0..node.inputs().len() {
                let in_id = InputSocketId::new(n, i as u32);
                for o in graph.upstream(in_id) {
                    stack.push(o.node());
                }
            }
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::sync::{Arc, Mutex};

    use xcal_graph_algo::KahnSorter;
    use xcal_node::executor::EvalStrategy;
    use xcal_node::node::{DirtyPolicy, Node, NodeMeta};
    use xcal_node::simple_graph::SimpleGraph;
    use xcal_node::socket::{InputSocket, OutputSocket};
    use xcal_node::type_tag::NamedTypeTag;
    use xcal_node::{
        ExecContext, NodeExecutor, OutputSocketId as OutId, TypeTag,
    };

    use super::*;

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

    fn f32_tag() -> Box<dyn TypeTag> {
        Box::new(NamedTypeTag::new("f32"))
    }

    // --- test nodes ---
    #[derive(Debug, Clone)]
    struct ConstNode {
        meta: NodeMeta,
        value: f32,
        outputs: Vec<OutputSocket>,
        policy: DirtyPolicy,
    }
    impl ConstNode {
        fn new(value: f32, policy: DirtyPolicy) -> Self {
            Self {
                meta: NodeMeta::new("test.const", "Const", 1),
                value,
                outputs: vec![OutputSocket::new(f32_tag()).with_index(0)],
                policy,
            }
        }
    }
    impl Node for ConstNode {
        impl_as_any!();
        fn meta(&self) -> &NodeMeta {
            &self.meta
        }
        fn inputs(&self) -> &[xcal_node::socket::InputSocket] {
            &[]
        }
        fn outputs(&self) -> &[OutputSocket] {
            &self.outputs
        }
        fn dirty_policy(&self) -> DirtyPolicy {
            self.policy.clone()
        }
        fn state_type(&self) -> Box<dyn TypeTag> {
            Box::new(NamedTypeTag::new("()"))
        }
    }

    #[derive(Debug, Clone)]
    struct AddNode {
        meta: NodeMeta,
        inputs: Vec<InputSocket>,
        outputs: Vec<OutputSocket>,
        policy: DirtyPolicy,
        add: f32,
    }
    impl AddNode {
        fn new(add: f32, policy: DirtyPolicy) -> Self {
            Self {
                meta: NodeMeta::new("test.add", "Add", 1),
                inputs: vec![InputSocket::new(f32_tag()).with_index(0)],
                outputs: vec![OutputSocket::new(f32_tag()).with_index(0)],
                policy,
                add,
            }
        }
    }
    impl Node for AddNode {
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
            self.policy.clone()
        }
        fn state_type(&self) -> Box<dyn TypeTag> {
            Box::new(NamedTypeTag::new("()"))
        }
    }

    #[derive(Debug, Clone)]
    struct FailNode {
        meta: NodeMeta,
        outputs: Vec<OutputSocket>,
    }
    impl FailNode {
        fn new() -> Self {
            Self {
                meta: NodeMeta::new("test.fail", "Fail", 1),
                outputs: vec![OutputSocket::new(f32_tag()).with_index(0)],
            }
        }
    }
    impl Node for FailNode {
        impl_as_any!();
        fn meta(&self) -> &NodeMeta {
            &self.meta
        }
        fn inputs(&self) -> &[InputSocket] {
            &[]
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
    }

    /// Executor that logs every execution and computes Const/Add/Fail.
    struct TestExecutor {
        log: Arc<Mutex<Vec<NodeId>>>,
    }
    impl NodeExecutor for TestExecutor {
        fn execute(
            &self,
            node: &dyn Node,
            ctx: &mut dyn ExecContext,
        ) -> Result<(), EvalError> {
            self.log.lock().unwrap().push(ctx.current_node());
            if let Some(c) = node.downcast_ref::<ConstNode>() {
                ctx.set_output(OutId::new(ctx.current_node(), 0), Box::new(c.value))?;
                return Ok(());
            }
            if let Some(a) = node.downcast_ref::<AddNode>() {
                let in_id = xcal_node::InputSocketId::new(ctx.current_node(), 0);
                let v = *ctx
                    .peek_input(in_id)?
                    .downcast_ref::<f32>()
                    .ok_or_else(|| EvalError::new("input not f32", ctx.current_node(), None))?;
                ctx.set_output(OutId::new(ctx.current_node(), 0), Box::new(v + a.add))?;
                return Ok(());
            }
            if node.downcast_ref::<FailNode>().is_some() {
                return Err(EvalError::new(
                    "intentional failure",
                    ctx.current_node(),
                    Some(OutId::new(ctx.current_node(), 0)),
                ));
            }
            Err(EvalError::new(
                "no executor for node",
                ctx.current_node(),
                None,
            ))
        }
    }

    fn chain() -> (SimpleGraph, NodeId, NodeId, NodeId, Arc<Mutex<Vec<NodeId>>>) {
        let mut g = SimpleGraph::default();
        let f = g.add_node(Box::new(ConstNode::new(1.0, DirtyPolicy::Always)));
        let gnode = g.add_node(Box::new(AddNode::new(1.0, DirtyPolicy::Always)));
        let h = g.add_node(Box::new(AddNode::new(1.0, DirtyPolicy::Always)));
        g.quick_connect(OutId::new(f, 0), xcal_node::InputSocketId::new(gnode, 0))
            .unwrap();
        g.quick_connect(OutId::new(gnode, 0), xcal_node::InputSocketId::new(h, 0))
            .unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        (g, f, gnode, h, log)
    }

    #[test]
    fn lazy_eval_only_requested_subgraph() {
        let (g, f, _gnode, _h, log) = chain();
        let exec = TestExecutor { log: log.clone() };
        let mut eval = LazyPullEval::new(Box::new(KahnSorter::new()), EvalErrorPolicy::FailFast);
        // Request only f's output: g, h must NOT execute.
        eval.evaluate(&g, &exec, Some(OutId::new(f, 0))).unwrap();
        assert_eq!(log.lock().unwrap().as_slice(), &[f]);
    }

    #[test]
    fn lazy_eval_full_chain_computes_correctly() {
        let (g, f, gnode, h, log) = chain();
        let exec = TestExecutor { log: log.clone() };
        let mut eval = LazyPullEval::new(Box::new(KahnSorter::new()), EvalErrorPolicy::FailFast);
        eval.evaluate(&g, &exec, Some(OutId::new(h, 0))).unwrap();
        // 1 -> +1 -> +1 == 3
        let out = eval.cached_output(OutId::new(h, 0)).unwrap();
        assert_eq!(*out.downcast_ref::<f32>().unwrap(), 3.0);
        let _ = (f, gnode);
    }

    #[test]
    fn always_recomputes_every_pass() {
        let (g, f, _gnode, _h, log) = chain();
        let exec = TestExecutor { log: log.clone() };
        let mut eval = LazyPullEval::new(Box::new(KahnSorter::new()), EvalErrorPolicy::FailFast);
        eval.evaluate(&g, &exec, Some(OutId::new(f, 0))).unwrap();
        eval.evaluate(&g, &exec, Some(OutId::new(f, 0))).unwrap();
        assert_eq!(log.lock().unwrap().len(), 2); // f executed twice
    }

    #[test]
    fn never_computes_once() {
        let mut g = SimpleGraph::default();
        let n = g.add_node(Box::new(ConstNode::new(7.0, DirtyPolicy::Never)));
        let log = Arc::new(Mutex::new(Vec::new()));
        let exec = TestExecutor { log: log.clone() };
        let mut eval = LazyPullEval::new(Box::new(KahnSorter::new()), EvalErrorPolicy::FailFast);
        eval.evaluate(&g, &exec, Some(OutId::new(n, 0))).unwrap();
        eval.evaluate(&g, &exec, Some(OutId::new(n, 0))).unwrap();
        assert_eq!(log.lock().unwrap().len(), 1); // computed once, reused
    }

    #[test]
    fn resume_with_propagates_downstream() {
        let mut g = SimpleGraph::default();
        let fail = g.add_node(Box::new(FailNode::new()));
        let add = g.add_node(Box::new(AddNode::new(1.0, DirtyPolicy::Always)));
        g.quick_connect(OutId::new(fail, 0), xcal_node::InputSocketId::new(add, 0))
            .unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let exec = TestExecutor { log: log.clone() };
        let mut eval = LazyPullEval::new(Box::new(KahnSorter::new()), EvalErrorPolicy::FailFast);
        let mut err = eval
            .evaluate(&g, &exec, Some(OutId::new(add, 0)))
            .expect_err("fail node should error");
        // Recover: inject 42.0 at the failed output, continue.
        err.recovery().resume_with(Box::new(42.0f32));
        eval.apply_recovery(err, &g, &exec, Some(OutId::new(add, 0)))
            .unwrap();
        let out = eval.cached_output(OutId::new(add, 0)).unwrap();
        assert_eq!(*out.downcast_ref::<f32>().unwrap(), 43.0);
    }

    #[test]
    fn retry_recovers_after_fix() {
        // A fail node whose executor succeeds on the second attempt (state toggle).
        #[derive(Debug, Clone)]
        struct Flaky {
            meta: NodeMeta,
            outputs: Vec<OutputSocket>,
        }
        impl Node for Flaky {
            impl_as_any!();
            fn meta(&self) -> &NodeMeta {
                &self.meta
            }
            fn inputs(&self) -> &[InputSocket] {
                &[]
            }
            fn outputs(&self) -> &[OutputSocket] {
                &self.outputs
            }
            fn dirty_policy(&self) -> DirtyPolicy {
                DirtyPolicy::Always
            }
            fn state_type(&self) -> Box<dyn TypeTag> {
                Box::new(NamedTypeTag::new("u32"))
            }
        }
        struct FlakyExec {
            attempts: Arc<Mutex<u32>>,
        }
        impl NodeExecutor for FlakyExec {
            fn execute(&self, node: &dyn Node, ctx: &mut dyn ExecContext) -> Result<(), EvalError> {
                let n = *self.attempts.lock().unwrap();
                *self.attempts.lock().unwrap() += 1;
                if n == 0 {
                    return Err(EvalError::new(
                        "flaky first attempt",
                        ctx.current_node(),
                        Some(OutId::new(ctx.current_node(), 0)),
                    ));
                }
                ctx.set_output(OutId::new(ctx.current_node(), 0), Box::new(99.0f32))?;
                let _ = node;
                Ok(())
            }
        }

        let mut g = SimpleGraph::default();
        let f = g.add_node(Box::new(Flaky {
            meta: NodeMeta::new("test.flaky", "Flaky", 1),
            outputs: vec![OutputSocket::new(f32_tag()).with_index(0)],
        }));
        let attempts = Arc::new(Mutex::new(0u32));
        let exec = FlakyExec {
            attempts: attempts.clone(),
        };
        let mut eval = LazyPullEval::new(Box::new(KahnSorter::new()), EvalErrorPolicy::FailFast);
        let mut err = eval
            .evaluate(&g, &exec, Some(OutId::new(f, 0)))
            .expect_err("first attempt fails");
        err.recovery().retry();
        eval.apply_recovery(err, &g, &exec, Some(OutId::new(f, 0)))
            .unwrap();
        let out = eval.cached_output(OutId::new(f, 0)).unwrap();
        assert_eq!(*out.downcast_ref::<f32>().unwrap(), 99.0);
    }

    #[test]
    fn collect_all_gathers_errors() {
        // Two independent failing branches; CollectAll should record both.
        let mut g = SimpleGraph::default();
        let f1 = g.add_node(Box::new(FailNode::new()));
        let f2 = g.add_node(Box::new(FailNode::new()));
        let log = Arc::new(Mutex::new(Vec::new()));
        let exec = TestExecutor { log: log.clone() };
        let mut eval = LazyPullEval::new(Box::new(KahnSorter::new()), EvalErrorPolicy::CollectAll);
        let res = eval.evaluate(&g, &exec, None);
        // CollectAll returns Err (aggregated) when errors exist.
        assert!(res.is_err());
        // Both failing nodes were attempted.
        assert!(log.lock().unwrap().contains(&f1));
        assert!(log.lock().unwrap().contains(&f2));
    }

    #[test]
    fn continue_other_branches_keeps_going() {
        // f1 fails, f2 succeeds; with ContinueOtherBranches f2 still runs and
        // its output is cached.
        let mut g = SimpleGraph::default();
        let _f1 = g.add_node(Box::new(FailNode::new()));
        let f2 = g.add_node(Box::new(ConstNode::new(5.0, DirtyPolicy::Always)));
        let log = Arc::new(Mutex::new(Vec::new()));
        let exec = TestExecutor { log: log.clone() };
        let mut eval =
            LazyPullEval::new(Box::new(KahnSorter::new()), EvalErrorPolicy::ContinueOtherBranches);
        let res = eval.evaluate(&g, &exec, None);
        assert!(res.is_ok(), "ContinueOtherBranches should not hard-fail");
        let out = eval.cached_output(OutId::new(f2, 0)).unwrap();
        assert_eq!(*out.downcast_ref::<f32>().unwrap(), 5.0);
    }
}
