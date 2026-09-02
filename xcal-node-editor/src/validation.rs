//! Connection validation: socket type compatibility and cycle detection.
//!
//! Cycle detection needs a mutable graph for a *trial* connect: the proposed
//! edge is inserted, [`TarjanSccDetector`] runs, and the trial is immediately
//! rolled back. Existing edges on the target input (single-connection sockets
//! would otherwise reject the trial) are detached and restored afterwards.

use xcal_graph_algo::TarjanSccDetector;
use xcal_node::{
    ConnectError, CycleDetector, EdgeData, Graph, InputSocketId, OutputSocketId, UnitEdge,
};

/// Stateless validator: all methods are associated functions taking the
/// graph under inspection.
pub struct ConnectionValidator;

impl ConnectionValidator {
    /// Check that the output's type tag can feed the input's tag, using the
    /// graph's authoritative [`xcal_node::TypeCast`].
    pub fn check_types(
        graph: &dyn Graph,
        from: OutputSocketId,
        to: InputSocketId,
    ) -> Result<(), ConnectError> {
        let out_desc = {
            let node = graph
                .get_node(from.node())
                .ok_or(ConnectError::SourceNotFound(from))?;
            node.outputs()
                .get(from.index() as usize)
                .ok_or(ConnectError::SourceNotFound(from))?
        };
        let in_desc = {
            let node = graph
                .get_node(to.node())
                .ok_or(ConnectError::TargetNotFound(to))?;
            node.inputs()
                .get(to.index() as usize)
                .ok_or(ConnectError::TargetNotFound(to))?
        };

        if graph
            .type_cast()
            .is_compatible(out_desc.type_tag.as_ref(), in_desc.type_tag.as_ref())
        {
            Ok(())
        } else {
            Err(ConnectError::TypeIncompatible {
                from: out_desc.type_tag.type_name().to_string(),
                to: in_desc.type_tag.type_name().to_string(),
            })
        }
    }

    /// Trial-connect and run SCC detection, rolling the graph back to its
    /// exact prior state before returning. Existing edges on the target input
    /// are detached for the trial and restored.
    pub fn check_no_cycle(
        graph: &mut dyn Graph,
        from: OutputSocketId,
        to: InputSocketId,
    ) -> Result<(), ConnectError> {
        // Detach and save any edges already feeding the target input.
        let saved: Vec<(OutputSocketId, Box<dyn EdgeData>)> = graph
            .upstream(to)
            .into_iter()
            .filter_map(|src| graph.disconnect(src, to).map(|data| (src, data)))
            .collect();

        let result = match graph.connect_with_data(from, to, Box::new(UnitEdge)) {
            Ok(()) => {
                let cyclic = TarjanSccDetector::new().detect(&*graph).is_some();
                graph.disconnect(from, to);
                if cyclic {
                    Err(ConnectError::CycleDetected)
                } else {
                    Ok(())
                }
            }
            Err(e) => Err(e),
        };

        // Restore pre-existing edges no matter how the trial ended.
        for (src, data) in saved {
            let _ = graph.connect_with_data(src, to, data);
        }

        result
    }
}
