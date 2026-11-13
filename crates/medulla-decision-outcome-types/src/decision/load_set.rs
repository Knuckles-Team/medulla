//! Job-scoped load set question and answer (MEDULLA-DECISION-ENGINE-R176.1).
//!
//! The wire types live in `medulla-decision-types` (the typed decision value
//! and family answer carry them); this module holds the merge.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::decision::capability_designation::EntityKind;
use medulla_decision_types::decision::load_set::{AgentStart, LoadSetQuestion};

pub use medulla_decision_types::decision::load_set_knobs::LoadSetKnobs;

/// One designated component as the merge sees it: its score, the capabilities
/// it holds and the connector pack it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadComponent {
    pub id: String,
    pub kind: EntityKind,
    pub score_q32: i64,
    pub capabilities: BTreeSet<String>,
    /// The connector of the pack the component was projected from; empty for
    /// a component that came from no pack.
    pub pack: String,
}

/// A local agent read from the graph: what it uses and holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCandidate {
    pub agent_iri: String,
    pub prompt_iri: String,
    pub model_profile: String,
    /// `:orchestrationUsesTool` objects.
    pub tools: BTreeSet<String>,
    /// `:hasSkill` objects.
    pub skills: BTreeSet<String>,
}

/// The merged load set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadSetMerge {
    pub tools: Vec<LoadComponent>,
    pub skills: Vec<String>,
    pub agents: Vec<AgentStart>,
    /// Tool candidates cut by `max_tools`.
    pub excluded: usize,
}

fn by_score(a: &LoadComponent, b: &LoadComponent) -> std::cmp::Ordering {
    b.score_q32.cmp(&a.score_q32).then_with(|| a.id.cmp(&b.id))
}

/// Merge capability designations into one bounded, ordered load set
/// (MEDULLA-DECISION-ENGINE-R176.2). `per_capability` holds one designation
/// list per required capability, in the question's order; `overall` is the
/// designation by job embedding. Pure: the result never depends on the order
/// of the inputs inside a list.
pub fn merge_load_set(
    per_capability: &[Vec<LoadComponent>],
    overall: &[LoadComponent],
    agents: &[AgentCandidate],
    question: &LoadSetQuestion,
    knobs: &LoadSetKnobs,
) -> LoadSetMerge {
    // Maximum score per component id; capabilities union.
    let mut pool: BTreeMap<String, LoadComponent> = BTreeMap::new();
    for component in per_capability.iter().flatten().chain(overall) {
        pool.entry(component.id.clone())
            .and_modify(|held| {
                held.score_q32 = held.score_q32.max(component.score_q32);
                held.capabilities
                    .extend(component.capabilities.iter().cloned());
            })
            .or_insert_with(|| component.clone());
    }
    let max_tools = usize::from(question.max_tools);
    // The best tool of each required capability comes first.
    let mut chosen: BTreeSet<String> = BTreeSet::new();
    for list in per_capability {
        if chosen.len() >= max_tools {
            break;
        }
        let best = list
            .iter()
            .filter(|c| c.kind == EntityKind::Tool)
            .filter_map(|c| pool.get(&c.id))
            .min_by(|a, b| by_score(a, b));
        if let Some(best) = best {
            chosen.insert(best.id.clone());
        }
    }
    let mut rest: Vec<&LoadComponent> = pool
        .values()
        .filter(|c| c.kind == EntityKind::Tool && !chosen.contains(&c.id))
        .collect();
    rest.sort_by(|a, b| by_score(a, b));
    let tool_candidates = rest.len() + chosen.len();
    for component in rest {
        if chosen.len() >= max_tools {
            break;
        }
        chosen.insert(component.id.clone());
    }
    let mut tools: Vec<LoadComponent> = chosen
        .iter()
        .filter_map(|id| pool.get(id).cloned())
        .collect();
    tools.sort_by(by_score);
    let excluded = tool_candidates - tools.len();
    // Agents that use a chosen tool: most chosen tools held, then IRI.
    let mut qualified: Vec<(usize, &AgentCandidate)> = agents
        .iter()
        .filter_map(|agent| {
            let held = agent.tools.intersection(&chosen).count();
            (held > 0).then_some((held, agent))
        })
        .collect();
    qualified.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.agent_iri.cmp(&b.1.agent_iri))
    });
    qualified.truncate(usize::from(question.max_agents));
    // Skills of the same pack as a chosen tool with a shared capability, then
    // the skills the chosen agents hold.
    let mut skills: Vec<&LoadComponent> = pool
        .values()
        .filter(|skill| {
            skill.kind == EntityKind::Skill
                && tools.iter().any(|tool| {
                    tool.pack == skill.pack
                        && tool
                            .capabilities
                            .intersection(&skill.capabilities)
                            .next()
                            .is_some()
                })
        })
        .collect();
    skills.sort_by(|a, b| by_score(a, b));
    let mut skill_ids: Vec<String> = skills.iter().map(|skill| skill.id.clone()).collect();
    let agent_skills: BTreeSet<&String> = qualified
        .iter()
        .flat_map(|(_, agent)| agent.skills.iter())
        .collect();
    for skill in agent_skills {
        if !skill_ids.contains(skill) {
            skill_ids.push(skill.clone());
        }
    }
    skill_ids.truncate(knobs.max_skills);
    LoadSetMerge {
        tools,
        skills: skill_ids,
        agents: qualified
            .into_iter()
            .map(|(_, agent)| AgentStart {
                agent_iri: agent.agent_iri.clone(),
                prompt_iri: agent.prompt_iri.clone(),
                model_profile: agent.model_profile.clone(),
            })
            .collect(),
        excluded,
    }
}

#[cfg(test)]
mod tests;
