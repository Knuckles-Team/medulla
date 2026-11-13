//! The `temporal_basis` question family (MEDULLA-DECISION-ENGINE-R146.3).
//!
//! A time-windowed query that finds too few rows may have read the wrong
//! clock. The caller declares the three bases (`event`, `narrative`,
//! `storage`) and passes the query with the three dates of the node it read.
//! The family marks a basis legal when its date is present, picks the first
//! legal one in basis order and, when the first pass was insufficient, names
//! the widened fallback window as premises. Medulla runs no query: the caller
//! executes the fallback through `Method::UnifiedQuery`. `Decide` is
//! evaluate-only and writes nothing; every date is the caller's claim.

use medulla_decision_stats::decision::temporal_basis::{
    confusion, fallback_windows, needs_fallback,
};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, QuestionSafety, StatisticalErrorCode,
    StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::temporal_query::{
    TemporalKnobs, TemporalQueryInput, TemporalRefusal, TimeBasis, BASIS_OPTIONS,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_support::refusal;

use super::support::{abstained, advisory, invalid, param};
use super::DecisionFamily;

/// The family of the `temporal_basis` question kind.
pub struct TemporalBasisFamily;

const BASES: [TimeBasis; 3] = [TimeBasis::Event, TimeBasis::Narrative, TimeBasis::Storage];

/// The declared options must be exactly the three bases.
fn options_are_the_bases(request: &DecideRequest) -> Result<(), String> {
    let unmappable =
        |detail: String| refusal(StatisticalErrorCode::TemporalBasisUnmappable, detail);
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(unmappable(
            "a temporal_basis question declares its bases".to_string(),
        ));
    };
    let mut ids: Vec<&str> = options.iter().map(|o| o.option_id.as_str()).collect();
    ids.sort_unstable();
    let mut expected = BASIS_OPTIONS.to_vec();
    expected.sort_unstable();
    if ids != expected {
        return Err(unmappable(format!(
            "declared options {ids:?} are not {BASIS_OPTIONS:?}"
        )));
    }
    Ok(())
}

/// The temporal query the request carries: `None` when the parameter is
/// absent, a refusal when it is not a temporal query or its window is inverted.
fn query(request: &DecideRequest) -> Result<Option<&TemporalQueryInput>, String> {
    match param(request, "temporal_query")? {
        None => Ok(None),
        Some(TypedValue::TemporalQuery(input)) => {
            if input.window_from_ms > input.window_to_ms {
                return Err(refusal(
                    StatisticalErrorCode::TemporalWindowInvalid,
                    format!(
                        "window {}..{} is inverted",
                        input.window_from_ms, input.window_to_ms
                    ),
                ));
            }
            Ok(Some(input))
        }
        Some(_) => Err(invalid("parameter temporal_query is not a temporal query")),
    }
}

fn claim(subject: &'static str, fact: String) -> FamilyPremise {
    FamilyPremise {
        subject,
        fact,
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request("params.temporal_query"),
    }
}

fn conclude_basis(request: &DecideRequest, policy: &DecisionPolicy) -> Result<Executed, String> {
    options_are_the_bases(request)?;
    if request.question.safety != QuestionSafety::Ordinary {
        return Err(invalid("a temporal_basis question is Ordinary"));
    }
    let Some(input) = query(request)? else {
        return Ok(abstained(
            QuestionKind::TemporalBasis,
            &TemporalRefusal::ContextMissing {
                field: "params.temporal_query",
            },
        ));
    };
    let context = &input.context;
    if !context.has_usable_date() {
        return Ok(abstained(
            QuestionKind::TemporalBasis,
            &TemporalRefusal::ContextMissing {
                field: "params.temporal_query.context",
            },
        ));
    }
    let knobs = TemporalKnobs::from_policy(policy);
    // The first legal basis in basis order; the context has a usable date, so one exists.
    let basis = BASES
        .into_iter()
        .find(|basis| context.has_date(*basis))
        .unwrap_or(TimeBasis::Storage);
    let confused = confusion(context, knobs.confusion_gap_days);
    let window = if needs_fallback(input.first_pass_rows, knobs.min_hits) {
        fallback_windows(input.window_from_ms, input.window_to_ms, &knobs)
            .into_iter()
            .next()
    } else {
        None
    };
    let mut executed = advisory(vec![basis.option_id().to_string()])?;
    let mut premises = vec![
        claim(
            "temporal_query",
            format!("temporal:query={}", input.query_digest.to_hex()),
        ),
        claim(
            "temporal_basis",
            format!("temporal:basis={}", basis.option_id()),
        ),
        claim(
            "temporal_confusion",
            format!("temporal:confusion={confused}"),
        ),
        claim(
            "temporal_first_pass_rows",
            format!("temporal:first_pass_rows={}", input.first_pass_rows),
        ),
        claim(
            "temporal_fallback_built",
            format!("temporal:fallback_built={}", window.is_some()),
        ),
    ];
    if let Some((from_ms, to_ms)) = window {
        premises.push(claim(
            "temporal_fallback_from_ms",
            format!("temporal:fallback_from_ms={from_ms}"),
        ));
        premises.push(claim(
            "temporal_fallback_to_ms",
            format!("temporal:fallback_to_ms={to_ms}"),
        ));
    }
    executed.family_premises = premises;
    Ok(executed)
}

impl DecisionFamily for TemporalBasisFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        options_are_the_bases(request)?;
        query(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_basis(request, policy))
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
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(ResolutionKind::Constraint)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Claim)
    }
}
