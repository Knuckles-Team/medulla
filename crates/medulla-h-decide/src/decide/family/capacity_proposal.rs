//! The `capacity_proposal` question family (MEDULLA-DECISION-ENGINE-R824).
//!
//! The verdict on one capacity adjustment proposal against the headroom and
//! epoch the engine read from the capacity ledger (see `stat_capacity` and
//! `decision::statistical::capacity`). The answer is `allow` or
//! `refuse:<CODE>`: a refused proposal is a decided verdict, sealed in the
//! record. Advisory; `Decide` writes nothing.

use medulla_decision_types::decision::statistical::capacity::{
    judge, CapacityAdjustmentProposal, Observation, PARAM_OBSERVATION, PARAM_PROPOSAL,
    VERDICT_OPTION,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::DecideRequest;
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{advisory, invalid, json_param, single_option};
use super::DecisionFamily;

/// The family of the `capacity_proposal` question kind.
pub struct CapacityProposalFamily;

fn conclude_verdict(ctx: &ExecutionContext, request: &DecideRequest) -> Result<Executed, String> {
    let proposal: CapacityAdjustmentProposal = json_param(request, PARAM_PROPOSAL)?;
    let observation: Observation = json_param(request, PARAM_OBSERVATION).map_err(|_| {
        invalid("a capacity_proposal question is decided only after the ledger read")
    })?;
    let verdict = judge(&proposal, ctx.tenant_id, &observation);
    let mut executed = advisory(vec![verdict.option_id()])?;
    executed.family_premises = vec![FamilyPremise {
        subject: "capacity",
        fact: format!(
            "headroom:{}:{}:epoch={}",
            proposal.resource_class_iri, observation.available, observation.epoch
        ),
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Observation(proposal.resource_class_iri.clone()),
    }];
    Ok(executed)
}

impl DecisionFamily for CapacityProposalFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, VERDICT_OPTION)
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_verdict(ctx, request))
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
}
