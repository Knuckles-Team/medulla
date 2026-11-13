//! Plan of `Discover` neighbour expansion (MEDULLA-FEDERATED-QUERY-R090.8).
//!
//! `expand_hops` (in `medulla-graph-core`, shared with corpus retrieval) walks the graph breadth-first from the top-k fused hits over
//! the caller's row-projected core, so a neighbour the caller may not read is
//! never reached. Each hop keeps at most `fanout` neighbours per frontier node,
//! ordered by edge weight descending then node id. A neighbour scores
//! `seed score x decay ^ hop`. Hits dedupe by id, keep the best path, and the
//! list truncates to `k`. The walk writes nothing.

pub(crate) use medulla_graph_core::graph::ExpandedHit;
use medulla_graph_core::graph::{expand_hops, GraphCore};

/// Plan a `Discover` result: the fused top-k seeds plus their neighbours.
///
/// `fanout` and `decay` come from the registered knobs `discover.hop_fanout`
/// and `discover.hop_decay`.
pub(crate) fn plan_discover(
    core: &GraphCore,
    seeds: Vec<(String, f32)>,
    hops: u32,
    k: usize,
) -> Vec<ExpandedHit> {
    if hops == 0 {
        return seeds
            .into_iter()
            .take(k)
            .map(|(id, score)| ExpandedHit {
                path: vec![id.clone()],
                id,
                score,
            })
            .collect();
    }
    let knobs = medulla_server_api::knob_config::knobs();
    let fanout = usize::try_from(knobs.discover_hop_fanout).unwrap_or(usize::MAX);
    expand_hops(core, &seeds, hops, fanout, knobs.discover_hop_decay, k)
}
