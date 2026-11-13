//! The `capability_outcome` decision-log op (MEDULLA-DECISION-ENGINE-R172.7.1):
//! what one run observed of a designated entity updates that entity's learned
//! reward, durably, and leaves a trace of the update (R172.9).
//!
//! The reward update is `apply_capability_outcome` (R172.5) under the
//! `designation.reward_alpha` knob of the op's resolved policy. The reward row
//! and its trace commit in one owner transaction. A re-send of the same
//! outcome answers the stored trace and applies nothing twice; the same
//! `decision_id` with another outcome is an idempotency conflict.

use medulla_decision_memory::memory::capability_index::CapabilityIndex;
use medulla_decision_types::decision::capability_designation::{
    apply_capability_outcome, capability_outcome_key, CapabilityOutcomeApplied,
    CapabilityOutcomeTrace, CapabilityReward, DesignationKnobs, ObservedOutcome, RewardPolicy,
    DESIGNATION_OUTCOME_VALUE_CONFLICT, DESIGNATION_OUTCOME_VALUE_MISSING,
};
use medulla_decision_types::decision::request::DecisionPolicyRef;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
use medulla_types::contract::{BoundedVec, OpaqueId, TenantId};

use super::stat_designation::coded;
use super::stat_executor::ExecutionContext;
use super::stat_support::{invalid_parameter, refusal, resolve_policy};

/// The outcome one op names, or why it names none.
fn observed_of(success: Option<bool>, reward_q32: Option<i64>) -> Result<ObservedOutcome, String> {
    match (success, reward_q32) {
        (Some(success), None) => Ok(ObservedOutcome::Success(success)),
        (None, Some(reward)) => Ok(ObservedOutcome::Reward(reward)),
        (None, None) => Err(coded(
            DESIGNATION_OUTCOME_VALUE_MISSING,
            "name success or reward_q32",
        )),
        (Some(_), Some(_)) => Err(coded(
            DESIGNATION_OUTCOME_VALUE_CONFLICT,
            "name success or reward_q32, not both",
        )),
    }
}

/// Apply one `capability_outcome` op for the verified tenant of `ctx`.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply(
    ctx: &ExecutionContext,
    index: &CapabilityIndex,
    entity_id: &str,
    success: Option<bool>,
    reward_q32: Option<i64>,
    decision_id: &str,
    source_ids: &BoundedVec<String, 64>,
    policy: &DecisionPolicyRef,
) -> Result<CapabilityOutcomeApplied, String> {
    let observed = observed_of(success, reward_q32)?;
    let tenant = TenantId::new(ctx.tenant_id)
        .map_err(|detail| invalid_parameter(format!("tenant: {detail}")))?;
    let entity = OpaqueId::new(entity_id)
        .map_err(|detail| invalid_parameter(format!("entity_id: {detail}")))?;
    OpaqueId::new(decision_id)
        .map_err(|detail| invalid_parameter(format!("decision_id: {detail}")))?;
    // An entity the caller's tenant cannot read (another tenant's, or none)
    // is refused alike, so the answer never confirms it exists.
    let visible = index
        .visible(&tenant)
        .iter()
        .any(|snapshot| snapshot.record(&entity).is_some());
    if !visible {
        return Err(format!(
            "ACCESS_DENIED: {entity_id} is not a capability entity of this tenant"
        ));
    }
    let resolved = resolve_policy(ctx.store, ctx.tenant_id, policy)?;
    let knobs = DesignationKnobs::from_policy(&resolved.policy);
    let reward_policy = RewardPolicy::from(&knobs);
    let key = capability_outcome_key(entity_id, decision_id);
    let observed_q32 = observed.observed_q32();
    let sources: Vec<String> = source_ids.as_slice().to_vec();

    let (reward, trace_bytes, replayed) = ctx.store.apply_capability_outcome_row(
        ctx.tenant_id,
        entity_id,
        &key,
        |stored, existing| {
            if let Some(bytes) = existing {
                let earlier: CapabilityOutcomeTrace = decode_artifact(bytes, "capability outcome")?;
                if earlier.observed_q32 == observed_q32 && earlier.source_ids == sources {
                    return Ok(None);
                }
                return Err(refusal(
                    StatisticalErrorCode::IdempotencyConflict,
                    format!("the outcome of {decision_id} for {entity_id} is recorded with other content"),
                ));
            }
            let prior = stored.unwrap_or_else(|| CapabilityReward::neutral(tenant.clone(), entity.clone()));
            let revision = prior.updated_revision.saturating_add(1);
            let updated = apply_capability_outcome(&prior, observed, revision, &reward_policy);
            let trace = CapabilityOutcomeTrace {
                tenant_id: tenant.clone(),
                entity_id: entity.clone(),
                decision_id: decision_id.to_string(),
                observed_q32,
                prior_reward_q32: prior.reward_q32,
                new_reward_q32: updated.reward_q32,
                alpha_q32: reward_policy.alpha_q32,
                generation: updated.generation,
                revision,
                source_ids: sources.clone(),
            };
            Ok(Some((updated, encode_artifact(&trace)?)))
        },
    )?;
    let trace: CapabilityOutcomeTrace = decode_artifact(&trace_bytes, "capability outcome")?;
    Ok(CapabilityOutcomeApplied {
        reward,
        trace,
        replayed,
    })
}
