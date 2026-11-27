//! The `reasoning_paradigm` question family (MEDULLA-DECISION-ENGINE-R823).
//!
//! Four declared options (`rules`, `owl`, `causal`, `statistical`), each
//! naming the capability tag its paradigm requires. A paradigm whose tag the
//! task (`task_tags`) does not offer is illegal; the scorer ranks only the legal
//! set by expected value minus cost, and an empty legal set abstains
//! `PARADIGM_NONE_ADMISSIBLE`. Evaluate-only: `Decide` writes nothing.

use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::reasoning_paradigm::{
    legal_paradigms, NoParadigmAdmissible, COST_KEY, EXPECTED_VALUE_KEY, PARADIGMS, PARAM_TASK_TAGS,
};
use medulla_decision_types::decision::statistical::{CandidateSource, DecideRequest, QuestionKind};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;

use crate::decide::candidates::declared_number;
use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{abstained, advisory_scored, invalid, param};
use super::DecisionFamily;

/// The family of the `reasoning_paradigm` question kind.
pub struct ReasoningParadigmFamily;

/// The task's capability tags, ascending and unique; absent, empty or another
/// type refuses `PARAMETER_INVALID`.
fn task_tags(request: &DecideRequest) -> Result<Vec<String>, String> {
    let Some(medulla_decision_types::decision::statistical::TypedValue::IriList(list)) =
        param(request, PARAM_TASK_TAGS)?
    else {
        return Err(invalid(format!(
            "parameter {PARAM_TASK_TAGS} is required and must be an IRI list"
        )));
    };
    let mut tags = list.as_slice().to_vec();
    if tags.is_empty() {
        return Err(invalid(format!("parameter {PARAM_TASK_TAGS} is empty")));
    }
    tags.sort();
    tags.dedup();
    Ok(tags)
}

fn options(request: &DecideRequest) -> &[DeclaredOption] {
    match &request.candidates {
        CandidateSource::Declared { options } => options.as_slice(),
        _ => &[],
    }
}

/// The premises: one `task_tag:<tag>` per tag, then `paradigm_legal:<option>` or
/// `paradigm_illegal:<option>` per paradigm, all `Observation`.
fn family_premises(tags: &[String], legal: &[&str]) -> Vec<FamilyPremise> {
    let observed = |fact: String| FamilyPremise {
        subject: "reasoning_paradigm".into(),
        fact,
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Request("params.task_tags"),
    };
    tags.iter()
        .map(|tag| observed(format!("task_tag:{tag}")))
        .chain(PARADIGMS.iter().map(|(option, _)| {
            let verdict = if legal.contains(option) {
                "paradigm_legal"
            } else {
                "paradigm_illegal"
            };
            observed(format!("{verdict}:{option}"))
        }))
        .collect()
}

fn choose(request: &DecideRequest) -> Result<Executed, String> {
    let tags = task_tags(request)?;
    let legal = legal_paradigms(&tags);
    let mut executed = if legal.is_empty() {
        abstained(QuestionKind::ReasoningParadigm, &NoParadigmAdmissible)
    } else {
        let mut scored: Vec<(String, i64)> = options(request)
            .iter()
            .filter(|option| legal.contains(&option.option_id.as_str()))
            .map(|option| {
                let net = declared_number(option, EXPECTED_VALUE_KEY)
                    .saturating_sub(declared_number(option, COST_KEY));
                (option.option_id.clone(), net)
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        advisory_scored(scored)?
    };
    executed.family_premises = family_premises(&tags, &legal);
    Ok(executed)
}

impl DecisionFamily for ReasoningParadigmFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        let exact = options.len() == PARADIGMS.len()
            && options.iter().zip(PARADIGMS).all(|(option, (id, tag))| {
                option.option_id == id
                    && option.classification.as_slice().first().map(String::as_str) == Some(tag)
            });
        if exact {
            Ok(())
        } else {
            Err(invalid(
                "a reasoning paradigm question declares exactly the options causal, owl, rules and statistical, each classified by its reasoning tag",
            ))
        }
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(choose(request))
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
