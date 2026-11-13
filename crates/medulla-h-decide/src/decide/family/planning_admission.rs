//! The `planning_admission` question family (MEDULLA-DECISION-ENGINE-R171.3,
//! R171.4).
//!
//! A plan names features, the container each lives in and the attach point in
//! code. The family admits the plan only when the architecture view already
//! wires every feature. The view is derived from the caller's RLS-filtered
//! snapshot of the repository graph the question names (`graph`), by the same
//! `derive_architecture_view` function behind the served architecture view, and
//! compared with `compare_features`. `Decide` is evaluate-only: it writes
//! nothing, and a refusal admits nothing.
//!
//! A request the engine cannot evaluate is refused with its registered code:
//! no gap digests or no view digest (`ADMISSION_NO_COMPARISON`), or a view
//! digest that is not the digest of the view derived now
//! (`ADMISSION_STALE_VIEW`). A well-formed plan the view does not support is an
//! abstention: `ADMISSION_FEATURE_UNWIRED` names the first unwired feature and
//! `ADMISSION_GAP_UNCOVERED` the first gap no feature covers (DECISIONS
//! DE-REFUSE-VS-ABSTAIN). The `task_plan` family calls [`require_admitted`] for
//! a plan that carries a `planning_admission` param: one function, two callers.

use medulla_catalog_types::architecture_view::{ArchitectureView, ViewScope};
use medulla_core::architecture_view::{derive_architecture_view_of_snapshot, ViewRules};
use medulla_decision_plan::plan::planning_admission::{admits, compare_features};
use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::numeric::{QuantScaleTag, QuantisedValue};
use medulla_decision_types::decision::planning_admission::{
    DesignDifference, MissingKind, PlanningAdmissionRequest, ADMISSION_FEATURE_UNWIRED,
    ADMISSION_GAP_UNCOVERED, ADMISSION_NO_COMPARISON, ADMISSION_STALE_VIEW,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, ScoredOption, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, EvidenceClass, PremiseClass, PremiseProvenance, PremiseRef,
    ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::{BoundedVec, Digest256};
use medulla_types::refusal_code::RefusalCode;

use crate::decide::stat_executor::{Executed, ExecutionContext};
use crate::decide::stat_support::{filtered_view, invalid_parameter};

use super::DecisionFamily;

/// The refusal table of the family (MEDULLA-DECISION-ENGINE-R171.4), declared
/// beside the model so the uniqueness test of the code registry collects it.
pub use medulla_decision_types::decision::planning_admission::REFUSALS;

/// The param that carries the typed plan.
pub(crate) const PARAM_PLANNING_ADMISSION: &str = "planning_admission";
/// The param that names the repository graph the view is derived from.
const PARAM_GRAPH: &str = "graph";
/// The single candidate of an admission question.
const ADMIT_PLAN: &str = "admit_plan";

/// The family of the `planning_admission` question kind.
pub struct PlanningAdmissionFamily;

/// What the admission of one plan concluded.
pub(crate) enum Admission {
    /// Every feature is wired and every gap covered.
    Admitted,
    /// The plan is well-formed but not supported: the registered code and what
    /// it names.
    Held { code: RefusalCode, detail: String },
}

fn coded(code: RefusalCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

/// The typed plan of a question, when it carries one.
pub(crate) fn plan_of(request: &DecideRequest) -> Option<&PlanningAdmissionRequest> {
    request.params.iter().find_map(|param| {
        match (&param.value, param.name == PARAM_PLANNING_ADMISSION) {
            (TypedValue::PlanningAdmission(plan), true) => Some(plan.as_ref()),
            _ => None,
        }
    })
}

/// The graph param of a question.
fn graph_of(request: &DecideRequest) -> Option<&str> {
    request
        .params
        .iter()
        .find(|param| param.name == PARAM_GRAPH)
        .and_then(|param| match &param.value {
            TypedValue::Iri(value) | TypedValue::Text(value) if !value.trim().is_empty() => {
                Some(value.as_str())
            }
            _ => None,
        })
}

/// Whether the question reads the repository graph for an admission: a
/// `planning_admission` question, or a `task_plan` question that carries a plan
/// to admit.
pub(crate) fn reads_repository_graph(request: &DecideRequest) -> bool {
    matches!(
        request.question.kind,
        QuestionKind::PlanningAdmission | QuestionKind::TaskPlan
    ) && plan_of(request).is_some()
}

/// Open the repository graph a question names, under the same RLS-filtered
/// snapshot a graph-sourced candidate plan gets.
pub(crate) async fn planning_view(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<GraphView, String> {
    let graph = graph_of(request).ok_or_else(|| {
        invalid_parameter("a plan admission requires a graph param naming the repository graph")
    })?;
    filtered_view(state, agent_id, graph).await
}

/// The digest of a view: SHA-256 of its canonical JSON. The containers and
/// edges are sorted by the derivation and the field order is fixed, so equal
/// views have equal digests.
pub(crate) fn view_digest(view: &ArchitectureView) -> Result<Digest256, String> {
    serde_json::to_vec(view)
        .map(|bytes| Digest256::sha256(&bytes))
        .map_err(|error| invalid_parameter(format!("the view has no canonical form: {error}")))
}

/// The request-shape checks that need no read: a comparison needs gap digests
/// and a view digest (`ADMISSION_NO_COMPARISON`).
fn check_comparable(plan: &PlanningAdmissionRequest) -> Result<(), String> {
    if plan.gap_proposal_digests.is_empty()
        || plan.architecture_view_digest == Digest256::from_bytes([0u8; 32])
    {
        return Err(coded(
            ADMISSION_NO_COMPARISON,
            "the plan carries no gap digests or no architecture view digest",
        ));
    }
    Ok(())
}

fn describe(differences: &[DesignDifference]) -> String {
    differences
        .iter()
        .map(|difference| {
            let missing = match difference.missing {
                MissingKind::AttachPoint => "attach_point",
                MissingKind::Edge => "edge",
            };
            format!(
                "{} ({}): {missing}",
                difference.feature_id, difference.container
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Admit one plan over the caller's graph view. Derives the view, checks its
/// digest, compares the features and checks that every gap is covered. Reads
/// nothing but `graph_view`; writes nothing.
pub(crate) fn admit_plan(
    plan: &PlanningAdmissionRequest,
    tenant: &str,
    graph_view: Option<&GraphView>,
) -> Result<Admission, String> {
    check_comparable(plan)?;
    let snapshot = graph_view.ok_or_else(|| {
        invalid_parameter("a plan admission reads the repository graph its graph param names")
    })?;
    let scope = ViewScope {
        repository_id: plan.repository_id.clone(),
        revision: Some(plan.repository_revision.clone()),
    };
    // A node that carries another tenant's id is not part of this caller's
    // view; the repository then reads as unknown, never as forbidden.
    let own = |row: &serde_json::Map<String, serde_json::Value>| {
        row.get("tenant_id")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|owner| owner == tenant)
    };
    let view = derive_architecture_view_of_snapshot(snapshot, &scope, &ViewRules::default(), &own)
        .map_err(|abstained| format!("{}: {abstained}", abstained.code()))?;
    let derived = view_digest(&view)?;
    if derived != plan.architecture_view_digest {
        return Err(coded(
            ADMISSION_STALE_VIEW,
            format!(
                "the plan was made against view {} but the repository now derives {}",
                plan.architecture_view_digest.to_hex(),
                derived.to_hex()
            ),
        ));
    }
    let differences = compare_features(&view, plan);
    if !admits(&differences) {
        return Ok(Admission::Held {
            code: ADMISSION_FEATURE_UNWIRED,
            detail: describe(&differences),
        });
    }
    if let Some(gap) = plan.gap_proposal_digests.iter().find(|gap| {
        !plan
            .features
            .iter()
            .any(|feature| feature.gap_digests.iter().any(|covered| covered == *gap))
    }) {
        return Ok(Admission::Held {
            code: ADMISSION_GAP_UNCOVERED,
            detail: format!("gap {} appears in no feature", gap.to_hex()),
        });
    }
    Ok(Admission::Admitted)
}

/// The gate a plan-producing question passes before it returns: refuses
/// unless the plan its `planning_admission` param carries is admitted. A
/// question with no such param is not gated.
pub(crate) fn require_admitted(
    request: &DecideRequest,
    tenant: &str,
    graph_view: Option<&GraphView>,
) -> Result<(), String> {
    let Some(plan) = plan_of(request) else {
        return Ok(());
    };
    match admit_plan(plan, tenant, graph_view)? {
        Admission::Admitted => Ok(()),
        Admission::Held { code, detail } => Err(coded(code, detail)),
    }
}

fn executed(outcome: StatisticalOutcome) -> Executed {
    Executed {
        matrix: None,
        ladder: LadderResult {
            outcome,
            calibration: None,
            explained: None,
            audit: None,
            logging: Vec::new(),
        },
        reading: None,
        seed: [0u8; 32],
        harness_contrast: None,
        topology: None,
        family: None,
    }
}

impl DecisionFamily for PlanningAdmissionFamily {
    /// The single candidate is `admit_plan`.
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        match options {
            [only] if only.option_id == ADMIT_PLAN => Ok(()),
            _ => Err(invalid_parameter(format!(
                "a planning_admission question declares exactly one option, {ADMIT_PLAN}"
            ))),
        }
    }

    /// The request carries a plan that can be compared and names its graph;
    /// checked before any store read.
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        let plan = plan_of(request).ok_or_else(|| {
            invalid_parameter(format!(
                "a planning_admission question requires a {PARAM_PLANNING_ADMISSION} param"
            ))
        })?;
        graph_of(request).ok_or_else(|| {
            invalid_parameter("a planning_admission question requires a graph param")
        })?;
        check_comparable(plan)
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        let plan = plan_of(request)?;
        Some(
            admit_plan(plan, ctx.tenant_id, graph_view).map(|admission| match admission {
                Admission::Admitted => executed(StatisticalOutcome::Advisory {
                    scores: BoundedVec::new(vec![ScoredOption {
                        option_id: ADMIT_PLAN.to_string(),
                        score: QuantisedValue {
                            scale: QuantScaleTag::Q32,
                            value: 1i64 << 32,
                        },
                        probability: None,
                    }])
                    .expect("one score fits the bound"),
                    calibrated: false,
                }),
                Admission::Held { code, detail } => executed(StatisticalOutcome::Abstained {
                    reasons: BoundedVec::new(vec![AbstainReason::Family {
                        family: QuestionKind::PlanningAdmission,
                        code,
                        detail,
                    }])
                    .expect("one reason fits the bound"),
                }),
            }),
        )
    }

    /// The view the plan was compared against, as an `Observation`: the view
    /// derives from ingested code.
    fn premises(
        &self,
        request: &DecideRequest,
        _graph_view: Option<&GraphView>,
    ) -> Vec<PremiseRef> {
        let Some(plan) = plan_of(request) else {
            return Vec::new();
        };
        vec![PremiseRef {
            subject: plan.repository_id.clone(),
            fact: format!(
                "admission:view repository={} revision={} digest={}",
                plan.repository_id,
                plan.repository_revision,
                plan.architecture_view_digest.to_hex()
            ),
            class: PremiseClass::Observation,
            provenance: PremiseProvenance::Observation {
                evaluation_id: plan.repository_revision.clone(),
            },
        }]
    }

    /// The view derives from ingested code, so an admitted plan rests on an
    /// `Observation`, though the caller declared the option (R171.3).
    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        match outcome {
            StatisticalOutcome::Abstained { .. } => None,
            _ => Some(EvidenceClass::Observation),
        }
    }

    /// An admitted plan is a constraint; a held one keeps its abstention.
    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        match outcome {
            StatisticalOutcome::Abstained { .. } => None,
            _ => Some(ResolutionKind::Constraint),
        }
    }
}
