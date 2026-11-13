//! The edit concentration read of the `edit_concentration` family
//! (MEDULLA-DECISION-ENGINE-R173.3).
//!
//! The question names a graph (`graph`), a candidate `HarnessEdit` node
//! (`candidate_edit`) and a policy node (`concentration_policy`). The family
//! reads the caller's RLS-filtered snapshot of that graph, skips every edit of
//! another tenant, and answers with `concentration_verdict`. `Decide` is
//! evaluate-only: nothing is written.

use std::collections::BTreeMap;

use medulla_decision_outcome_types::decision::concentration::{
    concentration_verdict, ConcentrationAbstention, ConcentrationAction, ConcentrationPolicy,
    EditFact, EditStatus,
};
use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::codes::{MIXED_POLICY_REVISION, NO_CONCENTRATION_POLICY};
use medulla_decision_types::decision::numeric::{QuantScaleTag, QuantisedValue};
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, ScoredOption, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::AbstainReason;
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use super::stat_executor::Executed;
use super::stat_support::{filtered_view, invalid_parameter};

const PARAM_GRAPH: &str = "graph";
const PARAM_CANDIDATE: &str = "candidate_edit";
const PARAM_POLICY: &str = "concentration_policy";
/// Marker property of a `HarnessEdit` node.
const EDIT_MARKER: &str = "_harness_edit";
/// The edge a `HarnessEdit` points at its dimension with.
const TARGETS_DIMENSION: &str = "targetsDimension";

fn iri_param<'a>(request: &'a DecideRequest, name: &str) -> Result<&'a str, String> {
    request
        .params
        .iter()
        .find(|param| param.name == name)
        .and_then(|param| match &param.value {
            TypedValue::Iri(value) | TypedValue::Text(value) if !value.trim().is_empty() => {
                Some(value.as_str())
            }
            _ => None,
        })
        .ok_or_else(|| {
            invalid_parameter(format!(
                "an edit_concentration question requires a {name} param"
            ))
        })
}

/// Open the graph an `edit_concentration` question names, under the same
/// RLS-filtered snapshot a graph-sourced candidate plan gets.
pub(super) async fn concentration_view(
    state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<GraphView, String> {
    filtered_view(state, agent_id, iri_param(request, PARAM_GRAPH)?).await
}

fn unsigned(row: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<u64> {
    row.get(key).and_then(serde_json::Value::as_u64)
}

/// The dimension each edit node targets, read from its `targetsDimension` edge.
fn dimensions(view: &GraphView) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for ((source, target), blobs) in &view.edge_properties {
        let targets = blobs.iter().any(|blob| {
            rmp_serde::from_slice::<serde_json::Value>(blob)
                .ok()
                .and_then(|value| {
                    value
                        .get("relationship")
                        .and_then(serde_json::Value::as_str)
                        .map(|relationship| relationship == TARGETS_DIMENSION)
                })
                .unwrap_or(false)
        });
        if targets {
            out.insert(source.clone(), target.clone());
        }
    }
    out
}

fn status_of(row: &serde_json::Map<String, serde_json::Value>) -> Option<EditStatus> {
    match row.get("editStatus").and_then(serde_json::Value::as_str)? {
        "proposed" => Some(EditStatus::Proposed),
        "shipped" => Some(EditStatus::Shipped),
        "reverted" | "rejected" => Some(EditStatus::Reverted),
        _ => None,
    }
}

/// Whether a node belongs to the asking tenant. A node that carries no tenant
/// belongs to the graph and counts.
fn visible(row: &serde_json::Map<String, serde_json::Value>, tenant: &str) -> bool {
    row.get("tenant_id")
        .and_then(serde_json::Value::as_str)
        .is_none_or(|owner| owner == tenant)
}

fn policy_of(view: &GraphView, iri: &str, tenant: &str) -> Option<ConcentrationPolicy> {
    let row = view
        .node_row_object(iri)
        .filter(|row| visible(row, tenant))?;
    let knobs = medulla_server_api::knob_config::knobs();
    let bounded = |value: Option<u64>, default: u64| {
        u32::try_from(value.unwrap_or(default).max(1)).unwrap_or(u32::MAX)
    };
    Some(ConcentrationPolicy {
        policy_iri: iri.to_string(),
        min_shipped: bounded(
            unsigned(&row, "minShipped"),
            knobs.concentration_min_shipped,
        ),
        window_rounds: bounded(
            unsigned(&row, "windowRounds"),
            knobs.concentration_window_rounds,
        ),
        revision: unsigned(&row, "revision").unwrap_or(0),
    })
}

fn edit_of(
    id: &str,
    row: &serde_json::Map<String, serde_json::Value>,
    dimensions: &BTreeMap<String, String>,
    default_revision: u64,
) -> Option<EditFact> {
    Some(EditFact {
        edit_id: id.to_string(),
        dimension_iri: dimensions.get(id)?.clone(),
        round: u32::try_from(unsigned(row, "editRound")?).ok()?,
        status: status_of(row)?,
        policy_revision: unsigned(row, "policyRevision").unwrap_or(default_revision),
    })
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
    }
}

fn q32(value: i64) -> QuantisedValue {
    QuantisedValue {
        scale: QuantScaleTag::Q32,
        value: value << 32,
    }
}

fn scored(option_id: String, value: i64) -> ScoredOption {
    ScoredOption {
        option_id,
        score: q32(value),
        probability: None,
    }
}

fn abstained(code: medulla_types::refusal_code::RefusalCode, detail: String) -> Executed {
    executed(StatisticalOutcome::Abstained {
        reasons: BoundedVec::new(vec![AbstainReason::Family {
            family: QuestionKind::EditConcentration,
            code,
            detail,
        }])
        .expect("one reason fits the bound"),
    })
}

/// Answer one concentration question over the caller's filtered graph view.
pub(super) fn conclude(
    request: &DecideRequest,
    tenant: &str,
    graph_view: Option<&GraphView>,
) -> Result<Executed, String> {
    let view = graph_view.ok_or_else(|| {
        invalid_parameter("an edit_concentration question reads the graph its graph param names")
    })?;
    let candidate_id = iri_param(request, PARAM_CANDIDATE)?;
    let policy_iri = iri_param(request, PARAM_POLICY)?;
    let policy = policy_of(view, policy_iri, tenant);
    let default_revision = policy.as_ref().map_or(0, |policy| policy.revision);
    let dimensions = dimensions(view);
    let candidate = view
        .node_row_object(candidate_id)
        .filter(|row| visible(row, tenant))
        .and_then(|row| edit_of(candidate_id, &row, &dimensions, default_revision))
        .ok_or_else(|| {
            invalid_parameter(format!(
                "the candidate edit {candidate_id} is not a visible harness edit with a round and a dimension"
            ))
        })?;
    let facts: Vec<EditFact> = view
        .node_properties
        .keys()
        .filter(|id| id.as_str() != candidate_id)
        .filter_map(|id| {
            let row = view.node_row_object(id)?;
            (row.get(EDIT_MARKER) == Some(&serde_json::Value::Bool(true)) && visible(&row, tenant))
                .then(|| edit_of(id, &row, &dimensions, default_revision))
                .flatten()
        })
        .collect();
    Ok(
        match concentration_verdict(&facts, &candidate, policy.as_ref()) {
            Err(ConcentrationAbstention::NoConcentrationPolicy) => abstained(
                NO_CONCENTRATION_POLICY,
                format!("no concentration policy node {policy_iri} is visible"),
            ),
            Err(ConcentrationAbstention::MixedPolicyRevision { revisions }) => abstained(
                MIXED_POLICY_REVISION,
                format!("edits were judged under policy revisions {revisions:?}"),
            ),
            Ok(verdict) => {
                let (keep, revert) = match verdict.action {
                    ConcentrationAction::Keep => (1, 0),
                    ConcentrationAction::Revert => (0, 1),
                };
                let scores = vec![
                    scored("action:keep".to_string(), keep),
                    scored("action:revert".to_string(), revert),
                    scored(format!("count:{}", verdict.count), i64::from(verdict.count)),
                ];
                executed(StatisticalOutcome::Advisory {
                    scores: BoundedVec::new(scores).expect("three scores fit the bound"),
                    calibrated: false,
                })
            }
        },
    )
}
