//! What the pure-kernel question families share (MEDULLA-DECISION-ENGINE-R751.755,
//! R752.754, R753.754, R753.755): request shape checks, the one typed body,
//! the claim premise and the advisory outcome. Each family decodes its own body
//! and calls one pure kernel of `medulla-decision-stats`; none of them reads a
//! store, so a request never reaches another tenant's data.

use medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::record::AbstainReason;
use medulla_types::contract::BoundedVec;
use medulla_types::evidence_class::EvidenceClass;
use medulla_types::refusal_code::RefusalCode;
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseProvenance,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
};
use {
    medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind,
    medulla_decision_types::decision::statistical::CandidateSource,
    medulla_decision_types::decision::statistical::DecideRequest,
    medulla_decision_types::decision::statistical::QuestionSafety,
    medulla_decision_types::decision::statistical::StatisticalOutcome,
    medulla_decision_types::decision::statistical::TypedValue,
};

use crate::decide::stat_support::invalid_parameter;

/// The request shape every kernel family takes: ordinary safety, no head, no
/// belief slices, and one declared option per subject without facts.
pub fn shape<'a>(request: &'a DecideRequest, kind: &str) -> Result<&'a [DeclaredOption], String> {
    if request.question.safety != QuestionSafety::Ordinary
        || request.head.is_some()
        || !request.belief_as_of.is_empty()
    {
        return Err(invalid_parameter(format!(
            "{kind} needs ordinary safety, no head and no belief slices"
        )));
    }
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid_parameter(format!("{kind} declares its candidates")));
    };
    if options
        .iter()
        .any(|o| !o.classification.is_empty() || !o.numbers.is_empty() || !o.texts.is_empty())
    {
        return Err(invalid_parameter(format!(
            "{kind} candidates carry no facts"
        )));
    }
    Ok(options.as_slice())
}

/// The one typed body parameter, named `name`; no other parameter is allowed
/// beside the policy's safety measurements, which the safety stage reads
/// (MEDULLA-DECISION-ENGINE-R001.100.2).
pub fn body<'a>(
    request: &'a DecideRequest,
    kind: &str,
    name: &str,
) -> Result<&'a TypedValue, String> {
    let mut own = request
        .params
        .iter()
        .filter(|param| param.name != super::super::safety_stage::MEASUREMENTS_PARAM);
    match (own.next(), own.next()) {
        (Some(param), None) if param.name == name => Ok(&param.value),
        _ => Err(invalid_parameter(format!(
            "{kind} takes exactly one parameter, {name}"
        ))),
    }
}

/// The single declared option of a question about one subject.
pub fn one_option<'a>(options: &'a [DeclaredOption], kind: &str) -> Result<&'a str, String> {
    match options {
        [only] => Ok(only.option_id.as_str()),
        _ => Err(invalid_parameter(format!(
            "{kind} declares exactly one candidate: its subject"
        ))),
    }
}

/// A CLAIM premise on one request field: the caller supplied the value.
pub fn claim(subject: &str, fact: String, field: &str) -> PremiseRef {
    PremiseRef {
        subject: subject.to_string(),
        fact,
        class: PremiseClass::Claim,
        provenance: PremiseProvenance::Request {
            field: field.to_string(),
        },
        trust_tier: None,
    }
}

/// The outcome of a kernel family: an uncalibrated advisory, because every
/// input is a caller claim and the answer authorizes nothing.
pub fn advisory() -> StatisticalOutcome {
    StatisticalOutcome::advisory(BoundedVec::default(), false)
}

/// An abstention of `kind` under its registered `code`: the request is well
/// formed but its evidence does not support an answer (DE-REFUSE-VS-ABSTAIN).
pub fn abstained(
    kind: QuestionKind,
    code: RefusalCode,
    detail: String,
) -> Result<StatisticalOutcome, String> {
    Ok(StatisticalOutcome::Abstained {
        reasons: BoundedVec::new(vec![AbstainReason::Family {
            family: kind,
            code,
            detail,
        }])?,
    })
}

/// The wire label of an evidence class.
pub fn evidence_label(evidence: EvidenceClass) -> &'static str {
    match evidence {
        EvidenceClass::Proof => "proof",
        EvidenceClass::Observation => "observation",
        EvidenceClass::Claim => "claim",
    }
}
