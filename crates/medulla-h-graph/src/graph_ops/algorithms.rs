use super::*;

use medulla_graph_algos::graph_algos::{
    analogy_matches, check_topology_graph, checked_mincut, hyperedge_scores, spectral_partition,
    synergy_bridges, AdjacencyGraph, HyperedgeError, LabeledGraph, QualityFunction,
    TopologyKernelError,
};
use medulla_h_graph_gateway::graph_ops::ops_context::GraphOpsContext;
use medulla_protocol_types::protocol::CommunityQualityFunction;
use medulla_protocol_types::result_contract::compute as results;
use medulla_query_types::topology::{
    AnalogyMatch, HyperedgeScore, Partition, SynergyBridge, TopologyAnalytic, TopologyEvidence,
    TopologyResult, TOPOLOGY_K_MAX, TOPOLOGY_K_MIN,
};
use medulla_result_types::compute_result::algorithms::{
    AlgorithmScopeView, CommunityDetectionView, NodeScoreView, PageRankView, ShortestPathView,
};

/// EH-314: wire (`medulla_protocol_types::protocol::CommunityQualityFunction`) → kernel
/// (`medulla_graph_algos::graph_algos::QualityFunction`) — the same split
/// `mining/insight.rs::to_community_algo` already uses for `MineCommunity`'s
/// `CommunityAlgorithm`, kept as an exhaustive `match` (not a `From` impl)
/// so a future third quality function fails to compile here rather than
/// silently falling through.
fn to_quality_function(quality: CommunityQualityFunction) -> QualityFunction {
    match quality {
        CommunityQualityFunction::Modularity => QualityFunction::Modularity,
        CommunityQualityFunction::Cpm => QualityFunction::Cpm,
    }
}

/// The topology an algorithm runs on, optionally restricted to a `node_ids` scope
/// (MEDULLA-FEDERATED-QUERY-R743.2). `core` is already the caller's row-visible projection
/// (`terminal::try_handle`), so intersecting the scope with it is the visibility limit: a node
/// the caller cannot see is absent from the view exactly as an unknown id is, and contributes
/// to neither the topology nor `in_scope`. `Some(empty)` scopes to nothing.
fn scoped_topology(
    core: &GraphCore,
    node_ids: Option<Vec<String>>,
) -> Result<
    (
        medulla_graph_core::graph::GraphView,
        Option<AlgorithmScopeView>,
    ),
    String,
> {
    let mut view = core.topology_snapshot();
    let Some(node_ids) = node_ids else {
        return Ok((view, None));
    };
    if node_ids.len() > MAX_BATCH_IDS {
        return Err(invalid_argument(&format!(
            "node_ids names {} ids (max {MAX_BATCH_IDS})",
            node_ids.len()
        )));
    }
    let requested: std::collections::HashSet<String> = node_ids.into_iter().collect();
    let in_scope = view.restrict_to_nodes(&requested) as u64;
    let scope = AlgorithmScopeView {
        requested: requested.len() as u64,
        in_scope,
    };
    Ok((view, Some(scope)))
}

/// A graph-algorithm refusal names the caller's input (an unknown node id, a
/// cyclic graph); keep any code the inner error already carries.
fn invalid_argument(error: &str) -> String {
    medulla_protocol_types::error_registry::classify_refusal("INVALID_ARGUMENT", "", error)
}

/// `InDegree`: pure extract-method from `try_handle`'s match arm, byte-identical
/// behaviour, no signature change.
fn handle_in_degree(req_id: u64, core: &Arc<GraphCore>, node_id: &str) -> Response {
    let g = &**core;
    match g.in_degree(node_id) {
        Ok(deg) => Response::ok(req_id, ResultPayload::Count(deg as u64)),
        Err(e) => Response::err(req_id, invalid_argument(&e)),
    }
}

/// `OutDegree`: pure extract-method from `try_handle`'s match arm, byte-identical
/// behaviour, no signature change.
fn handle_out_degree(req_id: u64, core: &Arc<GraphCore>, node_id: &str) -> Response {
    let g = &**core;
    match g.out_degree(node_id) {
        Ok(deg) => Response::ok(req_id, ResultPayload::Count(deg as u64)),
        Err(e) => Response::err(req_id, invalid_argument(&e)),
    }
}

/// `GetPredecessors`: pure extract-method from `try_handle`'s match arm,
/// byte-identical behaviour, no signature change.
fn handle_get_predecessors(req_id: u64, core: &Arc<GraphCore>, node_id: &str) -> Response {
    let g = &**core;
    match g.get_predecessors(node_id) {
        Ok(nodes) => Response::ok(req_id, ResultPayload::Ids(nodes)),
        Err(e) => Response::err(req_id, invalid_argument(&e)),
    }
}

/// `GetSuccessors`: pure extract-method from `try_handle`'s match arm,
/// byte-identical behaviour, no signature change.
fn handle_get_successors(req_id: u64, core: &Arc<GraphCore>, node_id: &str) -> Response {
    let g = &**core;
    match g.get_successors(node_id) {
        Ok(nodes) => Response::ok(req_id, ResultPayload::Ids(nodes)),
        Err(e) => Response::err(req_id, invalid_argument(&e)),
    }
}

/// `GetNeighbors`: pure extract-method from `try_handle`'s match arm,
/// byte-identical behaviour, no signature change.
fn handle_get_neighbors(req_id: u64, core: &Arc<GraphCore>, node_id: &str) -> Response {
    let g = &**core;
    match g.get_neighbors(node_id) {
        Ok(nodes) => Response::ok(req_id, ResultPayload::Ids(nodes)),
        Err(e) => Response::err(req_id, invalid_argument(&e)),
    }
}

/// `DegreeCentrality`: pure extract-method from `try_handle`'s match arm,
/// byte-identical behaviour, no signature change.
fn handle_degree_centrality(req_id: u64, core: &Arc<GraphCore>, node_id: &str) -> Response {
    let g = core.topology_snapshot();
    match medulla_graph_algos::algorithms::compute_degree_centrality(&g, node_id) {
        Ok(val) => Response::ok(
            req_id,
            ResultPayload::scalar::<results::DegreeCentrality>(val),
        ),
        Err(e) => Response::err(req_id, invalid_argument(&e)),
    }
}

/// `GetNeighborsBatch`: pure extract-method from `try_handle`'s match arm,
/// byte-identical behaviour, no signature change.
fn handle_get_neighbors_batch(
    req_id: u64,
    core: &Arc<GraphCore>,
    node_ids: Vec<String>,
) -> Response {
    if node_ids.len() > MAX_BATCH_IDS {
        return Response::err(
            req_id,
            format!(
                "INVALID_ARGUMENT: batch too large: {} ids (max {})",
                node_ids.len(),
                MAX_BATCH_IDS
            ),
        );
    }
    let g = &**core;
    // [node_id, Vec<neighbor_id>] in input order — one round-trip and one
    // topo-lock acquisition for N nodes (D-DPF-1) instead of N of each.
    let out = g.get_neighbors_batch(node_ids);
    Response::ok(
        req_id,
        ResultPayload::of_ref::<medulla_protocol_types::result_contract::graph::GetNeighborsBatch>(
            &out,
        ),
    )
}

/// `BetweennessCentrality`: pure extract-method from `try_handle`'s match arm,
/// byte-identical behaviour, no signature change.
async fn handle_betweenness_centrality(req_id: u64, core: &Arc<GraphCore>) -> Response {
    // O(V·E) Brandes — snapshot topology, compute off-lock (KG-2.51).
    let snap = { core.topology_snapshot() };
    match compute_off_lock(req_id, move || {
        medulla_graph_algos::algorithms::betweenness_centrality(&snap)
    })
    .await
    {
        Ok(v) => Response::ok(
            req_id,
            ResultPayload::of::<results::BetweennessCentrality>(v),
        ),
        Err(resp) => resp,
    }
}

/// `PersonalizedPageRank`: pure extract-method from `try_handle`'s match arm,
/// byte-identical behaviour, no signature change.
async fn handle_personalized_page_rank(
    req_id: u64,
    core: &Arc<GraphCore>,
    seed_nodes: Vec<(String, f64)>,
    damping: f64,
    iterations: usize,
) -> Response {
    // O(iterations·E) — snapshot topology, compute off-lock (KG-2.51).
    let snap = { core.topology_snapshot() };
    match compute_off_lock(req_id, move || {
        medulla_graph_algos::algorithms::personalized_pagerank(
            &snap,
            &seed_nodes,
            damping,
            iterations,
        )
    })
    .await
    {
        Ok(v) => Response::ok(
            req_id,
            ResultPayload::of::<results::PersonalizedPageRank>(v),
        ),
        Err(resp) => resp,
    }
}

/// `CommunityDetection`, optionally scoped by `node_ids` (MEDULLA-FEDERATED-QUERY-R743.2).
async fn handle_community_detection(
    req_id: u64,
    core: &Arc<GraphCore>,
    resolution: f64,
    node_ids: Option<Vec<String>>,
) -> Response {
    // Louvain — NOT label propagation, as this comment used to claim: the method
    // routes to `algorithms::community_detection`, which is the Louvain kernel,
    // and so did the hand-rolled `community_detection` that carried the original
    // `COMMUNITY_DETECTION_BUDGET`. That 15s wall-clock budget now lives in the
    // kernel loop itself (`LouvainConfig::budget`), which is the only place that
    // can actually STOP the work — a handler-side timeout would bound the
    // response while the compute thread kept burning CPU. Commit `a14b9c28`
    // deleted the budget along with the duplicate kernel and left this comment
    // describing a bound that no longer existed.
    //
    // That budget used to burn entirely UNDER the read lock, stalling every
    // writer on the graph. Snapshot topology, compute off-lock (KG-2.51).
    let (snap, scope) = match scoped_topology(core, node_ids) {
        Ok(scoped) => scoped,
        Err(error) => return Response::err(req_id, error),
    };
    match compute_off_lock(req_id, move || {
        medulla_graph_algos::algorithms::community_detection(&snap, resolution)
    })
    .await
    {
        Ok(communities) => Response::ok(
            req_id,
            ResultPayload::of::<results::CommunityDetection>(CommunityDetectionView {
                communities,
                scope,
            }),
        ),
        Err(resp) => resp,
    }
}

/// `ComputeSimilarityEdges`: pure extract-method from `try_handle`'s match arm,
/// byte-identical behaviour, no signature change.
async fn handle_compute_similarity_edges(
    req_id: u64,
    core: &Arc<GraphCore>,
    threshold: f64,
) -> Response {
    // O(V²·d) all-pairs cosine on rayon — must never run under the
    // graph lock OR on the tokio runtime threads. Snapshot the
    // property blobs it reads, compute off-lock (KG-2.51).
    let snap = { core.analysis_snapshot() };
    match compute_off_lock(req_id, move || {
        medulla_graph_algos::algorithms::compute_similarity_edges(&snap, threshold)
    })
    .await
    {
        Ok(v) => Response::ok(
            req_id,
            ResultPayload::of::<results::ComputeSimilarityEdges>(v),
        ),
        Err(resp) => resp,
    }
}

/// `ResolveCandidates`: pure extract-method from `try_handle`'s match arm,
/// byte-identical behaviour, no signature change.
async fn handle_resolve_candidates(
    req_id: u64,
    core: &Arc<GraphCore>,
    sim_threshold: f64,
    merge_threshold: f64,
    node_type: Option<String>,
) -> Response {
    // Entity-resolution candidate generation (KG-2.260): embedding
    // similarity + clustering composed into one READ/propose op. Same
    // off-lock discipline as ComputeSimilarityEdges (it shares the O(V²)
    // cosine pass). Returns merge proposals; never mutates the graph.
    let snap = { core.analysis_snapshot() };
    match compute_off_lock(req_id, move || {
        medulla_graph_algos::algorithms::resolve_candidates(
            &snap,
            sim_threshold,
            merge_threshold,
            node_type.as_deref(),
        )
    })
    .await
    {
        Ok(v) => Response::ok(
            req_id,
            ResultPayload::of::<results::ResolveCandidates>(
                v.into_iter()
                    .map(|proposal| results::MergeProposal {
                        canonical: proposal.canonical,
                        members: proposal.members,
                        score: proposal.score,
                        kind: proposal.kind,
                    })
                    .collect(),
            ),
        ),
        Err(resp) => resp,
    }
}

/// `TopologicalSort`: pure extract-method from `try_handle`'s match arm,
/// byte-identical behaviour, no signature change.
fn handle_topological_sort(req_id: u64, core: &Arc<GraphCore>) -> Response {
    let g = core.topology_snapshot();
    match medulla_graph_algos::algorithms::topological_sort(&g) {
        Ok(order) => Response::ok(req_id, ResultPayload::of::<results::TopologicalSort>(order)),
        Err(e) => Response::err(req_id, format!("INVALID_ARGUMENT: {e}")),
    }
}

/// `PageRank`, optionally scoped by `node_ids` (MEDULLA-FEDERATED-QUERY-R743.2).
async fn handle_page_rank(
    req_id: u64,
    core: &Arc<GraphCore>,
    damping: f64,
    iterations: usize,
    node_ids: Option<Vec<String>>,
) -> Response {
    // O(iterations·E) — snapshot topology, compute off-lock (KG-2.51).
    let (snap, scope) = match scoped_topology(core, node_ids) {
        Ok(scoped) => scoped,
        Err(error) => return Response::err(req_id, error),
    };
    match compute_off_lock(req_id, move || {
        medulla_graph_algos::algorithms::pagerank(&snap, damping, iterations)
    })
    .await
    {
        Ok(ranked) => Response::ok(
            req_id,
            ResultPayload::of::<results::PageRank>(PageRankView {
                scores: ranked
                    .into_iter()
                    .map(|(node_id, score)| NodeScoreView { node_id, score })
                    .collect(),
                scope,
            }),
        ),
        Err(resp) => resp,
    }
}

/// `MinimumSpanningTree`: pure extract-method from `try_handle`'s match arm,
/// byte-identical behaviour, no signature change.
async fn handle_minimum_spanning_tree(req_id: u64, core: &Arc<GraphCore>) -> Response {
    // O(E log E) + per-edge JSON weight parsing — snapshot (incl. the
    // edge property blobs it reads), compute off-lock (KG-2.51).
    let snap = { core.analysis_snapshot() };
    match compute_off_lock(req_id, move || {
        medulla_graph_algos::algorithms::minimum_spanning_tree(&snap)
    })
    .await
    {
        Ok(v) => Response::ok(req_id, ResultPayload::of::<results::MinimumSpanningTree>(v)),
        Err(resp) => resp,
    }
}

/// `Metrics`: pure extract-method from `try_handle`'s match arm, byte-identical
/// behaviour, no signature change.
async fn handle_metrics(req_id: u64, core: &Arc<GraphCore>, raw_ledger_len: u64) -> Response {
    // Parses every node's property JSON — memcpy snapshot under the
    // lock is cheaper than O(V) JSON parsing under it (KG-2.51). The
    // ledger is not snapshotted; `raw_ledger_len` (captured off the
    // UN-projected core, above) is the total_mutations field — the
    // projected `core` in scope here carries no ledger at all (see the
    // comment above `project_core`).
    let snap = core.analysis_snapshot();
    let ledger_len = raw_ledger_len;
    let m = match compute_off_lock(req_id, move || {
        let mut m = medulla_graph_algos::algorithms::compute_metrics(&snap);
        m.total_mutations = ledger_len;
        m
    })
    .await
    {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    Response::ok(
        req_id,
        ResultPayload::of::<medulla_protocol_types::result_contract::graph::Metrics>(m),
    )
}

/// `CommunityDetectEphemeral` (EH-314): builds the weighted `AdjacencyGraph`
/// directly from the caller-supplied `(source, target, weight)` edges and the
/// selected quality function — no scratch `GraphCore` round-trip (see
/// `community_detection_weighted`'s own doc for why that round-trip could
/// never have carried a weight anyway).
async fn handle_community_detect_ephemeral(
    req_id: u64,
    node_ids: Vec<String>,
    edges: Vec<(String, String, f64)>,
    resolution: f64,
    quality: CommunityQualityFunction,
) -> Response {
    let quality = to_quality_function(quality);
    match compute_off_lock(req_id, move || {
        medulla_graph_algos::algorithms::community_detection_weighted(
            node_ids, edges, resolution, quality,
        )
    })
    .await
    {
        Ok(v) => Response::ok(
            req_id,
            ResultPayload::of::<results::CommunityDetectEphemeral>(v),
        ),
        Err(resp) => resp,
    }
}

/// Everything one `MineTopology` run needs: the request fields and the knob
/// values the handler resolved for it.
struct TopologyParams {
    analytic: TopologyAnalytic,
    label: Option<String>,
    relationship: Option<String>,
    k: u32,
    seed: u64,
    candidates: Vec<Vec<String>>,
    analogy_min: f64,
    max_candidates: usize,
    max_nodes: u64,
    max_edges: u64,
}

fn topology_kernel_refusal(error: TopologyKernelError) -> String {
    match error {
        TopologyKernelError::TrivialGraph => {
            "TOPOLOGY_TRIVIAL_GRAPH: a topology analytic needs at least 3 nodes".to_string()
        }
        TopologyKernelError::InvalidWeight => {
            "TOPOLOGY_PARAM_INVALID: an edge weight is not finite and positive".to_string()
        }
        TopologyKernelError::InvalidParam(name) => {
            format!("TOPOLOGY_PARAM_INVALID: parameter `{name}` is out of range")
        }
    }
}

/// A node's label: its stored `type`, `node_type` or `label`, else the empty string.
fn topology_node_label(view: &medulla_graph_core::graph::GraphView, id: &str) -> String {
    let object = view.node_row_object(id).unwrap_or_default();
    ["type", "node_type", "label"]
        .iter()
        .filter_map(|key| object.get(*key).and_then(serde_json::Value::as_str))
        .find(|value| !value.is_empty())
        .unwrap_or("")
        .to_string()
}

/// The relationship an edge blob stores (else `related`) and its `weight`
/// (else 1). A stored weight that is not a finite positive number is passed
/// through as it is, so the kernel check refuses it (MEDULLA-FEDERATED-QUERY-R607.4).
fn topology_edge(blob: &[u8]) -> (String, f64) {
    match medulla_types::msgpack::decode_property_value(blob) {
        Ok(serde_json::Value::Object(props)) => {
            let relation = ["relationship", "type", "label"]
                .iter()
                .filter_map(|key| props.get(*key).and_then(serde_json::Value::as_str))
                .find(|value| !value.is_empty())
                .unwrap_or("related")
                .to_string();
            let weight = match props.get("weight") {
                None => 1.0,
                Some(value) => value.as_f64().unwrap_or(f64::NAN),
            };
            (relation, weight)
        }
        _ => ("related".to_string(), 1.0),
    }
}

/// Run one topology analytic over the (already row-projected) snapshot. The
/// kernels live in `medulla_graph_algos::graph_algos`; this only projects the
/// view onto them and shapes their output.
fn mine_topology(
    view: &medulla_graph_core::graph::GraphView,
    params: &TopologyParams,
) -> Result<TopologyResult, String> {
    let started = std::time::Instant::now();
    if !(TOPOLOGY_K_MIN..=TOPOLOGY_K_MAX).contains(&params.k) {
        return Err(format!(
            "TOPOLOGY_PARAM_INVALID: k {} is outside [{TOPOLOGY_K_MIN}, {TOPOLOGY_K_MAX}]",
            params.k
        ));
    }
    let mut node_labels: std::collections::BTreeMap<String, String> = Default::default();
    for id in view.node_properties.keys() {
        let label = topology_node_label(view, id);
        if params
            .label
            .as_deref()
            .is_some_and(|wanted| wanted != label)
        {
            continue;
        }
        node_labels.insert(id.clone(), label);
    }
    if node_labels.len() as u64 > params.max_nodes {
        return Err(format!(
            "TOPOLOGY_GRAPH_TOO_LARGE: {} nodes exceed topology.max_nodes {}",
            node_labels.len(),
            params.max_nodes
        ));
    }
    let mut weighted_edges: Vec<(String, String, String, f64)> = Vec::new();
    for ((from, to), blobs) in &view.edge_properties {
        if !node_labels.contains_key(from) || !node_labels.contains_key(to) {
            continue;
        }
        for blob in blobs.iter() {
            let (relation, weight) = topology_edge(blob);
            if params
                .relationship
                .as_deref()
                .is_some_and(|wanted| wanted != relation)
            {
                continue;
            }
            weighted_edges.push((from.clone(), to.clone(), relation, weight));
        }
    }
    // The view's maps iterate in hash order; sort so one graph always yields one input.
    weighted_edges.sort_by(|a, b| (&a.0, &a.1, &a.2).cmp(&(&b.0, &b.1, &b.2)));
    let labeled_edges: Vec<(String, String, String)> = weighted_edges
        .iter()
        .map(|(from, to, relation, _)| (from.clone(), to.clone(), relation.clone()))
        .collect();
    if labeled_edges.len() as u64 > params.max_edges {
        return Err(format!(
            "TOPOLOGY_GRAPH_TOO_LARGE: {} edges exceed topology.max_edges {}",
            labeled_edges.len(),
            params.max_edges
        ));
    }
    let mut outgoing: std::collections::BTreeMap<String, Vec<(String, f64)>> = node_labels
        .keys()
        .map(|id| (id.clone(), Vec::new()))
        .collect();
    for (from, to, _, weight) in &weighted_edges {
        if let Some(list) = outgoing.get_mut(from) {
            list.push((to.clone(), *weight));
        }
    }
    let graph = AdjacencyGraph::from_adjacency(outgoing);
    check_topology_graph(&graph).map_err(topology_kernel_refusal)?;
    let k = params.k as usize;
    let mut result = TopologyResult {
        analytic: params.analytic,
        partitions: Vec::new(),
        matches: Vec::new(),
        bridges: Vec::new(),
        scores: Vec::new(),
        evidence: TopologyEvidence {
            nodes: node_labels.len() as u64,
            edges: labeled_edges.len() as u64,
            seed: params.seed,
            ..TopologyEvidence::default()
        },
    };
    match params.analytic {
        TopologyAnalytic::Spectral => {
            let spectral =
                spectral_partition(&graph, k, params.seed).map_err(topology_kernel_refusal)?;
            result.partitions = spectral
                .partitions
                .into_iter()
                .enumerate()
                .map(|(rank, members)| Partition {
                    id: rank.to_string(),
                    members,
                })
                .collect();
            result.evidence.modularity = Some(spectral.modularity);
            result.evidence.iterations = spectral.iterations;
        }
        TopologyAnalytic::Mincut => {
            let cut = checked_mincut(&graph).map_err(topology_kernel_refusal)?;
            result.partitions = vec![
                Partition {
                    id: "0".to_string(),
                    members: cut.left,
                },
                Partition {
                    id: "1".to_string(),
                    members: cut.right,
                },
            ];
            result.evidence.cut_weight = Some(cut.cut_weight);
        }
        TopologyAnalytic::Analogy => {
            let labeled = LabeledGraph {
                nodes: node_labels.into_iter().collect(),
                edges: labeled_edges,
            };
            result.matches = analogy_matches(&labeled, params.analogy_min, k)
                .into_iter()
                .map(|pair| AnalogyMatch {
                    a: pair.a,
                    b: pair.b,
                    score: pair.score,
                })
                .collect();
            result.evidence.match_scores = result.matches.iter().map(|m| m.score).collect();
        }
        TopologyAnalytic::Synergy => {
            let (bridges, _communities) = synergy_bridges(&graph, &node_labels, k, params.seed);
            result.bridges = bridges
                .into_iter()
                .map(|bridge| SynergyBridge {
                    node: bridge.node,
                    communities: bridge.communities.iter().map(usize::to_string).collect(),
                    score: bridge.score,
                })
                .collect();
        }
        TopologyAnalytic::HyperedgeScore => {
            let scores = hyperedge_scores(&graph, &params.candidates, params.max_candidates)
                .map_err(|error| match error {
                    HyperedgeError::TooManyCandidates => format!(
                        "TOPOLOGY_PARAM_INVALID: {} candidate sets exceed topology.max_candidates {}",
                        params.candidates.len(),
                        params.max_candidates
                    ),
                    HyperedgeError::UnknownNode(id) => {
                        format!("TOPOLOGY_PARAM_INVALID: candidate node `{id}` is not in the graph")
                    }
                })?;
            result.scores = params
                .candidates
                .iter()
                .cloned()
                .zip(scores)
                .map(|(members, score)| HyperedgeScore { members, score })
                .collect();
        }
    }
    result
        .validate()
        .map_err(|error| format!("TOPOLOGY_PARAM_INVALID: {error}"))?;
    result.evidence.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(result)
}

/// `MineTopology` (MEDULLA-FEDERATED-QUERY-R607.3): snapshots the row-projected
/// core like `CommunityDetectEphemeral` and runs the analytic off-lock.
async fn handle_mine_topology(
    req_id: u64,
    core: &Arc<GraphCore>,
    params: TopologyParams,
) -> Response {
    let snap = core.analysis_snapshot();
    let analytic = params.analytic;
    match compute_off_lock(req_id, move || mine_topology(&snap, &params)).await {
        Ok(Ok(result)) => {
            topology_span(analytic, Ok(&result.evidence));
            Response::ok(req_id, ResultPayload::of::<results::MineTopology>(result))
        }
        Ok(Err(message)) => {
            topology_span(analytic, Err(&message));
            Response::err(req_id, message)
        }
        Err(resp) => resp,
    }
}

/// The `medulla.topology.<analytic>` span (MEDULLA-FEDERATED-QUERY-R607.5): a
/// run records the evidence counts with outcome `ok`; a refusal records
/// outcome `refused` and its code. One event inside the span carries it to the
/// subscriber.
fn topology_span(analytic: TopologyAnalytic, outcome: Result<&TopologyEvidence, &String>) {
    macro_rules! span_for {
        ($name:literal) => {
            tracing::info_span!(
                $name,
                analytic = analytic.as_str(),
                outcome = tracing::field::Empty,
                code = tracing::field::Empty,
                nodes = tracing::field::Empty,
                edges = tracing::field::Empty,
                iterations = tracing::field::Empty,
                elapsed_ms = tracing::field::Empty,
                seed = tracing::field::Empty,
            )
        };
    }
    let span = match analytic {
        TopologyAnalytic::Spectral => span_for!("medulla.topology.spectral"),
        TopologyAnalytic::Mincut => span_for!("medulla.topology.mincut"),
        TopologyAnalytic::Analogy => span_for!("medulla.topology.analogy"),
        TopologyAnalytic::Synergy => span_for!("medulla.topology.synergy"),
        TopologyAnalytic::HyperedgeScore => span_for!("medulla.topology.hyperedge_score"),
    };
    match outcome {
        Ok(evidence) => {
            span.record("outcome", "ok");
            span.record("nodes", evidence.nodes);
            span.record("edges", evidence.edges);
            span.record("iterations", evidence.iterations);
            span.record("elapsed_ms", evidence.elapsed_ms);
            span.record("seed", evidence.seed);
        }
        Err(message) => {
            span.record("outcome", "refused");
            span.record("code", message.split(':').next().unwrap_or_default());
        }
    }
    span.in_scope(|| tracing::info!("topology analytic finished"));
}

/// Handle graph algorithms and metrics.
pub(super) async fn try_handle_graph_algorithms(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let GraphOpsContext {
        req_id,
        core,
        raw_ledger_len,
        ..
    } = ctx;
    ControlFlow::Break(match method {
        Method::TopologicalSort => handle_topological_sort(req_id, core),
        Method::FindCycle => {
            let g = core.topology_snapshot();
            Response::ok(
                req_id,
                ResultPayload::of::<results::FindCycle>(
                    medulla_graph_algos::algorithms::find_cycle(&g),
                ),
            )
        }
        Method::GetShortestPath {
            source_id,
            target_id,
            node_ids,
        } => match scoped_topology(core, node_ids) {
            Ok((g, scope)) => Response::ok(
                req_id,
                ResultPayload::of::<results::GetShortestPath>(ShortestPathView {
                    path: medulla_graph_algos::algorithms::get_shortest_path(
                        &g, &source_id, &target_id,
                    ),
                    scope,
                }),
            ),
            Err(error) => Response::err(req_id, error),
        },
        Method::PageRank {
            damping,
            iterations,
            node_ids,
        } => handle_page_rank(req_id, core, damping, iterations, node_ids).await,
        Method::ConnectedComponents => {
            let g = core.topology_snapshot();
            Response::ok(
                req_id,
                ResultPayload::of::<results::ConnectedComponents>(
                    medulla_graph_algos::algorithms::connected_components(&g),
                ),
            )
        }
        Method::StronglyConnectedComponents => {
            let g = core.topology_snapshot();
            Response::ok(
                req_id,
                ResultPayload::of::<results::StronglyConnectedComponents>(
                    medulla_graph_algos::algorithms::strongly_connected_components(&g),
                ),
            )
        }
        Method::MinimumSpanningTree => handle_minimum_spanning_tree(req_id, core).await,
        Method::Metrics => handle_metrics(req_id, core, raw_ledger_len).await,
        // DecaySweep/TouchNodes/FromMsgpack/Reconcile (CONCEPT:MEDULLA-P0-2
        // bypass guard, L11): GATEWAY_ROUTED — see the AddNode/RemoveNode
        // comment above. `ToMsgpack` is a pure read and keeps its normal arm.
        other => return ControlFlow::Continue(other),
    })
}

/// Handle graph neighbor queries.
pub(super) async fn try_handle_neighbor_queries(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let GraphOpsContext { req_id, core, .. } = ctx;
    ControlFlow::Break(match method {
        Method::InDegree { node_id } => handle_in_degree(req_id, core, &node_id),
        Method::OutDegree { node_id } => handle_out_degree(req_id, core, &node_id),
        Method::GetPredecessors { node_id } => handle_get_predecessors(req_id, core, &node_id),
        Method::GetSuccessors { node_id } => handle_get_successors(req_id, core, &node_id),
        Method::GetNeighbors { node_id } => handle_get_neighbors(req_id, core, &node_id),
        Method::GetNeighborsBatch { node_ids } => {
            handle_get_neighbors_batch(req_id, core, node_ids)
        }
        other => return ControlFlow::Continue(other),
    })
}

/// Handle centrality and blast-radius algorithms.
pub(super) async fn try_handle_centrality_algorithms(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let GraphOpsContext { req_id, core, .. } = ctx;
    ControlFlow::Break(match method {
        Method::GetBlastRadius { node_id, max_depth } => {
            let g = core.topology_snapshot();
            Response::ok(
                req_id,
                ResultPayload::of::<results::GetBlastRadius>(
                    medulla_graph_algos::algorithms::get_blast_radius(&g, &node_id, max_depth),
                ),
            )
        }
        Method::DegreeCentrality { node_id } => handle_degree_centrality(req_id, core, &node_id),
        Method::DegreeCentralityAll => {
            let g = core.topology_snapshot();
            Response::ok(
                req_id,
                ResultPayload::of::<results::DegreeCentralityAll>(
                    medulla_graph_algos::algorithms::degree_centrality_all(&g),
                ),
            )
        }
        Method::BetweennessCentrality => handle_betweenness_centrality(req_id, core).await,
        Method::PersonalizedPageRank {
            seed_nodes,
            damping,
            iterations,
        } => handle_personalized_page_rank(req_id, core, seed_nodes, damping, iterations).await,
        other => return ControlFlow::Continue(other),
    })
}

/// Handle community and similarity algorithms.
pub(super) async fn try_handle_community_algorithms(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let GraphOpsContext { req_id, core, .. } = ctx;
    ControlFlow::Break(match method {
        Method::CommunityDetection {
            resolution,
            node_ids,
        } => handle_community_detection(req_id, core, resolution, node_ids).await,
        // Stateless community detection over an inline call graph — no tenant load,
        // no persistence, no graph lock. Builds a throwaway in-memory graph from the
        // passed nodes/edges and runs detection off-reactor. Replaces the prior
        // "bulk-load ~160k edges into a scratch tenant, detect, delete tenant"
        // round-trip (the dominant ingest community cost + the tenant-sprawl source).
        Method::CommunityDetectEphemeral {
            node_ids,
            edges,
            resolution,
            quality,
        } => handle_community_detect_ephemeral(req_id, node_ids, edges, resolution, quality).await,
        Method::MineTopology {
            analytic,
            label,
            relationship,
            k,
            seed,
            candidates,
        } => {
            let knobs = medulla_server_api::knob_config::knobs();
            let params = TopologyParams {
                analytic,
                label,
                relationship,
                k,
                seed,
                candidates,
                analogy_min: knobs.topology_analogy_min,
                max_candidates: knobs.topology_max_candidates as usize,
                max_nodes: knobs.topology_max_nodes,
                max_edges: knobs.topology_max_edges,
            };
            handle_mine_topology(req_id, core, params).await
        }
        // GraphColoring: greedy coloring is a single O(V+E) sweep over a cheap
        // topology snapshot (Phase C-B: read algorithms take an unlocked view).
        Method::GraphColoring => {
            let g = core.topology_snapshot();
            Response::ok(
                req_id,
                ResultPayload::of::<results::GraphColoring>(
                    medulla_graph_algos::algorithms::graph_coloring(&g),
                ),
            )
        }
        Method::ComputeSimilarityEdges { threshold } => {
            handle_compute_similarity_edges(req_id, core, threshold).await
        }
        Method::ResolveCandidates {
            sim_threshold,
            merge_threshold,
            node_type,
        } => {
            handle_resolve_candidates(req_id, core, sim_threshold, merge_threshold, node_type).await
        }
        // ── VIZ-1: hierarchical cluster-tree RPCs (CONCEPT:MEDULLA-KG.compute.leiden-hierarchy) ──
        // Same off-lock discipline as CommunityDetection/ResolveCandidates above —
        // `analysis_snapshot` under the topo READ lock, the actual clustering runs
        // on the blocking pool. See `Method::ClusterHierarchyRefresh`'s doc for why
        // the persisted result is NOT a graph mutation (no GATEWAY_ROUTED routing,
        // no WAL/CDC/audit — a plain durable side-cache keyed by graph name).
        other => return ControlFlow::Continue(other),
    })
}
