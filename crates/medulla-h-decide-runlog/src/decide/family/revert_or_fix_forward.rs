//! The `revert_or_fix_forward` question family (MEDULLA-DECISION-ENGINE-R827).
//!
//! The caller names the culprit changeset a bisect found, the changesets that depend on it and
//! the gate verdicts recorded for it. `Revert` is legal only when no changeset depends on the
//! culprit; `FixForward` is always legal. The legal subset of the declared options is ranked
//! (`revert` before `fix_forward`) and an answer never names an illegal option. A question that
//! declares only `revert` for a culprit with dependents abstains `REVERT_HAS_DEPENDENTS`.
//! `Decide` is evaluate-only: it writes nothing.

use medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption;
use medulla_graph_core::graph::GraphView;
use medulla_types::evidence_class::EvidenceClass;
use medulla_work_types::step_verdict::VerificationVerdict;
use {
    medulla_decision_program_types::decision::revert_or_fix_forward::legal_ranked,
    medulla_decision_program_types::decision::revert_or_fix_forward::RecordedVerdict,
    medulla_decision_program_types::decision::revert_or_fix_forward::RevertRefusal,
    medulla_decision_program_types::decision::revert_or_fix_forward::OPTION_FIX_FORWARD,
    medulla_decision_program_types::decision::revert_or_fix_forward::OPTION_REVERT,
    medulla_decision_program_types::decision::revert_or_fix_forward::PARAM_CULPRIT,
    medulla_decision_program_types::decision::revert_or_fix_forward::PARAM_DEPENDENTS,
    medulla_decision_program_types::decision::revert_or_fix_forward::PARAM_VERDICTS,
};
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    medulla_decision_statistical_types::decision::DecisionPolicy,
};
use {
    medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind,
    medulla_decision_types::decision::statistical::CandidateSource,
    medulla_decision_types::decision::statistical::DecideRequest,
    medulla_decision_types::decision::statistical::StatisticalOutcome,
};

use {
    medulla_h_decide_base::decide::stat_executor::Executed,
    medulla_h_decide_base::decide::stat_executor::ExecutionContext,
    medulla_h_decide_base::decide::stat_executor::FamilyPremise,
    medulla_h_decide_base::decide::stat_executor::FamilyPremiseSource,
};

use medulla_h_decide_base::decide::family::DecisionFamily;
use {
    medulla_h_decide_base::decide::family::support::abstained,
    medulla_h_decide_base::decide::family::support::advisory_scored,
    medulla_h_decide_base::decide::family::support::invalid,
    medulla_h_decide_base::decide::family::support::json_param,
    medulla_h_decide_base::decide::family::support::text_param,
};

/// The family of the `revert_or_fix_forward` question kind.
pub struct RevertOrFixForwardFamily;

/// Q32 score of the first-ranked legal option; each later option scores one step less.
const TOP_SCORE: i64 = 1i64 << 33;
const SCORE_STEP: i64 = 1i64 << 32;

struct Inputs {
    culprit: String,
    dependents: Vec<String>,
    verdicts: Vec<RecordedVerdict>,
}

fn inputs_of(request: &DecideRequest) -> Result<Inputs, String> {
    let culprit = text_param(request, PARAM_CULPRIT)?
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| invalid("a revert_or_fix_forward question requires culprit"))?
        .to_string();
    let dependents: Vec<String> = json_param(request, PARAM_DEPENDENTS)?;
    if dependents
        .iter()
        .any(|id| id.trim().is_empty() || *id == culprit)
    {
        return Err(invalid(
            "dependents name changesets other than the culprit, none of them blank",
        ));
    }
    let verdicts: Vec<RecordedVerdict> = json_param(request, PARAM_VERDICTS)?;
    // A changeset no recorded gate failed on is not a culprit.
    if !verdicts
        .iter()
        .any(|recorded| recorded.verdict == VerificationVerdict::Fail)
    {
        return Err(invalid(
            "a revert_or_fix_forward question needs a recorded failing verdict of the culprit",
        ));
    }
    Ok(Inputs {
        culprit,
        dependents,
        verdicts,
    })
}

fn declared_ids(request: &DecideRequest) -> Result<Vec<String>, String> {
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid(
            "a revert_or_fix_forward question declares its options",
        ));
    };
    Ok(options
        .iter()
        .map(|option| option.option_id.clone())
        .collect())
}

fn premise(fact: String, culprit: &str) -> FamilyPremise {
    FamilyPremise {
        subject: "revert_or_fix_forward".into(),
        fact,
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Observation(culprit.to_string()),
    }
}

fn conclude_over(request: &DecideRequest) -> Result<Executed, String> {
    let inputs = inputs_of(request)?;
    let declared = declared_ids(request)?;
    let legal: Vec<&'static str> = legal_ranked(&inputs.dependents)
        .into_iter()
        .filter(|option| declared.iter().any(|id| id == option))
        .collect();
    if legal.is_empty() {
        return Ok(abstained(
            QuestionKind::RevertOrFixForward,
            &RevertRefusal {
                dependents: inputs.dependents.len(),
            },
        ));
    }
    let scored = legal
        .iter()
        .enumerate()
        .map(|(rank, option)| (option.to_string(), TOP_SCORE - SCORE_STEP * rank as i64))
        .collect();
    let mut executed = advisory_scored(scored)?;
    let failing = inputs
        .verdicts
        .iter()
        .filter(|recorded| recorded.verdict == VerificationVerdict::Fail)
        .count();
    executed.family_premises = vec![
        premise(
            format!(
                "revert_or_fix_forward.dependents={}",
                inputs.dependents.len()
            ),
            &inputs.culprit,
        ),
        premise(
            format!("revert_or_fix_forward.failing_verdicts={failing}"),
            &inputs.culprit,
        ),
        premise(
            format!("revert_or_fix_forward.legal={}", legal.join(",")),
            &inputs.culprit,
        ),
    ];
    Ok(executed)
}

impl DecisionFamily for RevertOrFixForwardFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        if options.is_empty()
            || options.iter().any(|option| {
                option.option_id != OPTION_REVERT && option.option_id != OPTION_FIX_FORWARD
            })
        {
            return Err(invalid(format!(
                "a revert_or_fix_forward question declares `{OPTION_REVERT}`, `{OPTION_FIX_FORWARD}` or both"
            )));
        }
        Ok(())
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        inputs_of(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_over(request))
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

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Observation)
    }
}
