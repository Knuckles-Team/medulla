//! `Method::AtomContext` (MEDULLA-INGEST-WORKER-R003.3).
//!
//! Lists the knowledge atoms `SourceIngest` stored (`KnowledgeAtom` nodes) that the caller's
//! row-level-security projected core shows and that belong to the verified tenant, then selects
//! the ones that answer the query inside the token budget with
//! `medulla_h_ingest::atom_context::atom_context`. Every selected atom names its provenance
//! key. The read writes nothing.

use super::terminal::GraphOpsContext;
use super::*;
use medulla_ingestion_types::knowledge_atom::{
    atom_node_id, AtomContextResult, KnowledgeAtom, ATOM_NODE_TYPE,
};
use medulla_server_api::knob_config::knobs;

/// Route the atom context read served from the graph.
pub(super) async fn try_handle_atom_context(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::AtomContext {
        query,
        token_budget,
    } = method
    else {
        return ControlFlow::Continue(method);
    };
    let Some(tenant) = ctx
        .read_authority
        .carrier()
        .map(|carrier| carrier.tenant_scope().to_string())
    else {
        return ControlFlow::Break(Response::err(
            ctx.req_id,
            "ACCESS_DENIED: an atom context read needs a verified tenant",
        ));
    };
    ControlFlow::Break(serve_atom_context(
        ctx.req_id,
        ctx.core,
        &tenant,
        &query,
        token_budget,
    ))
}

/// Answer one request over the caller's graph view.
pub(super) fn serve_atom_context(
    req_id: u64,
    core: &GraphCore,
    tenant: &str,
    query: &str,
    token_budget: Option<u64>,
) -> Response {
    if query.trim().is_empty() {
        return Response::err(req_id, "PARAMETER_INVALID: query must not be blank");
    }
    let result = select(core, tenant, query, token_budget);
    Response::ok(
        req_id,
        ResultPayload::of::<medulla_protocol_types::result_contract::graph::AtomContext>(result),
    )
}

/// The budget in words: the request's, else knob `ingest.context_token_budget`, held to
/// that knob's range.
fn budget_of(token_budget: Option<u64>) -> usize {
    let default = knobs().ingest_context_token_budget;
    token_budget.unwrap_or(default).clamp(100, 32_000) as usize
}

fn select(
    core: &GraphCore,
    tenant: &str,
    query: &str,
    token_budget: Option<u64>,
) -> AtomContextResult {
    let prefix = atom_node_id("");
    let mut visible: Vec<KnowledgeAtom> = Vec::new();
    for node_id in core.node_ids() {
        if !node_id.starts_with(&prefix) {
            continue;
        }
        let Some(blob) = core.get_node_properties(&node_id) else {
            continue;
        };
        let properties = super::gateway_graph::decode_json_object(&blob);
        if properties.get("type").and_then(serde_json::Value::as_str) != Some(ATOM_NODE_TYPE) {
            continue;
        }
        let text = |key: &str| {
            properties
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        };
        let kind = properties
            .get("kind")
            .and_then(|value| serde_json::from_value(value.clone()).ok());
        if let (Some(atom_id), Some(kind), Some(atom_text), Some(provenance_key), Some(owner)) = (
            text("atom_id"),
            kind,
            text("text"),
            text("provenance_key"),
            text("tenant"),
        ) {
            if owner == tenant {
                visible.push(KnowledgeAtom {
                    atom_id,
                    kind,
                    text: atom_text,
                    provenance_key,
                    tenant: owner,
                });
            }
        }
    }
    medulla_h_ingest_repository::atom_context::atom_context(
        query,
        budget_of(token_budget),
        &visible,
    )
}
