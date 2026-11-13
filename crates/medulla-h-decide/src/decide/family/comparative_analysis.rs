//! The `comparative_analysis_plan` question family (MEDULLA-DECISION-ENGINE-R139.3).
//!
//! Evaluate-only budgeting of a comparative analysis. The request body
//! travels in the typed param `comparative_analysis`; its candidates must be
//! visible in the caller's tenant library at the revision the caller names.
//! The sealed answer is the plan; the engine never invokes a model, and the
//! scores and importances in the plan are caller claims, so the evidence
//! class is `Claim` and the resolution is `Constraint`.

use medulla_agent_types::agent_component::AgentComponentEntry;
use medulla_decision_stats::decision::comparative_analysis::plan_comparative_analysis;
use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::comparative_analysis::{
    ComparativeAnalysisPlan, ComparativeAnalysisRequest, ComparativeAnalysisStage,
    ComparativeKnobs, COMPARATIVE_CANDIDATE_INVALID, COMPARATIVE_STALE_INPUT,
};
use medulla_decision_types::decision::record::outcome::RefusalCode;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, QuestionSafety, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseProvenance, PremiseRef, QuantisedValue,
    ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use crate::decide::stat_executor::{Executed, ExecutionContext};
use crate::decide::stat_support::invalid_parameter;

use super::DecisionFamily;

/// The name of the typed param carrying the request body.
pub(in crate::decide) const PARAM: &str = "comparative_analysis";

/// The family of the `comparative_analysis_plan` question kind.
pub struct ComparativeAnalysisPlanFamily;

/// A refusal as the wire error: `PARAMETER_INVALID` naming the registered code.
fn refused(code: RefusalCode, detail: impl std::fmt::Display) -> String {
    invalid_parameter(format!("{}: {detail}", code.as_str()))
}

/// The request body of a comparative-analysis question, or `None` for any
/// other kind. A body on another kind is refused.
fn body(request: &DecideRequest) -> Result<Option<&ComparativeAnalysisRequest>, String> {
    let carried = request
        .params
        .iter()
        .any(|p| matches!(p.value, TypedValue::ComparativeAnalysis(_)));
    if request.question.kind != QuestionKind::ComparativeAnalysisPlan {
        if carried {
            return Err(invalid_parameter(
                "comparative_analysis parameter requires comparative_analysis_plan question",
            ));
        }
        return Ok(None);
    }
    if request.question.safety != QuestionSafety::Ordinary
        || request.head.is_some()
        || !request.belief_as_of.is_empty()
        || request.params.len() != 1
    {
        return Err(invalid_parameter(
            "comparative_analysis_plan needs ordinary safety, no head or belief slices, and one parameter",
        ));
    }
    let param = &request.params.as_slice()[0];
    match &param.value {
        TypedValue::ComparativeAnalysis(body) if param.name == PARAM => Ok(Some(body)),
        _ => Err(invalid_parameter(
            "comparative_analysis_plan requires a typed comparative_analysis parameter",
        )),
    }
}

/// Every plan candidate names a visible library candidate at its current
/// revision. An id the caller cannot see refuses exactly as an id that does
/// not exist does, so a refusal never confirms another tenant's row.
fn check_visible(
    body: &ComparativeAnalysisRequest,
    entries: &[AgentComponentEntry],
) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for candidate in body.candidates.iter() {
        if !seen.insert(candidate.candidate_id.as_str()) {
            return Err(refused(
                COMPARATIVE_CANDIDATE_INVALID,
                format!("candidate {} is duplicated", candidate.candidate_id),
            ));
        }
        let Some(entry) = entries
            .iter()
            .find(|entry| entry.component_id == candidate.candidate_id)
        else {
            return Err(refused(
                COMPARATIVE_CANDIDATE_INVALID,
                format!("candidate {} is not visible", candidate.candidate_id),
            ));
        };
        if entry.entry_revision != candidate.candidate_revision {
            return Err(refused(
                COMPARATIVE_STALE_INPUT,
                format!(
                    "candidate {} is at revision {}, not {}",
                    candidate.candidate_id, entry.entry_revision, candidate.candidate_revision
                ),
            ));
        }
    }
    Ok(())
}

/// The plan of a `comparative_analysis_plan` question over the visible
/// library `entries`; `None` for any other kind.
pub(in crate::decide) fn plan(
    request: &DecideRequest,
    entries: &[AgentComponentEntry],
    tenant_id: &str,
    policy: &DecisionPolicy,
    policy_digest: &str,
) -> Result<Option<ComparativeAnalysisPlan>, String> {
    let Some(body) = body(request)? else {
        return Ok(None);
    };
    if !matches!(request.candidates, CandidateSource::AgentLibrary { .. }) {
        return Err(invalid_parameter(
            "comparative_analysis_plan candidates come from the agent library",
        ));
    }
    check_visible(body, entries)?;
    let knobs = ComparativeKnobs::from_policy(policy);
    plan_comparative_analysis(tenant_id, policy_digest, body, &knobs)
        .map(Some)
        .map_err(|refusal| refused(refusal.code(), refusal.detail()))
}

fn stage_name(stage: ComparativeAnalysisStage) -> &'static str {
    match stage {
        ComparativeAnalysisStage::Discovery => "discovery",
        ComparativeAnalysisStage::Synthesis => "synthesis",
        ComparativeAnalysisStage::Extraction => "extraction",
    }
}

/// A Q32 value as decimal millionths (`0.500000`), rounded down.
fn decimal(value: &QuantisedValue) -> String {
    let micros = (i128::from(value.value) * 1_000_000) >> 32;
    format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

/// One `Claim` premise per input the plan binds, in plan order, each with
/// `Request` provenance (MEDULLA-DECISION-ENGINE-R139.5): the caller supplied
/// every one of them, and the weakest class is `Claim`.
pub(in crate::decide) fn premises(plan: &ComparativeAnalysisPlan) -> Vec<PremiseRef> {
    let claim = |fact: String| PremiseRef {
        subject: "comparative".to_string(),
        fact,
        class: PremiseClass::Claim,
        provenance: PremiseProvenance::Request {
            field: PARAM.to_string(),
        },
    };
    let mut out = vec![
        claim(format!(
            "comparative:space={}@{}",
            plan.space_id, plan.space_revision
        )),
        claim(format!(
            "comparative:query_vector={}",
            plan.query_vector_digest
        )),
        claim(format!(
            "comparative:result={}",
            plan.unified_query_result_digest
        )),
    ];
    out.extend(plan.candidates.iter().map(|candidate| {
        claim(format!(
            "comparative:candidate={}@{}/{}/{}",
            candidate.candidate_id,
            candidate.candidate_revision,
            decimal(&candidate.score),
            decimal(&candidate.importance)
        ))
    }));
    out.extend(plan.stage_budgets.iter().map(|budget| {
        claim(format!(
            "comparative:stage={}/{}",
            stage_name(budget.stage),
            budget.max_calls
        ))
    }));
    out
}

impl DecisionFamily for ComparativeAnalysisPlanFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        body(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        _request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        // The plan itself is derived once in the Candidates-to-Constraints
        // stages (it needs the visible set) and sealed as the record's answer;
        // the outcome is advisory and carries no calibrated score.
        Some(Ok(Executed {
            matrix: None,
            ladder: LadderResult {
                outcome: StatisticalOutcome::Advisory {
                    scores: BoundedVec::default(),
                    calibrated: false,
                },
                calibration: None,
                explained: None,
                audit: None,
                logging: Vec::new(),
            },
            reading: None,
            seed: [0u8; 32],
            harness_contrast: None,
            topology: None,
            context_selection: None,
            thought: None,
            family_premises: Vec::new(),
            route: None,
        }))
    }

    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(ResolutionKind::Constraint)
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        // Scores and importance are caller claims.
        Some(EvidenceClass::Claim)
    }
}
