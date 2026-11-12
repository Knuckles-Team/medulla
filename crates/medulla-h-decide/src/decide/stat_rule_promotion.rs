//! `DecisionLog.promote_rule`: the attach point of rule promotion
//! (MEDULLA-DECISION-ENGINE-R129.1).
//!
//! A rule backed by stored abstention resolutions is checked against the
//! tenant's published `RuleInvariant` components with the pure invariant gate
//! (R129.3) before anything else. A rule that passes is published as an
//! active `GuardrailRule` component; a rule that contradicts an invariant is
//! returned as `RejectedInvariant` and nothing is written. No confidence input
//! is read here, so no agreement level can override an invariant.

use medulla_agent_types::agent_component::{
    AgentComponentDraft, AgentComponentKind, AgentComponentPublishRequest, DraftPublication,
    DraftSubject,
};
use medulla_agent_types::agent_guardrail::{
    check_rule_against_invariants, GuardrailRule, RulePromotionVerdict,
};
use medulla_agent_types::agent_library::{AgentLibraryLifecycle, AgentLibraryMutationContext};
use medulla_decision_types::decision::statistical::body::{
    canonical_body_bytes, content_digest_of,
};
use medulla_decision_types::decision::statistical::log::RulePromotionResult;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_persistence_records::agent_library::current_agent_library_policy_digest;
use medulla_types::contract::{BoundedVec, Nonce};

use super::stat_executor::ExecutionContext;
use super::stat_log::LogReader;
use super::stat_support::refusal;

fn invalid(detail: impl Into<String>) -> String {
    refusal(StatisticalErrorCode::ParameterInvalid, detail.into())
}

fn nonce_of(digest: &str) -> Result<Nonce, String> {
    let raw = hex::decode(digest.trim_start_matches("sha256:")).map_err(|e| e.to_string())?;
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| "rule digest is not 32 bytes".to_string())?;
    Ok(Nonce::from_bytes(bytes))
}

/// Serve one `PromoteRule` op.
pub(super) fn promote(
    ctx: &ExecutionContext,
    reader: &LogReader,
    rule: GuardrailRule,
    resolution_ids: &BoundedVec<String, 64>,
) -> Result<RulePromotionResult, String> {
    if resolution_ids.is_empty() || resolution_ids.iter().any(|id| id.trim().is_empty()) {
        return Err(invalid(
            "a promoted rule names at least one stored resolution",
        ));
    }
    // A rule that would not decode back from its component is refused up front.
    GuardrailRule::from_attributes(&rule.rule_id, &rule.to_attributes())
        .map_err(|detail| invalid(detail))?;
    // Only this tenant's invariants are read.
    let invariants = ctx.store.published_rule_invariants(ctx.tenant_id)?;
    let verdict = check_rule_against_invariants(&rule, &invariants);
    if matches!(verdict, RulePromotionVerdict::RejectedInvariant { .. }) {
        return Ok(RulePromotionResult {
            rule_id: rule.rule_id,
            verdict,
            published: false,
            definition_digest: None,
        });
    }

    let content_digest = content_digest_of(&canonical_body_bytes(&rule)?);
    let current = ctx.store.current_component(ctx.tenant_id, &rule.rule_id)?;
    if let Some(entry) = &current {
        if entry.kind == AgentComponentKind::GuardrailRule
            && entry.lifecycle == AgentLibraryLifecycle::Published
            && entry.content_digest == content_digest
        {
            // Idempotent: the same rule is already active.
            return Ok(RulePromotionResult {
                rule_id: rule.rule_id,
                verdict,
                published: true,
                definition_digest: Some(entry.definition_digest.clone()),
            });
        }
    }
    let policy_digest = current_agent_library_policy_digest()?;
    let mut draft = AgentComponentDraft::bare(
        DraftSubject {
            component_id: &rule.rule_id,
            kind: AgentComponentKind::GuardrailRule,
            version: &rule.source_generation,
            content_digest: &content_digest,
            summary: &format!("promoted guardrail rule {}", rule.rule_id),
        },
        DraftPublication {
            tenant_id: ctx.tenant_id,
            actor_scope: "decision-log:promote-rule",
            purpose_id: "agent-component:publish",
            policy_digest: &policy_digest,
            source_revision: &format!("promotion:{}", resolution_ids.len()),
            source_revision_digest: &content_digest,
        },
    );
    draft.attributes = rule.to_attributes();
    let context = AgentLibraryMutationContext {
        request_id: 0,
        principal: ctx.store.owner_principal().to_string(),
        caller_principal: reader.principal.clone(),
        attempt_nonce: nonce_of(&content_digest)?,
        tenant_id: ctx.tenant_id.to_string(),
        actor_scope: "decision-log:promote-rule".to_string(),
        purpose_id: "agent-component:publish".to_string(),
        policy_revision: "policy-v1".to_string(),
        policy_digest,
        policy_decision_id: "agent-component:decision:policy-v1".to_string(),
        idempotency_key: format!("promote-rule:{}:{content_digest}", rule.rule_id),
        expected_revision: Some(current.as_ref().map_or(0, |entry| entry.entry_revision)),
        trace_id: None,
        created_at_ms: ctx.now_ms,
    };
    let committed = ctx.store.publish_component(AgentComponentPublishRequest {
        context,
        evaluation_receipt_digest: None,
        component: draft,
    })?;
    Ok(RulePromotionResult {
        rule_id: rule.rule_id,
        verdict,
        published: true,
        definition_digest: Some(committed.result.component.definition_digest),
    })
}
