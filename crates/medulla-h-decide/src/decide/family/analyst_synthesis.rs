//! The `analyst_synthesis` question family (MEDULLA-DECISION-ENGINE-R134.13).
//!
//! Accounts for every expected analyst of one plan, refuses conflicting
//! claims and resolves each citation against the records the caller's graph
//! view (named by the `source_graph` parameter) makes visible. A record the
//! caller cannot see is indistinguishable from an absent one. Any failure
//! abstains under its registered code with no recommendation. Evaluate-only.

use medulla_decision_stats::decision::analyst_synthesis::synthesize;
use medulla_decision_types::decision::analyst_synthesis::{min_citations, SynthesisRequest};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{abstained, advisory, invalid, json_param, single_option, text_param};
use super::DecisionFamily;

/// The one option an analyst-synthesis question declares.
pub const SYNTHESIS_OPTION: &str = "synthesis";
/// The prefix of the answer option that carries the synthesis as JSON.
pub const ANSWER_PREFIX: &str = "synthesis:";
/// The request parameter carrying the findings and gaps.
pub const PARAM_SYNTHESIS: &str = "synthesis";
/// The request parameter naming the plan the analysts ran.
pub const PARAM_EXPECTED_PLAN: &str = "expected_plan_digest";

/// The family of the `analyst_synthesis` question kind.
pub struct AnalystSynthesisFamily;

fn expected_plan(request: &DecideRequest) -> Result<&str, String> {
    match text_param(request, PARAM_EXPECTED_PLAN)? {
        Some(text) if !text.trim().is_empty() => Ok(text),
        _ => Err(invalid(
            "an analyst_synthesis question requires an expected_plan_digest",
        )),
    }
}

fn claim(fact: String) -> FamilyPremise {
    FamilyPremise {
        subject: "analyst_synthesis",
        fact,
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request(PARAM_SYNTHESIS),
    }
}

impl DecisionFamily for AnalystSynthesisFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, SYNTHESIS_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        expected_plan(request)?;
        json_param::<SynthesisRequest>(request, PARAM_SYNTHESIS).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let asked: SynthesisRequest = json_param(request, PARAM_SYNTHESIS)?;
            let plan = expected_plan(request)?;
            let view = graph_view.ok_or_else(|| {
                invalid(
                    "an analyst_synthesis question reads its evidence from the named source graph",
                )
            })?;
            let digest_of = |evidence_id: &str| {
                view.node_row_object(evidence_id).and_then(|node| {
                    node.get("content_digest")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
            };
            let result = match synthesize(&asked, plan, min_citations(policy), digest_of) {
                Ok(result) => result,
                Err(refused) => return Ok(abstained(QuestionKind::AnalystSynthesis, &refused)),
            };
            let body = serde_json::to_string(&result)
                .map_err(|error| invalid(format!("synthesis encoding failed: {error}")))?;
            let mut executed = advisory(vec![format!("{ANSWER_PREFIX}{body}")])?;
            executed.family_premises = asked
                .findings
                .iter()
                .map(|finding| {
                    claim(format!(
                        "finding:analyst={};claim_key={};citations={}",
                        finding.analyst_id,
                        finding.claim_key,
                        finding.citations.len()
                    ))
                })
                .collect();
            Ok(executed)
        })())
    }

    fn premises(
        &self,
        request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        // An abstention keeps the findings it was asked to judge: one `Claim` each.
        if executed.family_premises.is_empty() {
            let Ok(asked) = json_param::<SynthesisRequest>(request, PARAM_SYNTHESIS) else {
                return Vec::new();
            };
            return asked
                .findings
                .iter()
                .map(|finding| {
                    claim(format!(
                        "finding:analyst={};claim_key={};citations={}",
                        finding.analyst_id,
                        finding.claim_key,
                        finding.citations.len()
                    ))
                    .seal(policy_digest)
                })
                .collect();
        }
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
