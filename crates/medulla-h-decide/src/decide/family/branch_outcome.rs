//! The `branch_outcome` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! R134.14).
//!
//! Fuses the recorded outcomes of one committed plan's branches. The branches
//! were written through `DecisionLog` (`RunRecord::BranchRecord`); this family
//! reads them from the caller's tenant, orders them by `branch_id`, and answers
//! one [`FusionReceipt`] under the policy triple the caller pinned. It writes
//! nothing and computes no policy.

use medulla_decision_types::decision::branch_fusion::{
    branch_record_key, fuse_branches, BranchRecord, FusionPolicyRef, PlannedBranch,
    FUSION_POLICY_ABSENT,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{DecideRequest, StatisticalOutcome};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_persistence_records::decision_jobs::decode_artifact;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{advisory, int_param, invalid, json_param, single_option, text_param};
use super::DecisionFamily;

/// The one option a branch-outcome question declares.
pub const FUSION_OPTION: &str = "fusion";
/// The prefix of the answer option that carries the receipt as JSON.
pub const ANSWER_PREFIX: &str = "receipt:";
/// The request parameter naming the plan the branches belong to.
pub const PARAM_PLAN_DIGEST: &str = "plan_digest";
/// The request parameter listing the planned branches and their input digests.
pub const PARAM_PLANNED: &str = "planned_branches";
/// The pinned fusion policy id, revision and digest.
pub const PARAM_POLICY_ID: &str = "fusion_policy_id";
pub const PARAM_POLICY_REVISION: &str = "fusion_policy_revision";
pub const PARAM_POLICY_DIGEST: &str = "fusion_policy_digest";

/// The family of the `branch_outcome` question kind.
pub struct BranchOutcomeFamily;

fn plan_digest(request: &DecideRequest) -> Result<&str, String> {
    match text_param(request, PARAM_PLAN_DIGEST)? {
        Some(text) if !text.trim().is_empty() => Ok(text),
        _ => Err(invalid("a branch_outcome question requires a plan_digest")),
    }
}

/// The policy triple the caller pinned; any part absent refuses.
fn policy_of(request: &DecideRequest) -> Result<FusionPolicyRef, String> {
    let absent = |part: &str| format!("{FUSION_POLICY_ABSENT}: pin {part}");
    let id = text_param(request, PARAM_POLICY_ID)?.ok_or_else(|| absent(PARAM_POLICY_ID))?;
    let revision =
        int_param(request, PARAM_POLICY_REVISION)?.ok_or_else(|| absent(PARAM_POLICY_REVISION))?;
    let digest =
        text_param(request, PARAM_POLICY_DIGEST)?.ok_or_else(|| absent(PARAM_POLICY_DIGEST))?;
    let policy = FusionPolicyRef {
        fusion_policy_id: id.to_string(),
        revision: u64::try_from(revision).map_err(|_| absent(PARAM_POLICY_REVISION))?,
        digest: digest.to_string(),
    };
    policy.check()?;
    Ok(policy)
}

fn claim(fact: String) -> FamilyPremise {
    FamilyPremise {
        subject: "branch_outcome",
        fact,
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request(PARAM_PLANNED),
    }
}

impl DecisionFamily for BranchOutcomeFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, FUSION_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        plan_digest(request)?;
        json_param::<Vec<PlannedBranch>>(request, PARAM_PLANNED)?;
        policy_of(request).map(|_| ())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let plan = plan_digest(request)?;
            let planned: Vec<PlannedBranch> = json_param(request, PARAM_PLANNED)?;
            let policy = policy_of(request)?;
            // Only the caller's tenant is read: another tenant's record is absent here.
            let mut records = Vec::new();
            for branch in &planned {
                let key = branch_record_key(plan, &branch.branch_id);
                if let Some(bytes) = ctx.store.decision_artifact(ctx.tenant_id, &key)? {
                    records.push(decode_artifact::<BranchRecord>(&bytes, "branch record")?);
                }
            }
            let (_, receipt) = fuse_branches(plan, &planned, &records, &policy)?;
            let body = serde_json::to_string(&receipt)
                .map_err(|error| invalid(format!("receipt encoding failed: {error}")))?;
            let mut executed = advisory(vec![format!("{ANSWER_PREFIX}{body}")])?;
            executed.family_premises = receipt
                .branches
                .iter()
                .map(|branch| {
                    claim(format!(
                        "branch:id={};position={};result={}",
                        branch.branch_id,
                        branch.fusion_position,
                        branch.result.text()
                    ))
                })
                .collect();
            executed
                .family_premises
                .push(claim(format!("fusion.digest={}", receipt.fused_digest)));
            Ok(executed)
        })())
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }

    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(ResolutionKind::Constraint)
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Observation)
    }
}
