//! The `paper_admission` question family (MEDULLA-DECISION-ENGINE-R031.100).
//!
//! The candidates are staged paper nodes of a graph source. Two features feed
//! the pinned head: BM25 of the paper summary against the platform goals and
//! the cosine similarity to the nearest other row of the named graph. A paper
//! is admitted when the head's CALIBRATED probability reaches
//! `enrichment.relevance_min`; a caller- or model-supplied score never is
//! relevance. Each admitted paper's premises name its relevance, both feature
//! readings and the claim ids it carries. `Decide` is evaluate-only.

use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_decision_stats::decision::candidate::CandidateView;
use medulla_decision_stats::decision::quant::q32;
use medulla_decision_types::decision::codes::UNCALIBRATED_CANDIDATE;
use medulla_decision_types::decision::paper_admission::{
    nearest_neighbour_param, relevance_min_micros, PaperAdmissionRefusal, PARAM_GRAPH_VERSION,
    PARAM_OBSERVED_VERSION, TEXT_CLAIM_IDS, TEXT_CONTENT_DIGEST,
};
use medulla_decision_types::decision::statistical::features::FeatureKind;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, ScoredOption, StatisticalOutcome, TypedParam,
    TypedValue,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, EvidenceClass, PremiseClass, PremiseRef,
};
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::BoundedVec;
use tokio::sync::RwLock;

use crate::decide::stat_executor::{
    pinned_inputs, Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource, Pinned,
};
use crate::decide::stat_vectors::graph_vectors;

use super::support::{abstained, int_param, invalid, text_param};
use super::DecisionFamily;

/// The family of the `paper_admission` question kind.
pub struct PaperAdmissionFamily;

fn graph_source(request: &DecideRequest) -> Result<(), String> {
    #[cfg(feature = "query")]
    if matches!(request.candidates, CandidateSource::Graph { .. }) {
        return Ok(());
    }
    let _ = request;
    Err(invalid(
        "a paper_admission question reads staged papers from a graph source",
    ))
}

/// The probability of `scored` in millionths, floored.
fn micros(scored: &ScoredOption) -> Option<i64> {
    let p = scored.probability?;
    let micros = u128::from(p.numerator()) * 1_000_000 / u128::from(p.denominator());
    i64::try_from(micros).ok()
}

/// The matrix column the feature of `kind_matches` reads.
fn column_of(pinned: &Pinned, wanted: impl Fn(&FeatureKind) -> bool) -> Option<usize> {
    pinned
        .schema
        .features
        .iter()
        .position(|spec| wanted(&spec.kind))
}

fn premise(
    subject_fact: String,
    class: PremiseClass,
    source: FamilyPremiseSource,
) -> FamilyPremise {
    FamilyPremise {
        subject: "paper_admission",
        fact: subject_fact,
        class,
        source,
    }
}

impl DecisionFamily for PaperAdmissionFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        graph_source(request)
    }

    fn refine(
        &self,
        request: Option<&DecideRequest>,
        pinned: &Pinned,
        views: &[CandidateView],
        policy: &DecisionPolicy,
        _policy_digest: &str,
        mut executed: Executed,
    ) -> Result<Executed, String> {
        let Some(request) = request else {
            return Ok(executed);
        };
        let StatisticalOutcome::Advisory { scores, .. } = &executed.ladder.outcome else {
            return Ok(executed);
        };
        let staged = int_param(request, PARAM_GRAPH_VERSION)?
            .ok_or_else(|| invalid(format!("parameter {PARAM_GRAPH_VERSION} is required")))?;
        let observed = int_param(request, PARAM_OBSERVED_VERSION)?;
        if let Some(observed) = observed.filter(|observed| *observed != staged) {
            return Ok(abstained(
                QuestionKind::PaperAdmission,
                &PaperAdmissionRefusal::SnapshotStale { staged, observed },
            ));
        }
        let mut seen = BTreeMap::new();
        for view in views {
            let Some(digest) = view.texts.get(TEXT_CONTENT_DIGEST) else {
                continue;
            };
            if seen.insert(digest.clone(), view.id.clone()).is_some() {
                return Ok(abstained(
                    QuestionKind::PaperAdmission,
                    &PaperAdmissionRefusal::Duplicate {
                        digest: digest.clone(),
                    },
                ));
            }
        }
        let calibrated = pinned
            .head
            .as_ref()
            .is_some_and(|(head, _)| head.calibration.is_some());
        let probabilities = executed
            .reading
            .as_ref()
            .and_then(|reading| reading.probabilities.as_deref())
            .filter(|_| calibrated);
        let Some(probabilities) = probabilities else {
            let reasons = scores
                .iter()
                .map(|scored| AbstainReason::Family {
                    family: QuestionKind::PaperAdmission,
                    code: UNCALIBRATED_CANDIDATE,
                    detail: scored.option_id.clone(),
                })
                .collect::<Vec<_>>();
            executed.ladder.outcome = StatisticalOutcome::Abstained {
                reasons: BoundedVec::new(reasons)?,
            };
            return Ok(executed);
        };
        let floor = relevance_min_micros(policy);
        let bm25 = column_of(pinned, |kind| matches!(kind, FeatureKind::TextBm25 { .. }));
        let neighbour = column_of(pinned, |kind| {
            matches!(kind, FeatureKind::NearestNeighbourSimilarity { .. })
        });
        let mut admitted = Vec::new();
        let mut premises = Vec::new();
        for (index, scored) in scores.iter().enumerate() {
            let mut scored = scored.clone();
            scored.probability = probabilities
                .get(index)
                .and_then(|p| medulla_decision_stats::decision::quant::unit_wire(*p).ok());
            let Some(relevance) = micros(&scored).filter(|m| *m >= floor) else {
                continue;
            };
            let id = scored.option_id.clone();
            let observation = || FamilyPremiseSource::Observation(id.clone());
            premises.push(premise(
                format!("relevance:{id}={relevance}"),
                PremiseClass::Observation,
                observation(),
            ));
            if let Some(matrix) = &executed.matrix {
                if let Some(row) = matrix.candidate_ids.iter().position(|c| *c == id) {
                    let cells = matrix.row(row);
                    for (name, column) in [
                        ("bm25:platform.goals", bm25),
                        ("nearest_neighbour_similarity", neighbour),
                    ] {
                        if let Some(column) = column {
                            premises.push(premise(
                                format!("{name}:{id}={}", cells[column]),
                                PremiseClass::Observation,
                                observation(),
                            ));
                        }
                    }
                }
            }
            let claims = views
                .iter()
                .find(|view| view.id == id)
                .and_then(|view| view.texts.get(TEXT_CLAIM_IDS))
                .cloned()
                .unwrap_or_default();
            premises.push(premise(
                format!("{TEXT_CLAIM_IDS}:{id}={claims}"),
                PremiseClass::Claim,
                FamilyPremiseSource::Request("candidates.paper.claim_ids"),
            ));
            admitted.push(scored);
        }
        executed.ladder.outcome = StatisticalOutcome::Advisory {
            scores: BoundedVec::new(admitted)?,
            calibrated: true,
        };
        executed.family_premises = premises;
        Ok(executed)
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
        Some(match outcome {
            StatisticalOutcome::Advisory { .. } => EvidenceClass::Observation,
            _ => EvidenceClass::Claim,
        })
    }
}

/// The nearest-neighbour similarities of the staged papers, and the version of
/// the graph they were read at, recorded in the question's own parameters
/// before the matrix is computed (MEDULLA-DECISION-ENGINE-R031.100). Both
/// reads go through the caller's graph ACL and row-level filter.
#[cfg(feature = "query")]
pub(in crate::decide) async fn with_similarity(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    verified: &VerifiedRequestContext,
    mut request: DecideRequest,
) -> Result<DecideRequest, String> {
    let CandidateSource::Graph { graph, .. } = &request.candidates else {
        return Err(invalid(
            "a paper_admission question reads staged papers from a graph source",
        ));
    };
    let source = graph.clone();
    let ctx = ExecutionContext {
        store,
        tenant_id: &request.tenant_id,
        now_ms: 0,
        server_secret: b"",
        matched: &[],
        designation: None,
    };
    let pinned = pinned_inputs(&ctx, &request.feature_schema, None)?;
    let staged = graph_vectors(state, verified.agent_id(), &source).await?;
    let view = crate::decide::candidates::filtered_graph_view(
        state,
        verified.agent_id(),
        &request.candidates,
    )
    .await?
    .ok_or_else(|| invalid("a paper_admission question reads a graph"))?;
    let papers: Vec<String> = view
        .node_properties
        .keys()
        .filter(|id| {
            view.node_row_object(id)
                .is_some_and(|row| row.get(TEXT_CONTENT_DIGEST).is_some())
        })
        .cloned()
        .collect();
    let mut params = request.params.as_slice().to_vec();
    params.push(TypedParam {
        name: PARAM_OBSERVED_VERSION.to_string(),
        value: TypedValue::Int(i64::try_from(staged.version()).unwrap_or(i64::MAX)),
    });
    for spec in pinned.schema.features.iter() {
        let FeatureKind::NearestNeighbourSimilarity { graph_param } = &spec.kind else {
            continue;
        };
        let named = text_param(&request, graph_param)?
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| invalid(format!("parameter {graph_param} names the content graph")))?
            .to_string();
        let content = graph_vectors(state, verified.agent_id(), &named).await?;
        let mut nearest: BTreeMap<String, i64> = BTreeMap::new();
        for id in &papers {
            let Some(vector) = staged.stored_vector(id) else {
                continue;
            };
            let best = content
                .probe(&vector, 2)
                .into_iter()
                .find(|(hit, _)| hit != id);
            if let Some((_, similarity)) = best {
                nearest.insert(
                    id.clone(),
                    q32(f64::from(similarity)).map_err(|r| r.render())?.value,
                );
            }
        }
        params.push(TypedParam {
            name: nearest_neighbour_param(graph_param),
            value: TypedValue::Text(
                serde_json::to_string(&nearest).map_err(|error| invalid(error))?,
            ),
        });
    }
    request.params = BoundedVec::new(params).map_err(|detail| invalid(detail))?;
    Ok(request)
}

/// Without the query feature a graph source is refused before any read.
#[cfg(not(feature = "query"))]
pub(in crate::decide) async fn with_similarity(
    _state: &Arc<RwLock<ServerState>>,
    _store: &AgentLibraryStore,
    _verified: &VerifiedRequestContext,
    _request: DecideRequest,
) -> Result<DecideRequest, String> {
    Err(invalid(
        "a paper_admission question reads staged papers from a graph source",
    ))
}
