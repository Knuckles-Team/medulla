//! `Method::RetrievalContextSearch` (MEDULLA-DECISION-ENGINE-R100.100.4): find the
//! matches of a query's n-grams in the stored text of the caller's `Document` nodes.
//!
//! The handler reads the snapshot the one served read chokepoint hands out: the
//! caller's row-level-security view, narrowed to the caller's tenant and visible
//! units by `ReadScope::narrow`. Visibility is settled before any text is matched,
//! counted or digested, so a document the caller cannot read leaves no span, no
//! count, no cursor offset and no trace in `index_digest`: the answer is the one an
//! absent document gives. The kernel (`medulla_decision_stats::decision::context_search`)
//! turns text into spans with offsets into the original bytes; the cursor kernel
//! (`context_cursor`) pins tenant, query and snapshot revision. Nothing here writes.

use super::segment_filter::ReadScope;
use super::sql_read::sql_read_snapshot;
use super::*;
use medulla_agent_types::agent_component::AgentComponentEntry;
use medulla_agent_types::agent_component::DecisionPolicyRef;
use medulla_decision_types::decision::knob_families::context_search::{ContextSearchKnobs, KNOBS};
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_decision_types::decision::statistical::context_search::{
    sha256_hex, ContextSpan, RetrievalContextSearchRequest, RetrievalContextSearchResult,
    CONTEXT_SEARCH_SCHEMA_VERSION,
};
use medulla_decision_types::decision::DecisionPolicy;
use medulla_protocol_types::result_contract::query::RetrievalContextSearch;
use {
    medulla_decision_stats_records::decision::context_cursor,
    medulla_decision_stats_records::decision::context_cursor::CursorState,
};
use {
    medulla_decision_stats_records::decision::context_search::index_text,
    medulla_decision_stats_records::decision::context_search::query_ngrams,
    medulla_decision_stats_records::decision::context_search::search_document,
};

/// Node type whose `text` property is searched.
const DOCUMENT_TYPE: &str = "Document";
/// The node variable the read scope binds its tenant predicate to.
const SCOPE_VARIABLE: &str = "document";

/// One read handle over the agent library, for the decision-policy seam.
struct PolicyLibraryHandle<'a>(&'a medulla_persistence_records::agent_library::AgentLibraryStore);

impl medulla_decision_policy::policy::PolicyLibrary for PolicyLibraryHandle<'_> {
    fn policy_revisions(
        &self,
        tenant_id: &str,
        component_id: &str,
    ) -> Result<Vec<AgentComponentEntry>, String> {
        self.0.component_revisions(tenant_id, component_id)
    }

    fn champion_policy_target(
        &self,
        tenant_id: &str,
        policy_id: &str,
    ) -> Result<Option<String>, String> {
        self.0.champion_policy_target(tenant_id, policy_id)
    }
}

/// The policy the request names: the engine default, or a pinned published policy
/// of the caller's tenant read from the Agent Library through the decision-policy seam.
pub(super) async fn resolve_policy(
    ctx: &QueryHandlerCtx<'_>,
    tenant_id: &str,
    reference: &DecisionPolicyRef,
) -> Result<DecisionPolicy, String> {
    if matches!(reference, DecisionPolicyRef::Default) {
        return Ok(DecisionPolicy::engine_default());
    }
    let store = medulla_persistence_records::agent_library::shared_agent_library(ctx.state)
        .await
        .map_err(|error| format!("ENGINE_UNAVAILABLE: {error}"))?;
    medulla_decision_policy::policy::resolve_policy(
        &PolicyLibraryHandle(&store),
        tenant_id,
        reference,
    )
    .map(|resolved| resolved.policy)
}

/// The n-gram width one search uses: the knob when the request leaves it at 0,
/// otherwise the request's width, which must lie inside the knob's declared range.
fn effective_ngram_n(requested: u32, knobs: &ContextSearchKnobs) -> Result<u32, String> {
    if requested == 0 {
        return Ok(knobs.ngram_n);
    }
    let (KnobValue::Integer(min), KnobValue::Integer(max)) = (KNOBS[1].min, KNOBS[1].max) else {
        return Err("INVALID_ARGUMENT: the ngram_n knob has no integer range".to_string());
    };
    if i64::from(requested) < min || i64::from(requested) > max {
        return Err(format!(
            "INVALID_ARGUMENT: ngram_n {requested} is outside [{min}, {max}]"
        ));
    }
    Ok(requested)
}

/// The digest a cursor pins as "this query": graph, query text and n-gram width.
fn query_digest(graph: &str, query: &str, ngram_n: u32) -> String {
    sha256_hex(&[
        graph.as_bytes(),
        &[0],
        query.as_bytes(),
        &[0],
        ngram_n.to_string().as_bytes(),
    ])
}

/// The type of a stored node: `type`, else `node_type`.
pub(super) fn node_type(object: &serde_json::Map<String, serde_json::Value>) -> &str {
    ["type", "node_type"]
        .iter()
        .filter_map(|key| object.get(*key).and_then(serde_json::Value::as_str))
        .find(|value| !value.is_empty())
        .unwrap_or("")
}

/// Every span of every `Document` in `view`, in (document id, match start) order,
/// and the digest over the per-document index digests in document-id order.
fn search_view(
    view: &medulla_graph_core::graph::GraphView,
    revision: u64,
    grams: &[String],
    knobs: &ContextSearchKnobs,
) -> Result<(Vec<ContextSpan>, String), String> {
    let mut ids: Vec<&String> = view.node_properties.keys().collect();
    ids.sort();
    let mut spans = Vec::new();
    let mut digests: Vec<u8> = Vec::new();
    for id in ids {
        let Some(object) = view.node_row_object(id) else {
            continue;
        };
        if node_type(&object) != DOCUMENT_TYPE {
            continue;
        }
        let Some(text) = object.get("text").and_then(serde_json::Value::as_str) else {
            continue;
        };
        digests.extend_from_slice(id.as_bytes());
        digests.push(0);
        digests.extend_from_slice(index_text(text)?.index_digest.as_bytes());
        digests.push(b'\n');
        spans.extend(search_document(id, revision, text, grams, knobs)?);
    }
    Ok((spans, sha256_hex(&[&digests])))
}

fn refuse(req_id: u64, message: String) -> Response {
    Response::err(req_id, message)
}

/// Serve one `RetrievalContextSearch`.
pub(crate) async fn handle_context_search(
    ctx: &QueryHandlerCtx<'_>,
    request: RetrievalContextSearchRequest,
) -> Response {
    let req_id = ctx.req_id;
    let scope = ReadScope::resolve(ctx.state, ctx.read_authority, ctx.graph_name).await;
    let tenant = scope.caller.principal.tenant.clone();
    if request.schema_version != CONTEXT_SEARCH_SCHEMA_VERSION {
        return refuse(
            req_id,
            format!(
                "INVALID_ARGUMENT: context search schema_version {} is not {CONTEXT_SEARCH_SCHEMA_VERSION}",
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
    let policy = match resolve_policy(ctx, &tenant, &request.policy).await {
        Ok(policy) => policy,
        Err(message) => return refuse(req_id, message),
    };
    let knobs = ContextSearchKnobs::from_policy(&policy);
    let ngram_n = match effective_ngram_n(request.ngram_n, &knobs) {
        Ok(width) => width,
        Err(message) => return refuse(req_id, message),
    };
    let grams = match query_ngrams(&request.query, ngram_n) {
        Ok(grams) => grams,
        Err(message) => return refuse(req_id, message),
    };
    let page_limit = match request.max_spans {
        0 => knobs.max_spans,
        asked => asked.min(knobs.max_spans),
    } as usize;

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
    let digest = query_digest(&request.graph, &request.query, ngram_n);
    let start = match request.cursor.as_deref() {
        None => 0,
        Some(cursor) => match context_cursor::decode(cursor, &tenant, &digest, revision) {
            Ok(offset) => usize::try_from(offset).unwrap_or(usize::MAX),
            Err(message) => return refuse(req_id, message),
        },
    };
    let authorization_revision = ctx
        .read_authority
        .and_then(GraphReadAuthority::carrier)
        .map_or(
            medulla_mutation_types::graph_change_feed::authorization_revision("", &[]),
            |carrier| {
                medulla_mutation_types::graph_change_feed::authorization_revision(
                    carrier.agent_id(),
                    carrier.scopes(),
                )
            },
        );
    let outcome =
        compute_off_lock(req_id, move || search_view(&view, revision, &grams, &knobs)).await;
    let (spans, index_digest) = match outcome {
        Err(response) => return response,
        Ok(Err(message)) => return refuse(req_id, message),
        Ok(Ok(found)) => found,
    };
    let end = start.saturating_add(page_limit).min(spans.len());
    let page: Vec<ContextSpan> = spans
        .get(start.min(end)..end)
        .map(<[ContextSpan]>::to_vec)
        .unwrap_or_default();
    let next_cursor = (end < spans.len()).then(|| {
        context_cursor::encode(&CursorState {
            tenant_id: tenant,
            query_digest: digest,
            snapshot_revision: revision,
            next_offset: end as u64,
        })
    });
    served_response::<RetrievalContextSearch>(
        req_id,
        Ok(Ok(RetrievalContextSearchResult {
            spans: page,
            query_revision: revision,
            index_digest,
            authorization_revision,
            next_cursor,
        })),
        "RetrievalContextSearch",
        #[cfg(feature = "result-cache")]
        ctx.core,
        #[cfg(feature = "result-cache")]
        None,
    )
}
