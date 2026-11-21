//! `DecisionLog.promote_rule`: the attach point of rule promotion
//! (MEDULLA-DECISION-ENGINE-R129.1).
//!
//! A rule backed by stored abstention resolutions is checked against the
//! tenant's published `RuleInvariant` components with the pure invariant gate
//! (R129.3) before anything else. A rule that passes is published as an
//! active `GuardrailRule` component; a rule that contradicts an invariant is
//! refused `RULE_CONTRADICTS_INVARIANT` and nothing is activated. No confidence
//! input is read here, so no agreement level can override an invariant.
//!
//! Agreement is the share of the named stored resolutions a human gave
//! (MEDULLA-DECISION-ENGINE-R129.5). Fewer than `rule_promotion.min_overrides`
//! human resolutions refuse `RULE_PROMOTION_TOO_FEW_OVERRIDES`; agreement
//! below `rule_promotion.min_agreement` refuses `RULE_PROMOTION_AGREEMENT_LOW`.
//! A rejection and a promotion each store one [`RulePromotionRecord`].

use medulla_agent_types::agent_component::{
    AgentComponentDraft, AgentComponentKind, AgentComponentPublishRequest, DraftPublication,
    DraftSubject,
};
use medulla_agent_types::agent_guardrail::{
    promote_on_consensus, GuardrailRule, RulePromotionVerdict,
};
use medulla_agent_types::agent_library::{AgentLibraryLifecycle, AgentLibraryMutationContext};
use medulla_decision_types::decision::statistical::body::{
    canonical_body_bytes, content_digest_of,
};
use medulla_decision_types::decision::statistical::log::{
    AbstentionResolver, RulePromotionResult, StoredResolution,
};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_persistence_records::agent_library::current_agent_library_policy_digest;
use medulla_types::contract::{BoundedVec, Nonce};

use super::stat_executor::ExecutionContext;
use super::stat_log::{store_once, visible_entry, LogReader, MAX_LOG_ROWS};
use super::stat_support::refusal;

/// One audited promotion decision (MEDULLA-DECISION-ENGINE-R129.5): the rule,
/// what the invariant gate said, the resolutions it rested on and their
/// human agreement. `verdict` names the invariant a rejection contradicted.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RulePromotionRecord {
    pub rule: GuardrailRule,
    pub verdict: RulePromotionVerdict,
    pub resolution_ids: Vec<String>,
    pub agreement: f64,
}

/// The stored resolutions `resolution_ids` name, each on a record the reader
/// sees; an id that names none is refused.
fn named_resolutions(
    ctx: &ExecutionContext,
    reader: &LogReader,
    resolution_ids: &BoundedVec<String, 64>,
) -> Result<Vec<StoredResolution>, String> {
    let stored: Vec<StoredResolution> = ctx
        .store
        .decision_artifacts_with_prefix(ctx.tenant_id, "resolution:", MAX_LOG_ROWS)?
        .into_iter()
        .map(|(_, bytes)| {
            medulla_persistence_records::decision_jobs::decode_artifact(&bytes, "resolution")
        })
        .collect::<Result<_, String>>()?;
    let mut named = Vec::new();
    for id in resolution_ids.iter() {
        let found = stored
            .iter()
            .find(|row| row.resolution.resolution_id == *id)
            .filter(|row| {
                matches!(
                    visible_entry(ctx.store, reader, &row.resolution.record_id),
                    Ok(Some(_))
                )
            })
            .ok_or_else(|| invalid(format!("no visible stored resolution named {id}")))?;
        named.push(found.clone());
    }
    Ok(named)
}

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
    GuardrailRule::from_attributes(&rule.rule_id, &rule.to_attributes()).map_err(invalid)?;
    let named = named_resolutions(ctx, reader, resolution_ids)?;
    let human = named
        .iter()
        .filter(|row| matches!(row.resolution.resolver, AbstentionResolver::Human))
        .count();
    let agreement = human as f64 / named.len() as f64;
    // Only this tenant's invariants are read.
    let invariants = ctx.store.published_rule_invariants(ctx.tenant_id)?;
    let knobs = medulla_server_api::knob_config::knobs();
    let verdict = promote_on_consensus(
        &rule,
        &invariants,
        agreement,
        knobs.rule_promotion_min_agreement,
    );
    let mut ids: Vec<String> = resolution_ids.iter().cloned().collect();
    ids.sort();
    ids.dedup();
    let record_key = format!(
        "rule_promotion:{}:{}",
        rule.rule_id,
        content_digest_of(&canonical_body_bytes(&ids)?)
    );
    if let Some(RulePromotionVerdict::RejectedInvariant { invariant_iri }) = &verdict {
        let record = RulePromotionRecord {
            rule: rule.clone(),
            verdict: RulePromotionVerdict::RejectedInvariant {
                invariant_iri: invariant_iri.clone(),
            },
            resolution_ids: ids,
            agreement,
        };
        store_once(ctx, record_key, "rule promotion", record, |a, b| a == b)?;
        return Err(refusal(
            StatisticalErrorCode::RuleContradictsInvariant,
            format!("the rule contradicts invariant {invariant_iri}"),
        ));
    }
    if (human as u64) < knobs.rule_promotion_min_overrides {
        return Err(refusal(
            StatisticalErrorCode::RulePromotionTooFewOverrides,
            format!(
                "{human} human resolutions; at least {} are needed",
                knobs.rule_promotion_min_overrides
            ),
        ));
    }
    let Some(verdict) = verdict else {
        return Err(refusal(
            StatisticalErrorCode::RulePromotionAgreementLow,
            format!(
                "human agreement {agreement:.3} is below {}",
                knobs.rule_promotion_min_agreement
            ),
        ));
    };
    let record = RulePromotionRecord {
        rule: rule.clone(),
        verdict: verdict.clone(),
        resolution_ids: ids,
        agreement,
    };
    store_once(ctx, record_key, "rule promotion", record, |a, b| a == b)?;

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
        prompt_blueprint: None,
    }, None)?;
    Ok(RulePromotionResult {
        rule_id: rule.rule_id,
        verdict,
        published: true,
        definition_digest: Some(committed.result.component.definition_digest),
    })
}
