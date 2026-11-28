//! The `PlacementPlan` question (MEDULLA-PLACEMENT-R700.3): place a set of services
//! onto hosts within their CPU, memory and storage budgets, asked of served
//! `Decide`.
//!
//! The request carries one typed parameter, [`PlacementPlanQuestion`]. The
//! host budgets are `placement:Host` graph nodes of the named graph (node id =
//! host IRI); the server reads them under the caller's access and records them
//! in that parameter, so the pure decision and every replay read the same
//! facts. The solve is [`plan_services`] under the pinned policy's
//! `placement.node_budget` knob. Evaluate-only: nothing is moved and nothing is
//! written beyond the sealed decision record.

use medulla_decision_policy::solve::placement::{plan_services, PlanOutcome};
use medulla_decision_types::decision::knob_families::placement::plan_knobs;
use medulla_decision_types::decision::statistical::placement::{
    HeavyCapability, PLACEMENT_FACTS_INVALID,
};
use medulla_decision_types::decision::statistical::placement_plan::{
    check_plan_request, HostBudget, PlacementPlan, PlacementPlanQuestion, PlacementPlanRead,
    MAX_PLAN_HOSTS, PLACEMENT_PLAN_OPTION, PLACEMENT_PLAN_PARAM,
};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionSafety, ScoredOption, StatisticalOutcome, TypedValue,
};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::contract::BoundedVec;
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseProvenance,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    medulla_decision_statistical_types::decision::DecisionPolicy,
    medulla_decision_statistical_types::decision::QuantScaleTag,
    medulla_decision_statistical_types::decision::QuantisedValue,
};

use super::stat_support::invalid_parameter;
use super::{topology_read, SharedState};

/// The graph label of one host budget (`placement:Host`).
pub(crate) const HOST_CLASS: &str = medulla_types::ontology_iri!("placement#Host");
const CPU_MILLIS: &str = medulla_types::ontology_iri!("placement#cpuMillis");
const MEMORY_BYTES: &str = medulla_types::ontology_iri!("placement#memoryBytes");
const STORAGE_BYTES: &str = medulla_types::ontology_iri!("placement#storageBytes");
const CAPABILITIES: &str = medulla_types::ontology_iri!("placement#capabilities");

/// The typed question of a `PlacementPlan` request, or why the request is not one.
pub(super) fn question_of(request: &DecideRequest) -> Result<&PlacementPlanQuestion, String> {
    if request.question.safety != QuestionSafety::Ordinary
        || request.head.is_some()
        || !request.belief_as_of.is_empty()
        || request.params.len() != 1
    {
        return Err(invalid_parameter(
            "placement_plan needs ordinary safety, no head or belief slices, and one parameter",
        ));
    }
    let param = &request.params.as_slice()[0];
    let TypedValue::PlacementPlan(question) = &param.value else {
        return Err(invalid_parameter(
            "placement_plan requires a typed placement plan question",
        ));
    };
    if param.name != PLACEMENT_PLAN_PARAM {
        return Err(invalid_parameter(
            "the placement plan parameter must be named placement_plan",
        ));
    }
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid_parameter(
            "placement_plan requires the declared solve candidate",
        ));
    };
    let [option] = options.as_slice() else {
        return Err(invalid_parameter(
            "placement_plan declares exactly one candidate",
        ));
    };
    if option.option_id != PLACEMENT_PLAN_OPTION
        || !option.classification.is_empty()
        || !option.numbers.is_empty()
        || !option.texts.is_empty()
    {
        return Err(invalid_parameter(
            "the declared candidate must be placement_plan:solve without facts",
        ));
    }
    let hosts = question
        .read
        .as_ref()
        .map_or(&[][..], |read| read.hosts.as_slice());
    check_plan_request(hosts, &question.body.services).map_err(invalid_parameter)?;
    Ok(question)
}

/// The budget of one host node; every property is required.
fn host_of(node_id: &str, properties: &[u8]) -> Result<HostBudget, String> {
    let invalid = |what: &str| format!("{PLACEMENT_FACTS_INVALID}: {node_id}: {what}");
    let value = medulla_types::msgpack::decode_property_value(properties)
        .map_err(|_| invalid("the properties are not a valid property value"))?;
    let node = value
        .as_object()
        .ok_or_else(|| invalid("the node is not an object"))?;
    let amount = |key: &str, name: &str| -> Result<u64, String> {
        node.get(key)
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| invalid(&format!("{name} must be a non-negative integer")))
    };
    let names = node
        .get(CAPABILITIES)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid("placement:capabilities must be an array"))?;
    let mut capabilities = Vec::with_capacity(names.len());
    for name in names {
        let capability: HeavyCapability = serde_json::from_value(name.clone())
            .map_err(|_| invalid("placement:capabilities names an unknown capability"))?;
        capabilities.push(capability);
    }
    capabilities.sort();
    capabilities.dedup();
    Ok(HostBudget {
        host_iri: node_id.to_string(),
        cpu_millis: amount(CPU_MILLIS, "placement:cpuMillis")?,
        memory_bytes: amount(MEMORY_BYTES, "placement:memoryBytes")?,
        storage_bytes: amount(STORAGE_BYTES, "placement:storageBytes")?,
        capabilities,
    })
}

/// The budget of every `placement:Host` node of `graph`, read through the
/// caller's access and sorted by host IRI. More than [`MAX_PLAN_HOSTS`] hosts
/// refuse.
pub(super) async fn load_host_budgets(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
) -> Result<Vec<HostBudget>, String> {
    let core = topology_read::graph_core(state, graph, verified).await?;
    let nodes = core.get_nodes_by_label(HOST_CLASS, MAX_PLAN_HOSTS + 1);
    if nodes.len() > MAX_PLAN_HOSTS {
        return Err(format!(
            "{PLACEMENT_FACTS_INVALID}: more than {MAX_PLAN_HOSTS} hosts"
        ));
    }
    let mut hosts = Vec::with_capacity(nodes.len());
    for (node_id, properties) in nodes {
        hosts.push(host_of(&node_id, &properties)?);
    }
    hosts.sort_by(|a, b| a.host_iri.cmp(&b.host_iri));
    Ok(hosts)
}

/// Read the host budgets of the question and put them in its parameter. A
/// caller-supplied `read` is refused: observed budgets are not a caller claim.
pub(super) async fn with_read(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    mut request: DecideRequest,
) -> Result<DecideRequest, String> {
    let mut question = question_of(&request)?.clone();
    if question.read.is_some() {
        return Err(invalid_parameter(
            "the placement plan read is filled by the engine and cannot be supplied",
        ));
    }
    question.read = Some(PlacementPlanRead {
        hosts: load_host_budgets(state, &question.graph, verified).await?,
    });
    let mut params = request.params.as_slice().to_vec();
    params[0].value = TypedValue::PlacementPlan(Box::new(question));
    request.params = BoundedVec::new(params).map_err(invalid_parameter)?;
    Ok(request)
}

/// `Some` only for a `PlacementPlan` question: the outcome to seal in place of the
/// ordinary scored ladder. A solved plan is the `Advisory` score of the one
/// declared option, with the plan as the family answer; a program with no plan
/// abstains with the solver's typed reasons and seals no answer.
pub(super) fn placement_plan_outcome(
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<(StatisticalOutcome, Option<PlacementPlan>), String> {
    let question = question_of(request)?;
    let read = question.read.as_ref().ok_or_else(|| {
        invalid_parameter("the placement plan read is missing: the engine fills it before deciding")
    })?;
    match plan_services(&read.hosts, &question.body.services, &plan_knobs(policy))? {
        PlanOutcome::Planned(plan) => {
            let scores = vec![ScoredOption {
                option_id: PLACEMENT_PLAN_OPTION.to_string(),
                score: QuantisedValue {
                    scale: QuantScaleTag::Q32,
                    value: 1i64 << 32,
                },
                probability: None,
            }];
            Ok((
                StatisticalOutcome::advisory(
                    BoundedVec::new(scores).map_err(invalid_parameter)?,
                    false,
                ),
                Some(plan),
            ))
        }
        PlanOutcome::Abstained(reasons) => Ok((
            StatisticalOutcome::Abstained {
                reasons: BoundedVec::new(reasons).map_err(invalid_parameter)?,
            },
            None,
        )),
    }
}

/// The recorded reads as premises: one observation per host read.
pub(super) fn premises(request: &DecideRequest) -> Vec<PremiseRef> {
    let Ok(question) = question_of(request) else {
        return Vec::new();
    };
    let Some(read) = &question.read else {
        return Vec::new();
    };
    read.hosts
        .iter()
        .map(|host| PremiseRef {
            subject: host.host_iri.clone(),
            fact: format!(
                "placement:host cpu={} memory={} storage={} capabilities={}",
                host.cpu_millis,
                host.memory_bytes,
                host.storage_bytes,
                host.capabilities
                    .iter()
                    .map(|capability| {
                        serde_json::to_value(capability)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_string))
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            class: PremiseClass::Observation,
            provenance: PremiseProvenance::Request {
                field: "params.placement_plan.read".to_string(),
            },
            trust_tier: None,
        })
        .collect()
}
