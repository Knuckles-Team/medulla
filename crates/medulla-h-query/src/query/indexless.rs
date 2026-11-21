//! `Method::IndexlessRetrieve` (MEDULLA-DECISION-ENGINE-R144.3, R144.4, R144.5,
//! R144.7): raw reads, an id-ordered corpus scan and span digest checks over the
//! caller's visible `Document` nodes, with no index built or consulted.
//!
//! The handler reads the snapshot the served read chokepoint hands out, narrowed to
//! the caller's tenant and visible units by `ReadScope::narrow`, the same snapshot
//! authorization `RetrievalContextSearch` and `UnifiedQuery` use. A document the
//! caller cannot read is therefore absent from the view, and a read of it refuses
//! with the very `DOCUMENT_NOT_FOUND` answer an absent document gets.
//!
//! The applied budget is the field-wise minimum of the caller budget and the tenant
//! ceiling the decision policy's `indexless.*` knobs set (`clamp_budget`). Before
//! each operation the pure `BudgetMeter` admits what it needs:
//!
//! * `RawRead`: 1 row, 1 visit, `indexless.read_cost` cost units;
//! * `CorpusScan { limit }`: one row per `Document` returned (at most `limit`, in
//!   id order), one visit per node inspected to reach them, `indexless.scan_cost`
//!   per row;
//! * `VerifySpan`: 1 row, 1 visit, `indexless.verify_cost` cost units.
//!
//! A refused operation leaves the meter unchanged. Every response that got past the
//! tenant, graph and policy checks carries the `CostReport`: an accepted one in its
//! body, a refused one in the refusal detail, which also names the first refused
//! operation id. Nothing here writes.

use std::collections::BTreeMap;

use super::context_search::{node_type, resolve_policy};
use super::segment_filter::ReadScope;
use super::sql_read::sql_read_snapshot;
use super::*;
use medulla_decision_types::decision::indexless_retrieval::{
    ceiling_premises, clamp_budget, operation_id, tenant_ceiling, unit_costs, verify_span, BudgetMeter, CostReport,
    IndexlessErrorCode, IndexlessOp, IndexlessRequest, IndexlessResult, IndexlessUnitCosts,
};
use medulla_protocol_types::result_contract::query::IndexlessRetrieve;

/// Node type whose `text` property the operations read.
const DOCUMENT_TYPE: &str = "Document";
/// The node variable the read scope binds its tenant predicate to.
const SCOPE_VARIABLE: &str = "document";
/// The wire code a denied or absent document refuses with.
const DOCUMENT_NOT_FOUND: &str = "DOCUMENT_NOT_FOUND";

/// The caller-visible corpus: every node id in id order, with the text of the ones
/// that are `Document` nodes.
struct Corpus {
    nodes: Vec<(String, Option<String>)>,
    documents: BTreeMap<String, String>,
}

impl Corpus {
    fn of(view: &medulla_graph_core::graph::GraphView) -> Self {
        let mut ids: Vec<&String> = view.node_properties.keys().collect();
        ids.sort();
        let mut nodes = Vec::with_capacity(ids.len());
        let mut documents = BTreeMap::new();
        for id in ids {
            let text = view.node_row_object(id).and_then(|object| {
                (node_type(&object) == DOCUMENT_TYPE)
                    .then(|| {
                        object
                            .get("text")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    })
                    .flatten()
            });
            if let Some(text) = &text {
                documents.insert(id.clone(), text.clone());
            }
            nodes.push((id.clone(), text));
        }
        Self { nodes, documents }
    }
}

/// One refused operation: its code, the operation id and what went wrong.
struct Refused {
    code: &'static str,
    operation: String,
    detail: String,
}

/// The refusal text `Response::err` splits into code and detail; the detail names
/// the first refused operation and carries the cost report at that moment.
fn refusal_text(refused: &Refused, report: &CostReport) -> String {
    let cost = serde_json::to_string(report).unwrap_or_default();
    format!(
        "{}: operation {}: {}; cost {cost}",
        refused.code, refused.operation, refused.detail
    )
}

/// Run every operation in order under the meter. `Err` holds the first refusal
/// and the report at that moment.
fn run_ops(
    corpus: &Corpus,
    request: &IndexlessRequest,
    costs: IndexlessUnitCosts,
    meter: &mut BudgetMeter,
) -> Result<Vec<serde_json::Value>, Refused> {
    let mut rows = Vec::new();
    for (index, op) in request.ops.iter().enumerate() {
        let id = operation_id(index, op);
        let exhausted = |code: IndexlessErrorCode, detail: String| Refused {
            code: code.as_str(),
            operation: id.clone(),
            detail,
        };
        let not_found = |document_id: &str| Refused {
            code: DOCUMENT_NOT_FOUND,
            operation: id.clone(),
            detail: format!("document `{document_id}` not found"),
        };
        match op {
            IndexlessOp::RawRead { document_id } => {
                let text = corpus
                    .documents
                    .get(document_id)
                    .ok_or_else(|| not_found(document_id))?;
                meter.charge(&id, 1, 1, costs.read).map_err(|code| {
                    exhausted(code, "the read exceeds the applied budget".to_string())
                })?;
                rows.push(serde_json::json!({
                    "operation_id": id,
                    "document_id": document_id,
                    "text": text,
                }));
            }
            IndexlessOp::CorpusScan { limit } => {
                let mut visits = 0u64;
                let mut found: Vec<(&String, &String)> = Vec::new();
                for (node_id, text) in &corpus.nodes {
                    if found.len() as u64 >= *limit {
                        break;
                    }
                    visits += 1;
                    if let Some(text) = text {
                        found.push((node_id, text));
                    }
                }
                let count = found.len() as u64;
                meter
                    .charge(&id, count, visits, count.saturating_mul(costs.scan))
                    .map_err(|code| {
                        exhausted(
                            code,
                            format!("a scan of {count} rows exceeds the applied budget"),
                        )
                    })?;
                rows.extend(found.into_iter().map(|(document_id, text)| {
                    serde_json::json!({
                        "operation_id": id,
                        "document_id": document_id,
                        "text": text,
                    })
                }));
            }
            IndexlessOp::VerifySpan {
                document_id,
                start_byte,
                end_byte,
                expected_digest,
            } => {
                let text = corpus
                    .documents
                    .get(document_id)
                    .ok_or_else(|| not_found(document_id))?;
                meter.charge(&id, 1, 1, costs.verify).map_err(|code| {
                    exhausted(code, "the span check exceeds the applied budget".to_string())
                })?;
                verify_span(text, *start_byte, *end_byte, expected_digest).map_err(|code| {
                    exhausted(
                        code,
                        format!(
                            "span [{start_byte}, {end_byte}) of `{document_id}` is not on character boundaries or its digest differs"
                        ),
                    )
                })?;
                rows.push(serde_json::json!({
                    "operation_id": id,
                    "document_id": document_id,
                    "start_byte": start_byte,
                    "end_byte": end_byte,
                    "verified": true,
                }));
            }
        }
    }
    Ok(rows)
}

/// Serve one `IndexlessRetrieve`.
pub(crate) async fn handle_indexless_retrieve(
    ctx: &QueryHandlerCtx<'_>,
    request: IndexlessRequest,
) -> Response {
    let req_id = ctx.req_id;
    let scope = ReadScope::resolve(ctx.state, ctx.read_authority, ctx.graph_name).await;
    let tenant = scope.caller.principal.tenant.clone();
    // Tenant and graph are settled before any read (R144.4).
    if tenant.is_empty() || request.tenant_id != tenant {
        return Response::err(
            req_id,
            "ACCESS_DENIED: the request tenant must match the verified tenant",
        );
    }
    if request.graph != ctx.graph_name {
        return Response::err(
            req_id,
            "ACCESS_DENIED: the request graph is not the graph the call is authorized for",
        );
    }
    let policy = match resolve_policy(ctx, &tenant, &request.policy).await {
        Ok(policy) => policy,
        Err(message) => return Response::err(req_id, message),
    };
    let ceiling = tenant_ceiling(&policy);
    let applied = clamp_budget(request.budget, ceiling);
    let costs = unit_costs(&policy);
    let mut meter = BudgetMeter::new(applied);
    // Every report names the ceiling it was clamped to (R144.7).
    let premises = ceiling_premises(ceiling);
    let report = move |meter: &BudgetMeter, revision: u64| CostReport {
        premises: premises.clone(),
        ..meter.report(revision)
    };

    let (snap, revision) = sql_read_snapshot(
        ctx.core,
        #[cfg(feature = "security")]
        ctx.caller,
        #[cfg(feature = "security")]
        ctx.rls,
    );
    let request = match request.checked() {
        Ok(request) => request,
        Err(code) => {
            let refused = Refused {
                code: code.as_str(),
                operation: "none".to_string(),
                detail: "the request carries no operation".to_string(),
            };
            return Response::err(req_id, refusal_text(&refused, &report(&meter, revision)));
        }
    };
    let view = match scope.narrow(req_id, revision, snap, &[SCOPE_VARIABLE.to_string()]) {
        Ok(view) => view,
        Err(response) => return response,
    };
    let outcome = compute_off_lock(req_id, move || {
        let corpus = Corpus::of(&view);
        let ran = run_ops(&corpus, &request, costs, &mut meter);
        (ran, report(&meter, revision))
    })
    .await;
    match outcome {
        Err(response) => response,
        Ok((Err(refused), report)) => Response::err(req_id, refusal_text(&refused, &report)),
        Ok((Ok(rows), cost)) => served_response::<IndexlessRetrieve>(
            req_id,
            Ok(Ok(IndexlessResult { rows, cost })),
            "IndexlessRetrieve",
            #[cfg(feature = "result-cache")]
            ctx.core,
            #[cfg(feature = "result-cache")]
            None,
        ),
    }
}
