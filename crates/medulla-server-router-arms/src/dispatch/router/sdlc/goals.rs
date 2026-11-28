//! The served agent goal methods (MEDULLA-DECISION-ENGINE-R901): `GoalList`, `GoalIterations` and
//! `GoalControl`.
//!
//! Each arm reads only the nodes the caller can see in the request graph, decides in the pure
//! functions of `medulla_source_types::agent_goal` and, for the one write, commits the goal node
//! and the opened or closed iteration node in one gateway-routed batch. A refusal and a replay
//! return before the batch is built, so they write nothing. A goal of another tenant, an invisible
//! goal and an absent goal all refuse alike.

use super::deploy::{verified_tenant, visible_nodes_of};
use super::*;

use medulla_protocol_types::result_contract::ingestion::{
    GoalControl as GoalControlResult, GoalIterations as GoalIterationsResult,
    GoalList as GoalListResult,
};
use medulla_source_types::agent_goal::{
    goal_node_id, handle_goal_control, handle_goal_iterations, handle_goal_list, iteration_node_id,
    validate_goal_id, AgentGoal, GoalControlOp, GoalIteration, StoredGoal, AGENT_GOAL_TYPE,
    GOAL_ITERATIONS_PROPERTY, GOAL_ITERATION_TYPE,
};
use medulla_types::transition_refused::MachineState;

/// Most goal nodes one list reads.
const MAX_GOALS_READ: usize = 100_000;

/// The stored goal of `goal_id` in the verified tenant that the caller can see. A malformed id
/// holds no goal.
async fn stored_goal(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    tenant: &str,
    goal_id: &str,
) -> Result<Option<StoredGoal>, String> {
    if validate_goal_id(goal_id).is_err() {
        return Ok(None);
    }
    Ok(visible_node(state, target, &goal_node_id(tenant, goal_id))
        .await?
        .and_then(|node| StoredGoal::from_node(&node, tenant)))
}

/// The iteration `number` of `goal_id` in the verified tenant that the caller can see.
async fn stored_iteration(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    tenant: &str,
    goal_id: &str,
    number: u32,
) -> Result<Option<GoalIteration>, String> {
    Ok(
        visible_node(state, target, &iteration_node_id(tenant, goal_id, number))
            .await?
            .and_then(|node| GoalIteration::from_node(&node, tenant)),
    )
}

fn goal_properties(
    goal: &AgentGoal,
    iterations: u32,
    target: &SdlcTarget<'_>,
) -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert("type".into(), AGENT_GOAL_TYPE.into());
    properties.insert("goal_id".into(), goal.goal_id.clone().into());
    properties.insert("title".into(), goal.title.clone().into());
    properties.insert("state".into(), goal.state.stored().into());
    if let Some(run_id) = &goal.run_id {
        properties.insert("run_id".into(), run_id.clone().into());
    }
    properties.insert("created_ms".into(), goal.created_ms.into());
    properties.insert(GOAL_ITERATIONS_PROPERTY.into(), iterations.into());
    stamp(&mut properties, target);
    properties
}

fn iteration_properties(iteration: &GoalIteration, target: &SdlcTarget<'_>) -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert("type".into(), GOAL_ITERATION_TYPE.into());
    properties.insert("goal_id".into(), iteration.goal_id.clone().into());
    properties.insert("iteration".into(), iteration.iteration.into());
    properties.insert("run_id".into(), iteration.run_id.clone().into());
    properties.insert("outcome".into(), iteration.outcome.as_str().into());
    properties.insert("started_ms".into(), iteration.started_ms.into());
    if let Some(ended_ms) = iteration.ended_ms {
        properties.insert("ended_ms".into(), ended_ms.into());
    }
    stamp(&mut properties, target);
    properties
}

/// `GoalList`: one page of the tenant's goals the caller can see.
pub(super) async fn list(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    limit: Option<u32>,
    cursor: Option<String>,
) -> Response {
    let result = async {
        let tenant = verified_tenant(&target)?;
        let goals: Vec<AgentGoal> =
            visible_nodes_of(state, &target, AGENT_GOAL_TYPE, MAX_GOALS_READ)
                .await?
                .iter()
                .filter_map(|(_, node)| StoredGoal::from_node(node, tenant))
                .map(|stored| stored.goal)
                .collect();
        handle_goal_list(goals, tenant, limit, cursor.as_deref()).map_err(|e| e.to_string())
    }
    .await;
    typed_response::<GoalListResult>(target.req_id, result)
}

/// `GoalIterations`: the iterations of one goal. An absent, foreign, invisible or malformed goal
/// id refuses alike.
pub(super) async fn iterations(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    goal_id: String,
) -> Response {
    let result = async {
        let tenant = verified_tenant(&target)?;
        let stored = stored_goal(state, &target, tenant, &goal_id).await?;
        let mut read = Vec::new();
        if let Some(held) = &stored {
            for number in 1..=held.iterations {
                if let Some(iteration) =
                    stored_iteration(state, &target, tenant, &goal_id, number).await?
                {
                    read.push(iteration);
                }
            }
        }
        handle_goal_iterations(stored, read).map_err(|e| e.to_string())
    }
    .await;
    typed_response::<GoalIterationsResult>(target.req_id, result)
}

/// `GoalControl`: decide against the stored goal and its open iteration, then write the goal node
/// and the iteration node in one batch. Every time is the engine clock.
pub(super) async fn control(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    op: GoalControlOp,
) -> Response {
    let result = async {
        let tenant = verified_tenant(&target)?;
        validate_goal_id(op.goal_id()).map_err(|e| e.to_string())?;
        let goal_id = op.goal_id().to_owned();
        let stored = stored_goal(state, &target, tenant, &goal_id).await?;
        let open = match &stored {
            Some(held) if held.iterations > 0 => {
                stored_iteration(state, &target, tenant, &goal_id, held.iterations).await?
            }
            _ => None,
        };
        let write = handle_goal_control(
            tenant,
            stored,
            open,
            op,
            medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
        )
        .map_err(|e| e.to_string())?;
        if !write.replayed {
            let mut operations = vec![json!({
                "op": "upsert_node",
                "id": goal_node_id(tenant, &write.goal.goal_id),
                "properties": goal_properties(&write.goal, write.iterations, &target),
            })];
            if let Some(iteration) = &write.iteration {
                operations.push(json!({
                    "op": "upsert_node",
                    "id": iteration_node_id(tenant, &iteration.goal_id, iteration.iteration),
                    "properties": iteration_properties(iteration, &target),
                }));
            }
            commit(state, &target, bounded(operations)?).await?;
        }
        Ok(write.receipt())
    }
    .await;
    typed_response::<GoalControlResult>(target.req_id, result)
}
