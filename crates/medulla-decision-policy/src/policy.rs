//! Decision policy resolution, the seam the handler layers share
//! (MEDULLA-SDLC-R006.7).
//!
//! A request names its policy with a [`DecisionPolicyRef`]; [`resolve_policy`]
//! is the one place that turns that reference into the policy body, its digest
//! and the engine's statistical defaults. The agent-library store is hidden
//! behind [`PolicyLibrary`] so this crate stays below the persistence layer of
//! the crate DAG while any handler layer can reach the seam without a
//! sibling-handler edge.

use medulla_agent_types::agent_component::{
    AgentComponentEntry, AgentComponentKind, ComponentDependency,
};
use medulla_decision_types::decision::digest::policy_digest;
use medulla_decision_types::decision::policy::policy_from_attributes;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_decision_types::decision::{
    DecisionPolicy, DecisionPolicyRef, QuantScaleTag, QuantisedValue, StatisticalPolicy,
    TraceFidelityLevel, UnitRationalWire,
};
use medulla_types::contract::BoundedVec;

/// A resolved policy: the body, its digest and its statistical half.
pub struct ResolvedPolicy {
    /// The policy body.
    pub policy: DecisionPolicy,
    /// The digest of the policy body.
    pub digest: String,
    /// The statistical half, falling back to the engine default.
    pub statistical: StatisticalPolicy,
}

/// The agent-library read the policy resolver needs: the retained revisions of
/// one component in one tenant.
pub trait PolicyLibrary {
    /// The retained revisions of `component_id` in `tenant_id`, newest first.
    fn policy_revisions(
        &self,
        tenant_id: &str,
        component_id: &str,
    ) -> Result<Vec<AgentComponentEntry>, String>;
}

/// One pin mismatch, rendered with the statistical surface's stable code.
fn pin_mismatch(component: &ComponentDependency, detail: &str) -> String {
    StatisticalErrorCode::ComponentPinMismatch
        .refusal(format!("{} {detail}", component.component_id))
}

fn unit_rational(numerator: u64, denominator: u64) -> UnitRationalWire {
    UnitRationalWire::new(numerator, denominator).expect("default levels are unit rationals")
}

/// The engine's default statistical levels: alpha 1/10, epsilon 1/20, delta
/// 1/20, `n_min` 100 per class, k-anonymity 10, minimum ESS 50, tool-call
/// fidelity, and the ruled 5% audit sample of acted decisions.
pub fn default_statistical_policy() -> StatisticalPolicy {
    StatisticalPolicy {
        alpha: unit_rational(1, 10),
        epsilon: unit_rational(1, 20),
        delta: unit_rational(1, 20),
        n_min: 100,
        min_support: 10,
        min_ess: QuantisedValue {
            scale: QuantScaleTag::Q32,
            value: 50 << 32,
        },
        min_outcome_fidelity: TraceFidelityLevel::ToolCalls,
        tenant_public_features: false,
        audit_sample: unit_rational(1, 20),
        approved_commit_principals: BoundedVec::default(),
        compact_after_ms: None,
        drop_blob_after_ms: None,
    }
}

/// Resolve `reference` for `tenant_id` through `store`: the engine default, or
/// a pinned policy revision checked for tenant and kind, decoded against its
/// content digest.
pub fn resolve_policy(
    store: &dyn PolicyLibrary,
    tenant_id: &str,
    reference: &DecisionPolicyRef,
) -> Result<ResolvedPolicy, String> {
    let policy = match reference {
        DecisionPolicyRef::Default => DecisionPolicy::engine_default(),
        DecisionPolicyRef::Pinned { component } => {
            if component.kind != AgentComponentKind::DecisionPolicy {
                return Err(pin_mismatch(component, "is pinned as the wrong kind"));
            }
            let entry = store
                .policy_revisions(tenant_id, &component.component_id)?
                .into_iter()
                .rev()
                .find(|entry| entry.definition_digest == component.definition_digest)
                .filter(|entry| entry.kind == AgentComponentKind::DecisionPolicy)
                .ok_or_else(|| {
                    pin_mismatch(component, "has no revision at the pinned definition digest")
                })?;
            policy_from_attributes(&entry.attributes, &entry.content_digest)
                .map_err(|code| format!("{code}: pinned decision policy"))?
        }
    };
    let statistical = policy
        .statistical
        .clone()
        .unwrap_or_else(default_statistical_policy);
    Ok(ResolvedPolicy {
        digest: policy_digest(&policy),
        policy,
        statistical,
    })
}
