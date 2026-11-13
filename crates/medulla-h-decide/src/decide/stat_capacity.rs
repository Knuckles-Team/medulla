//! The engine-side read of a `capacity_proposal` question
//! (MEDULLA-DECISION-ENGINE-R824).
//!
//! The headroom and epoch of the proposal's cell are read once from the
//! capacity ledger, through `capacity_premise::headroom`, before the family
//! concludes, and recorded in the question's `observation` parameter. They are
//! observed facts, not a caller claim: a request that already carries them is
//! refused. The cell is the proposal's `resource_class_iri`, read at
//! `Orchestration` priority.

use medulla_agent_types::agent_topology::CapacityScope;
use medulla_decision_types::decision::statistical::capacity::{
    CapacityAdjustmentProposal, Observation, PARAM_GRAPH, PARAM_OBSERVATION, PARAM_PROPOSAL,
};
use medulla_decision_types::decision::statistical::{DecideRequest, TypedParam, TypedValue};
use medulla_mutation_types::capacity_lease::LeasePriority;
use medulla_types::contract::BoundedVec;

use super::stat_support::invalid_parameter;
use super::{capacity_premise, SharedState};

fn text<'a>(request: &'a DecideRequest, name: &str) -> Result<&'a str, String> {
    request
        .params
        .iter()
        .find(|p| p.name == name)
        .and_then(|p| match &p.value {
            TypedValue::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| invalid_parameter(format!("a capacity_proposal question requires {name}")))
}

/// Read the cell's headroom and put it in the question's `observation`.
pub(super) async fn with_read(
    state: &SharedState,
    mut request: DecideRequest,
) -> Result<DecideRequest, String> {
    if request.params.iter().any(|p| p.name == PARAM_OBSERVATION) {
        return Err(invalid_parameter(
            "the capacity observation is filled by the engine and cannot be supplied",
        ));
    }
    let proposal: CapacityAdjustmentProposal =
        serde_json::from_str(text(&request, PARAM_PROPOSAL)?)
            .map_err(|e| invalid_parameter(format!("parameter {PARAM_PROPOSAL}: {e}")))?;
    let scope = CapacityScope {
        cells: BoundedVec::new(vec![proposal.resource_class_iri.clone()])
            .map_err(invalid_parameter)?,
        priority: LeasePriority::Orchestration,
    };
    let graph = text(&request, PARAM_GRAPH)?.to_string();
    let headroom = capacity_premise::headroom(state, &graph, &scope).await?;
    let cell = headroom
        .iter()
        .find(|h| h.cell_id == proposal.resource_class_iri)
        .ok_or_else(|| {
            invalid_parameter(format!(
                "the capacity ledger has no cell `{}`",
                proposal.resource_class_iri
            ))
        })?;
    let unit = serde_json::to_value(cell.class)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .ok_or_else(|| invalid_parameter("the cell's resource class has no wire name"))?;
    let observation = Observation {
        unit,
        available: cell.available,
        epoch: cell.epoch,
    };
    let mut params = request.params.as_slice().to_vec();
    params.push(TypedParam {
        name: PARAM_OBSERVATION.to_string(),
        value: TypedValue::Text(serde_json::to_string(&observation).map_err(invalid_parameter)?),
    });
    params.sort_by(|a, b| a.name.cmp(&b.name));
    request.params = BoundedVec::new(params).map_err(invalid_parameter)?;
    Ok(request)
}
