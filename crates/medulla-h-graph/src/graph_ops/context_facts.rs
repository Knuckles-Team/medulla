//! `Method::ContextFacts` (MEDULLA-DECISION-ENGINE-R152.2, R152.3, R152.4, R152.5).
//!
//! For each node id in the caller's order, the visible literal properties and
//! outgoing edges become cited facts inside a token budget. The read runs over
//! the row-level-security projected core, so a node the caller cannot see yields
//! no fact and an edge to a hidden node does not exist. The ranking and the
//! budget are the pure functions of `medulla_types::context_facts`.

use super::terminal::GraphOpsContext;
use super::*;
use medulla_server_api::knob_config::knobs;
use medulla_types::context_facts::{
    select, validate_request, ContextFactsRefusal, ContextFactsRequest, ContextFactsResult, RawFact,
};
use medulla_types::evidence_class::EvidenceClass;

/// Property keys that carry provenance, not content.
const SOURCE_KEY: &str = "source_key";
const EVIDENCE_KEY: &str = "evidence_class";
/// Upper bound of knob `context_facts.max_facts_per_node`.
const MAX_FACTS_PER_NODE: u64 = 256;

/// Route the context facts read served from the graph.
pub(super) async fn try_handle_context_facts(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    match method {
        Method::ContextFacts { request } => {
            ControlFlow::Break(serve_context_facts(ctx.req_id, ctx.core, &request))
        }
        other => ControlFlow::Continue(other),
    }
}

/// Answer one request over the caller's graph view. A refusal carries its
/// registered code and no result; a read adds no sealed record.
pub(super) fn serve_context_facts(
    req_id: u64,
    core: &GraphCore,
    request: &ContextFactsRequest,
) -> Response {
    match select_facts(core, request) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::graph::ContextFacts>(
                result,
            ),
        ),
        Err(refusal) => Response::err(req_id, refusal.to_string()),
    }
}

/// Select the cited facts of the requested nodes inside the token budget.
pub(super) fn select_facts(
    core: &GraphCore,
    request: &ContextFactsRequest,
) -> Result<ContextFactsResult, ContextFactsRefusal> {
    validate_request(request)?;
    let configured = knobs();
    let per_node_cap = request
        .max_facts_per_node
        .map(u64::from)
        .unwrap_or(configured.context_facts_max_facts_per_node)
        .clamp(1, MAX_FACTS_PER_NODE) as usize;
    let mut seen = std::collections::HashSet::new();
    let per_node: Vec<Vec<RawFact>> = request
        .node_ids
        .iter()
        .filter(|id| seen.insert(id.as_str()))
        .map(|id| node_facts(core, id))
        .collect();
    select(
        per_node,
        request.token_budget,
        per_node_cap,
        configured.context_facts_bytes_per_token,
    )
}

/// The literal properties and outgoing edges of one node; empty when the node is
/// not in the caller's view.
fn node_facts(core: &GraphCore, node_id: &str) -> Vec<RawFact> {
    let Some(blob) = core.get_node_properties(node_id) else {
        return Vec::new();
    };
    let properties = super::gateway_graph::decode_json_object(&blob);
    let node_source = text(&properties, SOURCE_KEY);
    let node_authority = authority(&properties);
    let mut facts = Vec::new();
    for (key, value) in &properties {
        if key.starts_with('_') || key == SOURCE_KEY || key == EVIDENCE_KEY {
            continue;
        }
        let Some(object) = literal(value) else {
            continue;
        };
        facts.push(RawFact {
            subject: node_id.to_string(),
            predicate: key.clone(),
            object,
            source_key: node_source.clone(),
            authority: node_authority,
        });
    }
    for target in core.get_successors(node_id).unwrap_or_default() {
        for (relation, edge_blob) in core.typed_edges_between(node_id, &target) {
            let edge = super::gateway_graph::decode_json_object(&edge_blob);
            facts.push(RawFact {
                subject: node_id.to_string(),
                predicate: relation,
                object: target.clone(),
                source_key: text(&edge, SOURCE_KEY),
                authority: authority(&edge),
            });
        }
    }
    facts
}

/// A scalar property value as fact text; arrays, objects and null are not facts.
fn literal(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn text(properties: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    properties
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// The stored evidence class; the weakest class when none is stored.
fn authority(properties: &serde_json::Map<String, serde_json::Value>) -> EvidenceClass {
    text(properties, EVIDENCE_KEY)
        .as_deref()
        .and_then(EvidenceClass::from_label)
        .unwrap_or(EvidenceClass::Claim)
}
