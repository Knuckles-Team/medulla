//! Served `Method::HistorySearch` (MEDULLA-UNIFIED-DATA-PLANE-R750.753/.754).
//!
//! The terminal work-item commit stores each conversation step as a `Step`
//! graph node. This handler checks the caller's scope grant, then runs one UQL
//! plan (`Scan Step` → filter tenant and scope → `RankText` → `Limit k`) over
//! the caller's RLS-filtered view through the `QueryApi` seam. Tenant and scope
//! filter before the rank, so a hit from another tenant or scope never appears.

use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
#[cfg(feature = "text")]
use medulla_graph_core::graph::GraphView;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_api::state::ServerState;
#[cfg(feature = "text")]
use medulla_types::wire::{Op, Plan, Pred};
use medulla_work_types::run_history::{HistoryHit, HistoryRefusal};
use tokio::sync::RwLock;

/// The graph label a stored step carries (written by the work-item commit).
pub const STEP_LABEL: &str = "Step";
/// The largest `k` one search returns; a larger `k` is served as this.
pub const MAX_HISTORY_HITS: u32 = 1000;

/// Already-authorized context for one `HistorySearch`.
pub struct HandleContext<'a> {
    /// The wire request id.
    pub req_id: u64,
    /// The server state the query seam is read from.
    pub state: &'a Arc<RwLock<ServerState>>,
    /// The request graph's core.
    pub core: &'a Arc<GraphCore>,
    /// The caller's RLS read authority.
    pub read_authority: &'a Option<GraphReadAuthority>,
    /// The verified caller.
    pub verified_context: &'a VerifiedRequestContext,
}

/// One validated search request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRequest {
    /// The verified tenant.
    pub tenant: String,
    /// The granted history scope.
    pub scope: String,
    /// The lexical query.
    pub query: String,
    /// The hit cap.
    pub k: usize,
}

/// Handle `Method::HistorySearch`; hand any other method back.
pub async fn try_handle(ctx: HandleContext<'_>, method: Method) -> Result<Response, Method> {
    let Method::HistorySearch { scope, query, k } = method else {
        return Err(method);
    };
    let result = handle_history_search(&ctx, scope, query, k).await.and_then(
        ResultPayload::of::<medulla_protocol_types::result_contract::ingestion::HistorySearch>,
    );
    Ok(match result {
        Ok(payload) => Response::ok(ctx.req_id, payload),
        Err(error) => Response::err(ctx.req_id, error),
    })
}

/// Without the lexical index there is no rank stage: refuse rather than serve
/// an unranked listing.
#[cfg(not(feature = "text"))]
pub async fn handle_history_search(
    _ctx: &HandleContext<'_>,
    _scope: String,
    _query: String,
    _k: u32,
) -> Result<Vec<HistoryHit>, String> {
    Err("INTERNAL: HistorySearch requires the `text` feature".into())
}

/// Validate the grant, run the plan through the query seam, and read the hits.
#[cfg(feature = "text")]
pub async fn handle_history_search(
    ctx: &HandleContext<'_>,
    scope: String,
    query: String,
    k: u32,
) -> Result<Vec<HistoryHit>, String> {
    let request = validate(ctx.verified_context, scope, query, k).map_err(|r| r.message())?;
    let mut view = ctx.core.analysis_snapshot();
    if let Some(authority) = ctx.read_authority {
        authority.filter_view(&mut view);
    }
    let api = medulla_server_ops::api::query_txn::query_seam(ctx.state)
        .await
        .ok_or("INTERNAL: query seam is not installed")?;
    let core = ctx.core.clone();
    let plan = history_plan(&request);
    tokio::task::spawn_blocking(move || {
        let rows = api.run_graph_plan_rows(plan, &view, &core)?;
        Ok(hits(&view, rows))
    })
    .await
    .map_err(|error| format!("INTERNAL: history search task failed: {error}"))?
}

/// The caller holds `scope` when its verified token carries it (exactly, by a
/// domain wildcard such as `project:*`, or `*`).
pub fn validate(
    context: &VerifiedRequestContext,
    scope: String,
    query: String,
    k: u32,
) -> Result<HistoryRequest, HistoryRefusal> {
    if !context.allows_action(&scope) {
        return Err(HistoryRefusal::ScopeForbidden(scope));
    }
    Ok(HistoryRequest {
        tenant: context.tenant().to_string(),
        scope,
        query,
        k: k.min(MAX_HISTORY_HITS) as usize,
    })
}

/// `Scan Step` → tenant and scope filter → `RankText` → `Limit k`.
#[cfg(feature = "text")]
pub fn history_plan(request: &HistoryRequest) -> Plan {
    let eq = |prop: &str, value: &str| Pred::Eq {
        prop: prop.to_string(),
        value: value.to_string(),
    };
    Plan::new(vec![
        Op::Scan {
            label: STEP_LABEL.to_string(),
        },
        Op::Filter {
            preds: vec![eq("tenant", &request.tenant), eq("scope", &request.scope)],
        },
        Op::RankText {
            query: request.query.clone(),
        },
        Op::Limit { k: request.k },
    ])
}

#[cfg(feature = "text")]
fn hits(view: &GraphView, rows: Vec<(String, Option<f32>)>) -> Vec<HistoryHit> {
    rows.into_iter()
        .filter_map(|(id, score)| {
            let blob = view.node_properties.get(&id)?;
            let props = medulla_types::msgpack::decode_property_value(blob.as_slice()).ok()?;
            Some(HistoryHit {
                run_id: props.get("run_id")?.as_str()?.to_string(),
                index: u32::try_from(props.get("index")?.as_u64()?).ok()?,
                score: f64::from(score.unwrap_or_default()),
            })
        })
        .collect()
}

#[cfg(all(test, feature = "text"))]
mod tests {
    use super::*;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R750.754
    #[test]
    fn history_plan_filters_tenant_and_scope_before_ranking() {
        let request = HistoryRequest {
            tenant: "acme".into(),
            scope: "project:alpha".into(),
            query: "release".into(),
            k: 5,
        };
        let ops = history_plan(&request).ops;
        let rank = ops.iter().position(|op| matches!(op, Op::RankText { .. }));
        let filter = ops.iter().position(|op| matches!(op, Op::Filter { .. }));
        assert!(matches!(&ops[0], Op::Scan { label } if label == STEP_LABEL));
        assert!(
            filter < rank,
            "the tenant/scope filter runs before the rank"
        );
        assert!(matches!(ops.last(), Some(Op::Limit { k: 5 })));
    }
}
