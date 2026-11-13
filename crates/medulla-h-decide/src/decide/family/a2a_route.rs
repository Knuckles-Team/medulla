//! The `a2a_route` question family (MEDULLA-DECISION-ENGINE-R044.2.2).
//!
//! An inbound A2A task names the agent graph it wants in the text parameter
//! `a2a.requested_graph_id`. The candidates are the tenant's visible published
//! graphs of the request's `AgentLibrary` scope, read through the same tenant-
//! bound search every library question uses. An exact match routes to that
//! graph (`Constraint`, `Observation`); no match abstains with `Claim` evidence.
//! `Decide` is evaluate-only and writes nothing.

use medulla_decision_outcome_types::decision::rapid_r044::A2ARoutingResolution;
use medulla_decision_types::decision::a2a_route::A2aRouteRefusal;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;

use crate::decide::candidates::library_candidates;
use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{abstained, advisory, invalid, text_param};
use super::DecisionFamily;

/// The family of the `a2a_route` question kind.
pub struct A2aRouteFamily;

impl DecisionFamily for A2aRouteFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        match request.candidates {
            CandidateSource::AgentLibrary { .. } => Ok(()),
            _ => Err(invalid(
                "an a2a_route question reads the tenant's published graphs from an agent library scope",
            )),
        }
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(route(ctx, request))
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
        Some(match outcome {
            StatisticalOutcome::Advisory { .. } => EvidenceClass::Observation,
            _ => EvidenceClass::Claim,
        })
    }
}

fn route(ctx: &ExecutionContext, request: &DecideRequest) -> Result<Executed, String> {
    let CandidateSource::AgentLibrary { scope } = &request.candidates else {
        return Err(invalid(
            "an a2a_route question reads an agent library scope",
        ));
    };
    let requested = match text_param(request, "a2a.requested_graph_id")? {
        Some(text) if !text.is_empty() => text,
        _ => {
            return Ok(abstained(
                QuestionKind::A2aRoute,
                &A2aRouteRefusal::RequestMissing,
            ))
        }
    };
    let read = library_candidates(ctx.store, ctx.tenant_id, scope)?;
    let known: Vec<String> = read
        .entries
        .iter()
        .map(|e| e.component_id.clone())
        .collect();
    match A2ARoutingResolution::route(&known, requested) {
        Ok(resolution) if resolution.resolution_kind == ResolutionKind::Constraint => {
            let entry = read
                .entries
                .iter()
                .find(|e| e.component_id == resolution.graph_id)
                .ok_or_else(|| invalid("the routed graph left the candidate set"))?;
            let mut executed = advisory(vec![resolution.graph_id.clone()])?;
            executed.family_premises = vec![FamilyPremise {
                subject: "a2a_route",
                fact: format!("a2a_route:{}", resolution.graph_id),
                class: PremiseClass::Observation,
                source: FamilyPremiseSource::Publisher {
                    component_id: entry.component_id.clone(),
                    definition_digest: entry.content_digest.clone(),
                },
            }];
            Ok(executed)
        }
        Ok(_) | Err(_) => Ok(abstained(
            QuestionKind::A2aRoute,
            &A2aRouteRefusal::GraphUnknown {
                requested: requested.to_string(),
            },
        )),
    }
}
