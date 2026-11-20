//! `Method::QualityRulePut` (MEDULLA-UNIFIED-DATA-PLANE-R762.3).
//!
//! The handler reads the verified tenant and agent from the request carrier (never from
//! the body), validates and compiles the rule with `put_quality_rule`, and commits the
//! one tenant-private `QualityRule` node as ONE `BatchUpdate` through the ordinary graph
//! gateway (`dispatch_graph_op`), the same path the incident writes take, so the write
//! is ACL-checked, durable, audited and CDC-emitted. A refused rule commits nothing.

use std::sync::Arc;

use medulla_h_jobs::quality_rule::put_quality_rule;
use medulla_protocol_types::protocol::{Method, Response};
use medulla_protocol_types::result_contract::ingestion::QualityRulePut as QualityRulePutResult;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_server_graph::dispatch::graph_pipeline::dispatch_graph_op;
use medulla_server_graph::dispatch::shared::typed_response;
use medulla_catalog_types::quality::{QualityRule, QualityRuleReceipt};
use tokio::sync::RwLock;

/// Who asked, and which graph holds the rule.
pub struct QualityTarget<'a> {
    pub graph: &'a str,
    pub req_id: u64,
    pub caller: Option<&'a str>,
    pub verified: &'a VerifiedRequestContext,
}

/// `QualityRulePut`: store one rule in the caller's tenant, approved by the caller.
pub async fn handle_quality_rule_put(
    state: &Arc<RwLock<ServerState>>,
    target: QualityTarget<'_>,
    rule: QualityRule,
) -> Response {
    let result = put(state, &target, &rule).await;
    typed_response::<QualityRulePutResult>(target.req_id, result)
}

async fn put(
    state: &Arc<RwLock<ServerState>>,
    target: &QualityTarget<'_>,
    rule: &QualityRule,
) -> Result<QualityRuleReceipt, String> {
    let tenant = target.verified.tenant();
    if tenant.trim().is_empty() {
        return Err(
            medulla_types::contract::ServerErrorCode::TenantScopeRequired
                .as_str()
                .to_string(),
        );
    }
    let (operations, receipt) = put_quality_rule(rule, tenant, target.verified.agent_id())
        .map_err(|refusal| refusal.to_string())?;
    let operations_msgpack = rmp_serde::to_vec_named(&operations)
        .map_err(|_| "quality rule batch encoding failed".to_string())?;
    let response = dispatch_graph_op(
        state,
        target.graph,
        target.req_id,
        target.caller,
        target.verified,
        Method::BatchUpdate { operations_msgpack },
    )
    .await;
    if let Some(refusal) = response.refusal_text() {
        return Err(refusal);
    }
    Ok(receipt)
}
