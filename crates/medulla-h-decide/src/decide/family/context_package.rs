//! The `context_package` question family (MEDULLA-DECISION-ENGINE-R137.8,
//! MEDULLA-DECISION-ENGINE-R137.9).
//!
//! Which measured evidence spans fit a model window under token, money and
//! latency ceilings. The package arrives in the typed `context_packages`
//! parameter. The family lowers it and solves it exactly through the one
//! package kernel (`medulla_decision_policy::solve::context_package::select`),
//! which verifies the certificate independently and refuses anything but a
//! proven optimum. The answer is an advisory with one scored option per
//! chosen candidate; a proven optimum is sealed as an optimization with proof
//! evidence.

use std::collections::BTreeSet;

use medulla_decision_policy::solve::context_package::select;
use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::context_package::ContextPackageRequest;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, ScoredOption, StatisticalErrorCode, StatisticalOutcome,
    TypedValue,
};
use medulla_decision_types::decision::{
    EvidenceClass, QuantScaleTag, QuantisedValue, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use crate::decide::stat_executor::{Executed, ExecutionContext};
use crate::decide::stat_support::refusal;

use super::DecisionFamily;

/// The Q32 fixed-point value for "chosen".
const CHOSEN_SCORE: i64 = 1i64 << 32;

/// The family of the `context_package` question kind.
pub struct ContextPackageFamily;

fn invalid(detail: impl std::fmt::Display) -> String {
    refusal(StatisticalErrorCode::ParameterInvalid, detail)
}

/// The one `context_packages` parameter, validated.
fn package(request: &DecideRequest) -> Result<&ContextPackageRequest, String> {
    let mut found = request
        .params
        .iter()
        .filter(|param| param.name == "context_packages");
    let (Some(param), None) = (found.next(), found.next()) else {
        return Err(invalid(
            "a context_package question carries exactly one context_packages parameter",
        ));
    };
    let TypedValue::ContextPackages(package) = &param.value else {
        return Err(invalid("context_packages is not a context package"));
    };
    package
        .validate()
        .map_err(|(code, detail)| invalid(format!("{}: {detail}", code.as_str())))?;
    Ok(package)
}

impl DecisionFamily for ContextPackageFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        let package = package(request)?;
        let CandidateSource::Declared { options } = &request.candidates else {
            return Err(invalid(
                "a context_package question declares its candidates",
            ));
        };
        let declared: BTreeSet<&str> = options.iter().map(|o| o.option_id.as_str()).collect();
        let packaged: BTreeSet<&str> = package
            .candidates
            .iter()
            .map(|c| c.candidate_id.as_str())
            .collect();
        if declared != packaged || declared.len() != options.len() {
            return Err(invalid(
                "declared options must equal the package candidate ids",
            ));
        }
        Ok(())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &medulla_decision_types::decision::DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(solve(request))
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. })
            .then_some(ResolutionKind::Optimization)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Proof)
    }
}

fn solve(request: &DecideRequest) -> Result<Executed, String> {
    let solved = select(package(request)?)
        .map_err(|refused| invalid(format!("{}: {}", refused.code.as_str(), refused.detail)))?;
    let scores: Vec<ScoredOption> = solved
        .selection
        .chosen_ids
        .iter()
        .map(|id| ScoredOption {
            option_id: id.clone(),
            score: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: CHOSEN_SCORE,
            },
            probability: None,
        })
        .collect();
    let scores = BoundedVec::new(scores)
        .map_err(|detail| refusal(StatisticalErrorCode::CandidateSetTooLarge, detail))?;
    Ok(Executed {
        matrix: None,
        ladder: LadderResult {
            outcome: StatisticalOutcome::Advisory {
                scores,
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
    })
}
