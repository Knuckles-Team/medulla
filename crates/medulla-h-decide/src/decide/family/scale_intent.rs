//! The `scale_intent` question family (MEDULLA-DECISION-ENGINE-R833).
//!
//! The verdict on one replica-count intent against the `Scale*` authority nodes
//! stored in the graph the question names, read under the caller's access and
//! row-level filter. The unit the intent targets is the `ScaleUnit` node whose
//! `unit_id` is the intent's `target.workload_id`; its pool, failure domain and
//! controller registration are the nodes that unit names. The answer is
//! `allow` or `refuse:<CODE>`: a refused intent is a decided verdict, sealed in
//! the record. Advisory; `Decide` writes nothing.

use serde_json::{Map, Value};

use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::scale::{
    judge, ScaleIntentRecord, ScaleObservation, ScalePoolFacts, ScaleUnitFacts, PARAM_GRAPH,
    PARAM_INTENT, VERDICT_OPTION,
};
use medulla_decision_types::decision::statistical::{DecideRequest, StatisticalOutcome};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;
use medulla_types::evidence_class::EvidenceClass;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::SharedState;

use super::support::{advisory, invalid, json_param, single_option, text_param};
use super::DecisionFamily;

/// The family of the `scale_intent` question kind.
pub struct ScaleIntentFamily;

type Row = Map<String, Value>;

/// The graph the authority nodes are read from, under the caller's access.
pub(in crate::decide) async fn evidence_view(
    state: &SharedState,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<Option<GraphView>, String> {
    match text_param(request, PARAM_GRAPH)? {
        Some(graph) => crate::decide::stat_support::filtered_view(state, agent_id, graph)
            .await
            .map(Some),
        None => Err(invalid(format!(
            "a scale_intent question requires the {PARAM_GRAPH} parameter"
        ))),
    }
}

fn text(row: &Row, key: &str) -> Option<String> {
    match row.get(key)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn number(row: &Row, key: &str) -> Option<u64> {
    match row.get(key)? {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// A nested record stored either as an object or as its JSON text.
fn object(row: &Row, key: &str) -> Option<Row> {
    match row.get(key)? {
        Value::Object(map) => Some(map.clone()),
        Value::String(text) => serde_json::from_str::<Row>(text).ok(),
        _ => None,
    }
}

/// The rows of `view` whose `type` is `label`.
fn rows_of(view: &GraphView, label: &str) -> Vec<Row> {
    view.node_properties
        .keys()
        .filter_map(|id| view.node_row_object(id))
        .filter(|row| text(row, "type").as_deref() == Some(label))
        .collect()
}

/// The one row of `label` whose `key` property is `value`.
fn row_with(rows: &[Row], key: &str, value: &str) -> Option<Row> {
    let mut found = rows
        .iter()
        .filter(|row| text(row, key).as_deref() == Some(value));
    let first = found.next()?;
    found.next().is_none().then(|| first.clone())
}

fn unit_facts(row: &Row) -> Option<ScaleUnitFacts> {
    Some(ScaleUnitFacts {
        revision: number(row, "revision")?,
        min_replicas: number(row, "min_replicas")?,
        max_replicas: number(row, "max_replicas")?,
        controller_mode: text(row, "controller_mode")?,
        replica_writer_id: text(row, "replica_writer_id")?,
    })
}

/// What the stored authority nodes report for the unit `unit_id`.
fn observe(view: &GraphView, unit_id: &str) -> ScaleObservation {
    let unit_row = row_with(&rows_of(view, "ScaleUnit"), "unit_id", unit_id);
    let unit = unit_row.as_ref().and_then(unit_facts);
    let pool = unit_row
        .as_ref()
        .and_then(|row| text(row, "resource_pool_id"))
        .and_then(|id| row_with(&rows_of(view, "ScaleResourcePool"), "pool_id", &id))
        .and_then(|row| {
            Some(ScalePoolFacts {
                capacity_units: number(&row, "capacity_units")?,
                reserved_headroom_units: number(&row, "reserved_headroom_units").unwrap_or(0),
            })
        });
    let failure_domain_status = unit_row
        .as_ref()
        .and_then(|row| text(row, "failure_domain_id"))
        .and_then(|id| row_with(&rows_of(view, "ScaleFailureDomain"), "domain_id", &id))
        .and_then(|row| text(&row, "status"));
    let registration_fence = row_with(
        &rows_of(view, "ScaleControllerRegistration"),
        "unit_id",
        unit_id,
    )
    .and_then(|row| object(&row, "lease_fence"))
    .and_then(|fence| {
        Some((
            number(&fence, "lease_epoch")?,
            number(&fence, "fence_token")?,
        ))
    });
    ScaleObservation {
        authority_revisions: rows_of(view, "ScaleAuthorityRevision").len(),
        unit,
        pool,
        failure_domain_status,
        registration_fence,
    }
}

fn conclude_verdict(view: Option<&GraphView>, request: &DecideRequest) -> Result<Executed, String> {
    let intent: ScaleIntentRecord = json_param(request, PARAM_INTENT)?;
    let view = view.ok_or_else(|| invalid("the scale authority graph was not read"))?;
    let unit_id = intent.target.workload_id.clone();
    let observation = observe(view, &unit_id);
    let verdict = judge(&intent, &observation);
    let mut executed = advisory(vec![verdict.option_id()])?;
    executed.family_premises = vec![FamilyPremise {
        subject: "scale".into(),
        fact: format!(
            "unit:{unit_id}:revision={}:authority_revisions={}",
            observation.unit.as_ref().map_or(0, |unit| unit.revision),
            observation.authority_revisions
        ),
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Observation(unit_id),
    }];
    Ok(executed)
}

impl DecisionFamily for ScaleIntentFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, VERDICT_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        json_param::<ScaleIntentRecord>(request, PARAM_INTENT).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_verdict(graph_view, request))
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
        // The authority is stored graph content, so the record is an observation.
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Observation)
    }
}
