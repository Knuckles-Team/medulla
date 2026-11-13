//! `Method::SynthesizeSearchTask` (MEDULLA-FEDERATED-QUERY-R702.3): build one
//! multi-hop search task around an answer node from the evidence the caller may
//! read, diagnose its shortcut risks and refuse a risky task.
//!
//! The handler reads the snapshot the one served read chokepoint hands out: the
//! caller's row-level-security view narrowed to the verified tenant and visible
//! units by `ReadScope::narrow`. Visibility is settled before the evidence
//! neighbourhood is walked, so a node the caller cannot read leaves no clue, no
//! edge and no count in the answer, and an answer node that is hidden reads like
//! one that is absent. The neighbourhood is bounded by the `search_task.*` knobs,
//! `medulla_plan::search_task::synthesize` builds and refines the task off-lock,
//! and `RiskReport::check` refuses it by detector name. Nothing here writes: the
//! caller turns an accepted task into a pinned query-set item and sends it to
//! `DecisionEval` (MEDULLA-DECISION-ENGINE-R100.100.2).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::segment_filter::ReadScope;
use super::sql_read::sql_read_snapshot;
use super::*;
use medulla_graph_core::graph::GraphView;
use medulla_plan::search_task::synthesize;
use medulla_protocol_types::result_contract::query::SynthesizeSearchTask;
use medulla_server_api::knob_config::knobs;
use medulla_work_types::search_task::{
    EvidenceEdge, EvidenceGraph, EvidenceNode, RiskThresholds, SynthesizeSearchTaskRequest,
    SynthesizeSearchTaskResult, SEARCH_TASK_INVALID, SEARCH_TASK_SCHEMA_VERSION,
};

/// The evidence neighbourhood is larger than `search_task.max_evidence_nodes`.
const SEARCH_TASK_TOO_LARGE: &str = "SEARCH_TASK_TOO_LARGE";
/// The node variable the read scope binds its tenant predicate to.
const SCOPE_VARIABLE: &str = "evidence";
/// The relation an edge without a stored relationship carries.
const UNTYPED_RELATION: &str = "related";

fn refuse(req_id: u64, message: String) -> Response {
    Response::err(req_id, message)
}

fn text_of<'a>(object: &'a serde_json::Map<String, serde_json::Value>, key: &str) -> &'a str {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

/// The first non-empty of `keys`, else `""`.
fn first_text<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> &'a str {
    keys.iter()
        .map(|key| text_of(object, key))
        .find(|value| !value.is_empty())
        .unwrap_or("")
}

/// Every undirected neighbour of every node, from the edges of `view` whose two
/// ends are both stored nodes of the (already narrowed) snapshot.
fn adjacency(view: &GraphView) -> BTreeMap<&str, BTreeSet<&str>> {
    let mut adjacent: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (from, to) in view.edge_properties.keys() {
        if view.node_properties.contains_key(from) && view.node_properties.contains_key(to) {
            adjacent
                .entry(from.as_str())
                .or_default()
                .insert(to.as_str());
            adjacent
                .entry(to.as_str())
                .or_default()
                .insert(from.as_str());
        }
    }
    adjacent
}

/// The evidence subgraph within `hops` of `answer`: every reachable node with
/// its label (name, label or type, else its id) and text, and every stored
/// edge between two of them with its relationship, in id order.
fn evidence_graph(
    view: &GraphView,
    answer: &str,
    hops: u32,
    max_nodes: u64,
) -> Result<EvidenceGraph, String> {
    let adjacent = adjacency(view);
    let mut reached: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<(&str, u32)> = VecDeque::new();
    reached.insert(answer);
    queue.push_back((answer, 0));
    while let Some((current, depth)) = queue.pop_front() {
        if depth >= hops {
            continue;
        }
        for next in adjacent.get(current).into_iter().flatten().copied() {
            if reached.insert(next) {
                if reached.len() as u64 > max_nodes {
                    return Err(format!(
                        "{SEARCH_TASK_TOO_LARGE}: the evidence within {hops} hops of `{answer}` exceeds {max_nodes} nodes"
                    ));
                }
                queue.push_back((next, depth + 1));
            }
        }
    }
    let nodes = reached
        .iter()
        .map(|id| {
            let object = view.node_row_object(id).unwrap_or_default();
            EvidenceNode {
                id: (*id).to_string(),
                label: match first_text(&object, &["name", "label", "type", "node_type"]) {
                    "" => (*id).to_string(),
                    label => label.to_string(),
                },
                text: first_text(&object, &["text", "name"]).to_string(),
            }
        })
        .collect();
    let mut edges: BTreeSet<(String, String, String)> = BTreeSet::new();
    for ((from, to), blobs) in &view.edge_properties {
        if !reached.contains(from.as_str()) || !reached.contains(to.as_str()) {
            continue;
        }
        for blob in blobs.iter() {
            let relation = match medulla_types::msgpack::decode_property_value(blob) {
                Ok(serde_json::Value::Object(props)) => {
                    match first_text(&props, &["relationship", "type", "label"]) {
                        "" => UNTYPED_RELATION.to_string(),
                        relation => relation.to_string(),
                    }
                }
                _ => UNTYPED_RELATION.to_string(),
            };
            edges.insert((from.clone(), to.clone(), relation));
        }
    }
    Ok(EvidenceGraph {
        nodes,
        edges: edges
            .into_iter()
            .map(|(from, to, relation)| EvidenceEdge { from, to, relation })
            .collect(),
    })
}

/// The thresholds one served synthesis judges a task against, from the knobs.
fn thresholds() -> RiskThresholds {
    let knobs = knobs();
    RiskThresholds {
        single_clue: knobs.search_task_single_clue_max,
        co_coverage: knobs.search_task_co_coverage_max,
        exposed_constant: knobs.search_task_expose_overlap,
        prior_knowledge: knobs.search_task_prior_degree as f64,
    }
}

/// Build the evidence subgraph, synthesize the task and refuse it by detector
/// name when a value is over its threshold.
fn synthesize_task(view: &GraphView, answer: &str) -> Result<SynthesizeSearchTaskResult, String> {
    let knobs = knobs();
    let graph = evidence_graph(
        view,
        answer,
        knobs.search_task_evidence_hops,
        knobs.search_task_max_evidence_nodes,
    )?;
    let thresholds = thresholds();
    let synthesis = synthesize(&graph, answer, &thresholds, knobs.search_task_refine_rounds)
        .map_err(|refusal| refusal.to_string())?;
    synthesis
        .report
        .check(&thresholds)
        .map_err(|refusal| refusal.to_string())?;
    Ok(SynthesizeSearchTaskResult {
        task: synthesis.task,
        report: synthesis.report,
        thresholds,
        rounds_used: synthesis.rounds_used,
        evidence_nodes: graph.nodes.len() as u64,
    })
}

/// Serve one `SynthesizeSearchTask`.
pub(crate) async fn handle_synthesize(
    ctx: &QueryHandlerCtx<'_>,
    request: SynthesizeSearchTaskRequest,
) -> Response {
    let req_id = ctx.req_id;
    let scope = ReadScope::resolve(ctx.state, ctx.read_authority, ctx.graph_name).await;
    let tenant = scope.caller.principal.tenant.clone();
    if request.schema_version != SEARCH_TASK_SCHEMA_VERSION {
        return refuse(
            req_id,
            format!(
                "INVALID_ARGUMENT: search task schema_version {} is not {SEARCH_TASK_SCHEMA_VERSION}",
                request.schema_version
            ),
        );
    }
    if tenant.is_empty() || request.tenant_id != tenant {
        return refuse(
            req_id,
            "ACCESS_DENIED: the request tenant must match the verified tenant".to_string(),
        );
    }
    if request.graph != ctx.graph_name {
        return refuse(
            req_id,
            format!(
                "INVALID_ARGUMENT: request graph `{}` is not the graph the call targets",
                request.graph
            ),
        );
    }
    let (snap, revision) = sql_read_snapshot(
        ctx.core,
        #[cfg(feature = "security")]
        ctx.caller,
        #[cfg(feature = "security")]
        ctx.rls,
    );
    let view = match scope.narrow(req_id, revision, snap, &[SCOPE_VARIABLE.to_string()]) {
        Ok(view) => view,
        Err(response) => return response,
    };
    let answer = request.answer_node;
    if !view.node_properties.contains_key(&answer) {
        // A hidden answer node reads like an absent one.
        return refuse(req_id, format!("{SEARCH_TASK_INVALID}: {answer}"));
    }
    let outcome = compute_off_lock(req_id, move || synthesize_task(&view, &answer)).await;
    match outcome {
        Err(response) => response,
        Ok(Err(message)) => refuse(req_id, message),
        Ok(Ok(result)) => served_response::<SynthesizeSearchTask>(
            req_id,
            Ok(Ok(result)),
            "SynthesizeSearchTask",
            #[cfg(feature = "result-cache")]
            ctx.core,
            #[cfg(feature = "result-cache")]
            None,
        ),
    }
}
