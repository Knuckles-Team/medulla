//! What a task's `:hasStep` objects add to an assembly: the steps of the
//! request's tasks, read from the request graph and ordered by the one step
//! ordering routine (MEDULLA-ORCH-VOCAB-R007.1). The steps are recorded in the
//! decision inputs, so the pure decision function and every replay of the
//! record read the same steps.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_stats::decision::usage_guidance::{
    order_steps, OrderError, PlanStep, UsageKnobs,
};
use medulla_decision_types::decision::record::AssemblyStep;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::contract::BoundedVec;

use {
    medulla_h_decide_base::decide::stat_usage_guidance::facts_of_triples,
    medulla_h_decide_base::decide::stat_usage_guidance::Fact,
    medulla_h_decide_base::decide::stat_usage_guidance::Object,
};
use {medulla_h_decide_base::decide::topology_read, medulla_h_decide_base::decide::SharedState};

const CORE: &str = medulla_types::ontology_iri!("core#");

/// The refusal code of a `:precedes` cycle between steps.
const STEP_ORDER_CYCLE: &str = "STEP_ORDER_CYCLE";

fn invalid(what: impl std::fmt::Display) -> String {
    format!("ASSEMBLY_INPUTS_INVALID: steps: {what}")
}

/// The IRI objects of `subject` under the core predicate `name`.
fn iri_objects(facts: &[Fact], subject: &str, name: &str) -> BTreeSet<String> {
    facts
        .iter()
        .filter(|fact| fact.subject == subject && fact.predicate.strip_prefix(CORE) == Some(name))
        .filter_map(|fact| match &fact.object {
            Object::Iri(iri) => Some(iri.clone()),
            Object::Literal(_) => None,
        })
        .collect()
}

/// The steps of `tasks` in step order, with their capability IRIs. A task with
/// no step adds none. The read is one triple export of the request graph,
/// behind the ordinary read ACL; a request with no tasks reads nothing.
pub(super) async fn read(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    tasks: &[String],
) -> Result<BoundedVec<AssemblyStep, 64>, String> {
    if tasks.is_empty() {
        return Ok(BoundedVec::default());
    }
    let core = topology_read::graph_core(state, graph, verified).await?;
    let graph = graph.to_string();
    let tasks = tasks.to_vec();
    let steps = tokio::task::spawn_blocking(move || {
        let triples = medulla_rdf::mapping::export_triples(&core, &graph)?;
        steps_of(&facts_of_triples(&triples), &tasks)
    })
    .await
    .map_err(|error| format!("assembly step read task failed: {error}"))??;
    BoundedVec::new(steps).map_err(invalid)
}

/// The ordered steps `facts` state for `tasks`.
fn steps_of(facts: &[Fact], tasks: &[String]) -> Result<Vec<AssemblyStep>, String> {
    let mut plan: Vec<PlanStep> = Vec::new();
    let mut owners: Vec<&str> = Vec::new();
    let mut capabilities: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for task in tasks {
        for step in iri_objects(facts, task, "hasStep") {
            plan.push(PlanStep {
                step_id: step.clone(),
                tool: String::new(),
                precedes: iri_objects(facts, &step, "precedes").into_iter().collect(),
            });
            owners.push(task);
            capabilities
                .entry(step.clone())
                .or_default()
                .extend(iri_objects(facts, &step, "requiresCapability"));
        }
    }
    if plan.is_empty() {
        return Ok(Vec::new());
    }
    let order =
        order_steps(&plan, &BTreeMap::new(), &UsageKnobs { min_trials: 0 }).map_err(|error| {
            match error {
                OrderError::Cyclic(ids) => format!("{STEP_ORDER_CYCLE}: {}", ids.join(", ")),
                OrderError::DuplicateStep(id) => invalid(format!("step {id} is declared twice")),
            }
        })?;
    let task_of: BTreeMap<&str, &str> = plan
        .iter()
        .map(|step| step.step_id.as_str())
        .zip(owners.iter().copied())
        .collect();
    order
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let held: Vec<String> = capabilities
                .get(step)
                .map(|set| set.iter().cloned().collect())
                .unwrap_or_default();
            Ok(AssemblyStep {
                task: task_of
                    .get(step.as_str())
                    .copied()
                    .unwrap_or_default()
                    .to_string(),
                step: step.clone(),
                position: u16::try_from(index).map_err(invalid)?,
                capabilities: BoundedVec::new(held).map_err(invalid)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(subject: &str, name: &str, object: &str) -> Fact {
        Fact {
            subject: subject.to_string(),
            predicate: format!("{CORE}{name}"),
            object: Object::Iri(object.to_string()),
        }
    }

    #[test]
    fn steps_follow_precedes_and_carry_their_capabilities() {
        let facts = vec![
            fact("t", "hasStep", "s2"),
            fact("t", "hasStep", "s1"),
            fact("s1", "precedes", "s2"),
            fact("s1", "requiresCapability", "eg:capability/retrieval"),
        ];
        let steps = steps_of(&facts, &["t".to_string()]).unwrap();
        let ids: Vec<(&str, u16)> = steps
            .iter()
            .map(|s| (s.step.as_str(), s.position))
            .collect();
        assert_eq!(ids, vec![("s1", 0), ("s2", 1)]);
        assert_eq!(
            steps[0].capabilities.as_slice(),
            ["eg:capability/retrieval"]
        );
        assert!(steps_of(&facts, &["other".to_string()]).unwrap().is_empty());
    }

    #[test]
    fn a_cycle_refuses_by_code() {
        let facts = vec![
            fact("t", "hasStep", "a"),
            fact("t", "hasStep", "b"),
            fact("a", "precedes", "b"),
            fact("b", "precedes", "a"),
        ];
        let error = steps_of(&facts, &["t".to_string()]).unwrap_err();
        assert_eq!(error, "STEP_ORDER_CYCLE: a, b");
    }
}
