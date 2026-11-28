//! `DecisionEval` `held_item_critique` (MEDULLA-DECISION-ENGINE-R038.6): the engine records the
//! critique of a run output proposal.
//!
//! The held-item admission gate (R038.3) admits or rejects a proposal by the critique verdict
//! bound to its digest, and nothing but the engine writes those fields. The caller answers the
//! checklist the admission answer names; the engine checks the proposal is the one the caller
//! read (same kind, same digest), derives the verdict (`accept` only when every question passed)
//! and the critique ref, and plans the node's `critique_ref`, `critique_digest` and
//! `critique_verdict` with the time of its own clock and the verified principal. The proposal is
//! read through the caller's access check and row-level filter, so an absent proposal, another
//! kind and a foreign tenant's proposal refuse alike.

use medulla_decision_statistical_types::decision::record::outcome::refusal::DecisionRefusal;
use medulla_decision_types::decision::held_item::{
    derive_critique, HeldItemCritiqueRecorded, HeldItemCritiqueRequest, HeldItemKind,
    HeldItemRefusal, ROW_CONTENT_DIGEST, ROW_CRITIQUE_ANSWER_PREFIX, ROW_CRITIQUE_DIGEST,
    ROW_CRITIQUE_RECORDED_AT, ROW_CRITIQUE_RECORDED_BY, ROW_CRITIQUE_REF, ROW_CRITIQUE_VERDICT,
    ROW_ITEM_KIND,
};
use medulla_graph_core::graph::GraphView;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_protocol_types::result_contract::coordination::DecisionEvalHeldItemCritique;
use medulla_server_api::authority_context::VerifiedRequestContext;
use serde_json::{json, Value};

use medulla_h_decide_base::decide::planned_write::PlannedWrite;
use medulla_h_decide_base::decide::stat_support::invalid_parameter;

/// A refusal of the held-item gate, worded with its code.
fn refused(refusal: &HeldItemRefusal) -> String {
    format!("{}: {}", refusal.code().as_str(), refusal.detail())
}

/// Plan the critique of `request` onto the proposal node read from `view`.
pub(super) fn plan(
    view: &GraphView,
    verified: &VerifiedRequestContext,
    request: &HeldItemCritiqueRequest,
    now_ms: u64,
) -> Result<PlannedWrite, String> {
    let item_ref = request.item_ref.as_str();
    if item_ref.trim().is_empty() || request.graph.trim().is_empty() {
        return Err(invalid_parameter("a critique names its graph and its item"));
    }
    let Some(mut row) = view.node_row_object(item_ref) else {
        return Err(refused(&HeldItemRefusal::ItemUnknown {
            item_ref: item_ref.to_string(),
        }));
    };
    let text = |row: &serde_json::Map<String, Value>, key: &str| {
        row.get(key).and_then(Value::as_str).map(str::to_string)
    };
    if text(&row, ROW_ITEM_KIND).as_deref() != Some(HeldItemKind::RunOutputProposal.name()) {
        return Err(refused(&HeldItemRefusal::ItemUnknown {
            item_ref: item_ref.to_string(),
        }));
    }
    let stored = text(&row, ROW_CONTENT_DIGEST).unwrap_or_default();
    if stored != request.content_digest {
        return Err(refused(&HeldItemRefusal::DigestMismatch {
            stored,
            expected: request.content_digest.clone(),
        }));
    }
    let critique = derive_critique(request).map_err(invalid_parameter)?;
    row.insert(
        ROW_CRITIQUE_REF.into(),
        critique.critique_ref.clone().into(),
    );
    row.insert(
        ROW_CRITIQUE_DIGEST.into(),
        request.content_digest.clone().into(),
    );
    row.insert(ROW_CRITIQUE_VERDICT.into(), critique.verdict.into());
    row.insert(ROW_CRITIQUE_RECORDED_AT.into(), now_ms.into());
    row.insert(ROW_CRITIQUE_RECORDED_BY.into(), verified.agent_id().into());
    for answer in &critique.answers {
        row.insert(
            format!("{ROW_CRITIQUE_ANSWER_PREFIX}{}", answer.id),
            answer.passed.into(),
        );
    }
    let recorded = HeldItemCritiqueRecorded {
        critique_ref: critique.critique_ref,
        item_ref: item_ref.to_string(),
        content_digest: request.content_digest.clone(),
        verdict: critique.verdict.to_string(),
    };
    Ok(PlannedWrite {
        graph: request.graph.clone(),
        operations: vec![json!({"op": "upsert_node", "id": item_ref, "properties": row})],
        reply: ResultPayload::of::<DecisionEvalHeldItemCritique>(recorded)?,
    })
}
