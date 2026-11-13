//! The `Placement` question (MEDULLA-PLACEMENT-R001.7): the cheapest feasible
//! deployment target for one workload, asked of served `Decide`.
//!
//! The request carries one typed parameter, [`PlacementQuestion`]. The server
//! reads what the decision needs (the declared `placement:Target` facts of the
//! named graph and the capacity headroom of the requested cells) and records it
//! in that parameter, so the pure decision and every replay read the same
//! facts. The policy caps come from the pinned policy's `placement.*` knobs.
//! Evaluate-only: nothing is acquired and nothing is written.

use medulla_agent_types::agent_topology::{CapacityHeadroom, CapacityScope};
use medulla_decision_types::decision::knob_families::placement::policy_from_knobs;
use medulla_decision_types::decision::statistical::placement::{
    place, PlacementQuestion, PlacementRead, PlacementTarget, TargetFacts, TargetFactsEntry,
    PLACEMENT_PARAM,
};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionSafety, ScoredOption, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, PremiseClass, PremiseProvenance, PremiseRef, QuantScaleTag,
    QuantisedValue,
};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::contract::BoundedVec;

use super::stat_support::invalid_parameter;
use super::{capacity_premise, topology_read, SharedState};

/// The graph label of one deployment target (`placement:Target`).
pub(crate) const TARGET_CLASS: &str = medulla_types::ontology_iri!("placement#Target");
const SUPPORTS_HA: &str = medulla_types::ontology_iri!("placement#supportsHa");
const SUPPORTS_VOLUMES: &str = medulla_types::ontology_iri!("placement#supportsVolumes");
const MAX_REPLICAS: &str = medulla_types::ontology_iri!("placement#maxReplicas");
const RELATIVE_COST: &str = medulla_types::ontology_iri!("placement#relativeCost");

/// The refusal code of a target node whose declared facts are malformed.
pub(crate) const PLACEMENT_FACTS_INVALID: &str = "PLACEMENT_FACTS_INVALID";

/// The most target nodes one graph read returns: the registry is closed, so a
/// graph holding more has a foreign node under the label.
const MAX_TARGET_NODES: usize = 64;

/// The typed question of a `Placement` request, or why the request is not one.
pub(super) fn question_of(request: &DecideRequest) -> Result<&PlacementQuestion, String> {
    if request.question.safety != QuestionSafety::Ordinary
        || request.head.is_some()
        || !request.belief_as_of.is_empty()
        || request.params.len() != 1
    {
        return Err(invalid_parameter(
            "placement needs ordinary safety, no head or belief slices, and one parameter",
        ));
    }
    let param = &request.params.as_slice()[0];
    let TypedValue::Placement(question) = &param.value else {
        return Err(invalid_parameter(
            "placement requires a typed placement question",
        ));
    };
    if param.name != PLACEMENT_PARAM {
        return Err(invalid_parameter(
            "the placement parameter must be named placement",
        ));
    }
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid_parameter(
            "placement requires the declared target candidates",
        ));
    };
    let declared: Vec<&str> = options.iter().map(|o| o.option_id.as_str()).collect();
    let asked: Vec<&str> = question
        .body
        .candidate_targets
        .iter()
        .map(String::as_str)
        .collect();
    if declared != asked
        || options.iter().any(|option| {
            !option.classification.is_empty()
                || !option.numbers.is_empty()
                || !option.texts.is_empty()
        })
    {
        return Err(invalid_parameter(
            "the declared candidates must be the body's target ids without facts",
        ));
    }
    question.body.check()?;
    Ok(question)
}

fn property_bool(node: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<bool> {
    node.get(key)?.as_bool()
}

fn property_u64(node: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<u64> {
    node.get(key)?.as_u64()
}

/// The declared facts of one target node; every one of the four is required.
fn facts_of(node_id: &str, properties: &[u8]) -> Result<TargetFacts, String> {
    let invalid = |what: &str| format!("{PLACEMENT_FACTS_INVALID}: {node_id}: {what}");
    let value = medulla_types::msgpack::decode_property_value(properties)
        .map_err(|_| invalid("the properties are not a valid property value"))?;
    let node = value
        .as_object()
        .ok_or_else(|| invalid("the node is not an object"))?;
    let max_replicas = property_u64(node, MAX_REPLICAS)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| invalid("placement:maxReplicas must be a non-negative 32-bit integer"))?;
    Ok(TargetFacts {
        supports_ha: property_bool(node, SUPPORTS_HA)
            .ok_or_else(|| invalid("placement:supportsHa must be a boolean"))?,
        supports_volumes: property_bool(node, SUPPORTS_VOLUMES)
            .ok_or_else(|| invalid("placement:supportsVolumes must be a boolean"))?,
        max_replicas,
        relative_cost: property_u64(node, RELATIVE_COST)
            .ok_or_else(|| invalid("placement:relativeCost must be a non-negative integer"))?,
    })
}

/// The declared facts of every registered target the graph holds a
/// `placement:Target` node for. A node outside the closed registry is not a
/// candidate and is skipped; a registered node with malformed facts is refused.
pub(super) async fn load_target_facts(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
) -> Result<Vec<TargetFactsEntry>, String> {
    let core = topology_read::graph_core(state, graph, verified).await?;
    let mut entries = Vec::new();
    for (node_id, properties) in core.get_nodes_by_label(TARGET_CLASS, MAX_TARGET_NODES) {
        let Some(target) = PlacementTarget::from_option_id(&node_id) else {
            continue;
        };
        entries.push(TargetFactsEntry {
            target,
            facts: facts_of(&node_id, &properties)?,
        });
    }
    entries.sort_by_key(|entry| entry.target);
    Ok(entries)
}

/// The headroom of the cells the request names, at the request's priority.
/// No named cell reads nothing: every target then has zero headroom.
async fn read_headroom(
    state: &SharedState,
    question: &PlacementQuestion,
) -> Result<Vec<CapacityHeadroom>, String> {
    let cells = &question.body.constraints.capacity_cells;
    if cells.is_empty() {
        return Ok(Vec::new());
    }
    let scope = CapacityScope {
        cells: BoundedVec::new(cells.clone())
            .map_err(|detail| invalid_parameter(format!("capacity_cells: {detail}")))?,
        priority: question.body.constraints.priority,
    };
    capacity_premise::headroom(state, &question.graph, &scope).await
}

/// Read the target facts and capacity headroom of the question and put them in
/// its parameter. A caller-supplied `read` is refused: observed facts are not a
/// caller claim.
pub(super) async fn with_read(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    mut request: DecideRequest,
) -> Result<DecideRequest, String> {
    let mut question = question_of(&request)?.clone();
    if question.read.is_some() {
        return Err(invalid_parameter(
            "the placement read is filled by the engine and cannot be supplied",
        ));
    }
    question.read = Some(PlacementRead {
        targets: load_target_facts(state, &question.graph, verified).await?,
        headroom: read_headroom(state, &question).await?,
    });
    let mut params = request.params.as_slice().to_vec();
    params[0].value = TypedValue::Placement(Box::new(question));
    request.params = BoundedVec::new(params).map_err(invalid_parameter)?;
    Ok(request)
}

/// The Q32 score of the `rank`-th cheapest of `count` feasible targets: the
/// cheapest scores 1.0 and each later one a step lower, so the order is the
/// cost order and ties are already broken by option id.
fn rank_score(rank: usize, count: usize) -> QuantisedValue {
    let step = (1i64 << 32) / count as i64;
    QuantisedValue {
        scale: QuantScaleTag::Q32,
        value: (1i64 << 32) - step * rank as i64,
    }
}

/// `Some` only for a `Placement` question: the outcome to seal in place of the
/// ordinary scored ladder. The feasible targets are the `Advisory` scores,
/// cheapest first; none feasible abstains naming each binding constraint.
pub(super) fn placement_outcome(
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<StatisticalOutcome, String> {
    let question = question_of(request)?;
    let read = question.read.as_ref().ok_or_else(|| {
        invalid_parameter("the placement read is missing: the engine fills it before deciding")
    })?;
    let verdict = place(
        &question.body,
        &read.facts(policy_from_knobs(policy)),
        &read.headroom_by_target(),
    )?;
    if verdict.feasible.is_empty() {
        let constraints = verdict
            .eliminated
            .iter()
            .map(|(target, reason)| format!("{}: {}", target.option_id(), reason.as_why_not()))
            .collect();
        return Ok(StatisticalOutcome::Abstained {
            reasons: BoundedVec::new(vec![AbstainReason::Infeasible {
                constraints: BoundedVec::new(constraints).map_err(invalid_parameter)?,
            }])
            .map_err(invalid_parameter)?,
        });
    }
    let count = verdict.feasible.len();
    let scores = verdict
        .feasible
        .iter()
        .enumerate()
        .map(|(rank, (target, _))| ScoredOption {
            option_id: target.option_id().to_string(),
            score: rank_score(rank, count),
            probability: None,
        })
        .collect();
    Ok(StatisticalOutcome::Advisory {
        scores: BoundedVec::new(scores).map_err(invalid_parameter)?,
        calibrated: false,
    })
}

/// The recorded reads as premises: one `lease:<cell>:<class>` observation per
/// capacity cell read (nothing is acquired), and one DEFINITION per target
/// whose declared facts the decision used.
pub(super) fn premises(request: &DecideRequest) -> Vec<PremiseRef> {
    let Ok(question) = question_of(request) else {
        return Vec::new();
    };
    let Some(read) = &question.read else {
        return Vec::new();
    };
    let cells = read.headroom.iter().map(|row| PremiseRef {
        subject: row.cell_id.clone(),
        fact: format!(
            "lease:{}:{}={}",
            row.cell_id,
            serde_json::to_value(row.class)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
            row.available
        ),
        class: PremiseClass::Observation,
        provenance: PremiseProvenance::CapacityCell {
            cell_id: row.cell_id.clone(),
            epoch: row.epoch,
        },
    });
    let facts = read.targets.iter().map(|entry| PremiseRef {
        subject: entry.target.option_id().to_string(),
        fact: format!(
            "placement:facts ha={} volumes={} max_replicas={} cost={}",
            entry.facts.supports_ha,
            entry.facts.supports_volumes,
            entry.facts.max_replicas,
            entry.facts.relative_cost
        ),
        class: PremiseClass::Definition,
        provenance: PremiseProvenance::Request {
            field: "params.placement.read".to_string(),
        },
    });
    cells.chain(facts).collect()
}
