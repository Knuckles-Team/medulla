//! The `recruit` question family (MEDULLA-ORCH-VOCAB-R750.751).
//!
//! Stage 6 (statistical rank): ranks the employees offered for one role by
//! capability coverage (MEDULLA-DECISION-ENGINE-R126.1) -- how many of the
//! role's required capabilities each declares -- with ties on the lowest
//! agent IRI. The allocation is the deterministic market selection:
//! `allocate_offer` of `medulla_work_types::work_market::selection` keeps only
//! the legal offers of `derive_legal_offers`, in their `RankedOffer` order by
//! `utility_rate`, and returns the top one with its `priority_bucket`; every
//! excluded Gap carries its `OfferExclusion` code and no allocation is made
//! from it. Stage 11 (replay): a twin replays the decision against its pinned
//! revisions -- an employee of the twin's agent counts only at a pinned
//! revision -- and the verify-replay re-runs this conclusion on the stored
//! request, so the twin's answer reproduces exactly. Evaluate-only: `Decide`
//! writes nothing; instances move on `Method::Statechart` with
//! `AGENT_INSTANCE_CHART`.

use std::collections::BTreeSet;

use medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{DecideRequest, StatisticalOutcome};
use medulla_graph_core::graph::GraphView;
use medulla_types::evidence_class::EvidenceClass;
use medulla_work_types::work_market::selection::allocate_offer;
use {
    medulla_decision_program_types::decision::recruit::RankedEmployee,
    medulla_decision_program_types::decision::recruit::RecruitRequest,
    medulla_decision_program_types::decision::recruit::RecruitResult,
    medulla_decision_program_types::decision::recruit::ANSWER_PREFIX,
    medulla_decision_program_types::decision::recruit::PARAM_RECRUIT,
    medulla_decision_program_types::decision::recruit::RECRUIT_OPTION,
};
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    medulla_decision_statistical_types::decision::DecisionPolicy,
    medulla_decision_types::decision::record::ResolutionKind,
};

use {
    medulla_h_decide_base::decide::stat_executor::Executed,
    medulla_h_decide_base::decide::stat_executor::ExecutionContext,
    medulla_h_decide_base::decide::stat_executor::FamilyPremise,
    medulla_h_decide_base::decide::stat_executor::FamilyPremiseSource,
};

use medulla_h_decide_base::decide::family::DecisionFamily;
use {
    medulla_h_decide_base::decide::family::support::advisory,
    medulla_h_decide_base::decide::family::support::invalid,
    medulla_h_decide_base::decide::family::support::json_param,
    medulla_h_decide_base::decide::family::support::single_option,
};

/// The family of the `recruit` question kind.
pub struct RecruitFamily;

/// Rank the request's employees for its role: most required capabilities
/// covered first, then the lowest agent IRI, then the lowest revision. With a
/// twin, an employee of the twin's agent is kept only at a pinned revision.
pub(in crate::decide) fn rank_employees(asked: &RecruitRequest) -> Vec<RankedEmployee> {
    let required: BTreeSet<&str> = asked
        .role
        .required_capabilities
        .iter()
        .map(String::as_str)
        .collect();
    let required_count = u32::try_from(required.len()).unwrap_or(u32::MAX);
    let mut ranked: Vec<RankedEmployee> = asked
        .candidates
        .iter()
        .filter(|candidate| {
            asked.twin.as_ref().is_none_or(|twin| {
                candidate.employee.agent_ref != twin.agent_ref()
                    || twin
                        .pinned_revisions()
                        .iter()
                        .any(|pin| pin == &candidate.employee.definition_revision)
            })
        })
        .map(|candidate| {
            let declared: BTreeSet<&str> =
                candidate.capabilities.iter().map(String::as_str).collect();
            let covered = required.intersection(&declared).count();
            RankedEmployee {
                agent_ref: candidate.employee.agent_ref.clone(),
                definition_revision: candidate.employee.definition_revision.clone(),
                covered: u32::try_from(covered).unwrap_or(u32::MAX),
                required: required_count,
            }
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.covered
            .cmp(&a.covered)
            .then_with(|| a.agent_ref.cmp(&b.agent_ref))
            .then_with(|| a.definition_revision.cmp(&b.definition_revision))
    });
    ranked
}

/// The whole answer: the ranking and, when a market is given, the allocation.
pub(in crate::decide) fn recruit(asked: &RecruitRequest) -> RecruitResult {
    RecruitResult {
        role_id: asked.role.role_id.clone(),
        ranked: rank_employees(asked),
        allocation: asked
            .market
            .as_ref()
            .map(|market| allocate_offer(&market.gaps, &market.facts)),
    }
}

fn claim(fact: String) -> FamilyPremise {
    FamilyPremise {
        subject: "recruit".into(),
        fact,
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request(PARAM_RECRUIT),
    }
}

/// One claim premise per employee considered and per allocated offer.
fn claims(result: &RecruitResult) -> Vec<FamilyPremise> {
    let mut premises: Vec<FamilyPremise> = result
        .ranked
        .iter()
        .map(|employee| {
            claim(format!(
                "employee:agent={};revision={};covered={}/{}",
                employee.agent_ref,
                employee.definition_revision,
                employee.covered,
                employee.required
            ))
        })
        .collect();
    if let Some(top) = result.allocation.as_ref().and_then(|a| a.top()) {
        premises.push(claim(format!(
            "allocation:gap={};work_item={};utility_rate={};priority_bucket={}",
            top.gap_id, top.work_item_id, top.utility_rate, top.priority_bucket
        )));
    }
    premises
}

impl DecisionFamily for RecruitFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, RECRUIT_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        json_param::<RecruitRequest>(request, PARAM_RECRUIT).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let asked: RecruitRequest = json_param(request, PARAM_RECRUIT)?;
            let result = recruit(&asked);
            let body = serde_json::to_string(&result)
                .map_err(|error| invalid(format!("recruit encoding failed: {error}")))?;
            let mut executed = advisory(vec![format!("{ANSWER_PREFIX}{body}")])?;
            executed.family_premises = claims(&result);
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

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(match outcome {
            StatisticalOutcome::Abstained { .. } => ResolutionKind::Abstention,
            _ => ResolutionKind::Constraint,
        })
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Claim)
    }
}
