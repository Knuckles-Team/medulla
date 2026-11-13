//! The ranked skill tree of one task (MEDULLA-DECISION-ENGINE-R134.9).
//!
//! Medulla ranks the visible skills of each subtask by the relevance of
//! R134.4, moved by the outcome rate of earlier decisions that ran in the same
//! context, and links each kept skill to its nearest kept ancestor. The result
//! is sealed on the plan; a caller applies it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use medulla_types::contract::BoundedVec;

use crate::decision::knob_families::task_plan::{
    SKILL_TREE_MAX_NODES, SKILL_TREE_MIN_OUTCOMES, SKILL_TREE_OUTCOME_WEIGHT,
};
use crate::decision::knobs::KnobValue;
use crate::decision::policy::DecisionPolicy;

/// A skill's `parent` chain returns to a skill already on the chain.
pub const SKILL_TREE_CYCLE: &str = "SKILL_TREE_CYCLE";
/// A skill's `parent` attribute names a skill the tenant does not see.
pub const SKILL_PARENT_UNKNOWN: &str = "SKILL_PARENT_UNKNOWN";
/// An outcome reference no longer equals the stored outcome, or names none.
pub const STALE_OUTCOME_EVIDENCE: &str = "STALE_OUTCOME_EVIDENCE";

/// `Q32` one.
const ONE_Q32: i128 = 1 << 32;
/// Most outcome references one node cites.
pub const MAX_OUTCOME_REFS: usize = 64;
/// The attribute of a skill component that names its parent skill.
pub const PARENT_ATTRIBUTE: &str = "parent";

/// One stored outcome a score leans on, pinned by revision and digest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OutcomeRef {
    pub record_id: String,
    /// How many evaluations the record held; the log only appends.
    pub outcome_revision: u64,
    pub outcome_digest: String,
}

/// One kept skill of a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillNode {
    pub skill_id: String,
    /// The nearest kept ancestor named by the `parent` attribute chain.
    #[serde(default)]
    pub parent_skill_id: Option<String>,
    pub score_q32: i64,
    /// One-based position by score descending, then skill id ascending.
    pub rank: u16,
    /// The context digest the outcome match used.
    pub context_digest: String,
    pub prior_outcome_refs: BoundedVec<OutcomeRef, 64>,
}

/// The ranked skills of one subtask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TaskSkillTree {
    pub task_id: String,
    pub nodes: BoundedVec<SkillNode, 32>,
}

/// The knobs a ranked tree reads, resolved from the pinned decision policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillTreeKnobs {
    pub min_outcomes: u64,
    pub outcome_weight_micros: i64,
    pub max_nodes: usize,
}

impl SkillTreeKnobs {
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let min_outcomes = match policy.knob(&SKILL_TREE_MIN_OUTCOMES) {
            KnobValue::Integer(value) => u64::try_from(value).unwrap_or(5),
            _ => 5,
        };
        let outcome_weight_micros = match policy.knob(&SKILL_TREE_OUTCOME_WEIGHT) {
            KnobValue::DecimalMicros(value) => value,
            _ => 300_000,
        };
        let max_nodes = match policy.knob(&SKILL_TREE_MAX_NODES) {
            KnobValue::Integer(value) => usize::try_from(value).unwrap_or(8),
            _ => 8,
        };
        Self {
            min_outcomes,
            outcome_weight_micros,
            max_nodes,
        }
    }
}

/// The matching outcomes of one record for one skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorOutcome {
    pub reference: OutcomeRef,
    pub successes: u64,
    pub trials: u64,
}

/// One visible skill of a task with its relevance and matching outcomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillCandidate {
    pub skill_id: String,
    pub relevance_q32: i64,
    pub outcomes: Vec<PriorOutcome>,
}

/// The visible skills and the `parent` attribute each declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillHierarchy {
    pub visible: BTreeSet<String>,
    pub parents: BTreeMap<String, String>,
}

fn refusal(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

/// `clamp(relevance + weight * (success_rate - 1/2), 0, 1)` in exact integer
/// arithmetic on `Q32`; below `min_outcomes` the relevance stands alone.
pub fn tree_score_q32(
    relevance_q32: i64,
    successes: u64,
    trials: u64,
    knobs: &SkillTreeKnobs,
) -> i64 {
    let mut score = i128::from(relevance_q32);
    if trials > 0 && trials >= knobs.min_outcomes {
        // weight * (s/t - 1/2) = weight * (2s - t) / (2t), floored.
        let spread = 2 * i128::from(successes) - i128::from(trials);
        let numerator = i128::from(knobs.outcome_weight_micros) * spread * ONE_Q32;
        let denominator = 2 * 1_000_000 * i128::from(trials);
        score += numerator.div_euclid(denominator);
    }
    score.clamp(0, ONE_Q32) as i64
}

/// Rank `candidates` for `task_id` and link each kept skill to its nearest
/// kept ancestor. Refuses a cycle or a parent the tenant does not see.
pub fn rank_skill_tree(
    task_id: &str,
    context_digest: &str,
    candidates: Vec<SkillCandidate>,
    hierarchy: &SkillHierarchy,
    knobs: &SkillTreeKnobs,
) -> Result<TaskSkillTree, String> {
    let mut scored: Vec<(i64, SkillCandidate)> = candidates
        .into_iter()
        .map(|candidate| {
            let successes = candidate.outcomes.iter().map(|o| o.successes).sum();
            let trials = candidate.outcomes.iter().map(|o| o.trials).sum();
            let score = tree_score_q32(candidate.relevance_q32, successes, trials, knobs);
            (score, candidate)
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.skill_id.cmp(&b.1.skill_id)));
    scored.truncate(knobs.max_nodes);
    let kept: BTreeSet<&str> = scored.iter().map(|(_, c)| c.skill_id.as_str()).collect();
    let mut nodes = Vec::with_capacity(scored.len());
    for (index, (score, candidate)) in scored.iter().enumerate() {
        let mut refs: Vec<OutcomeRef> = candidate
            .outcomes
            .iter()
            .map(|o| o.reference.clone())
            .collect();
        refs.sort();
        refs.truncate(MAX_OUTCOME_REFS);
        nodes.push(SkillNode {
            skill_id: candidate.skill_id.clone(),
            parent_skill_id: nearest_kept_ancestor(&candidate.skill_id, &kept, hierarchy)?,
            score_q32: *score,
            rank: u16::try_from(index + 1).map_err(|e| refusal("plan_bounds", e))?,
            context_digest: context_digest.to_string(),
            prior_outcome_refs: BoundedVec::new(refs).map_err(|e| refusal("plan_bounds", e))?,
        });
    }
    Ok(TaskSkillTree {
        task_id: task_id.to_string(),
        nodes: BoundedVec::new(nodes).map_err(|e| refusal("plan_bounds", e))?,
    })
}

fn nearest_kept_ancestor(
    skill: &str,
    kept: &BTreeSet<&str>,
    hierarchy: &SkillHierarchy,
) -> Result<Option<String>, String> {
    let mut seen: BTreeSet<&str> = BTreeSet::from([skill]);
    let mut current = skill;
    while let Some(parent) = hierarchy.parents.get(current) {
        if !hierarchy.visible.contains(parent) {
            return Err(refusal(
                SKILL_PARENT_UNKNOWN,
                format!("{current} names parent {parent}"),
            ));
        }
        if !seen.insert(parent.as_str()) {
            return Err(refusal(
                SKILL_TREE_CYCLE,
                format!("{current} returns to {parent}"),
            ));
        }
        if kept.contains(parent.as_str()) {
            return Ok(Some(parent.clone()));
        }
        current = parent;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HALF: i64 = 1 << 31;

    fn knobs() -> SkillTreeKnobs {
        SkillTreeKnobs::from_policy(&DecisionPolicy::engine_default())
    }

    fn candidate(id: &str, relevance: i64, successes: u64, trials: u64) -> SkillCandidate {
        SkillCandidate {
            skill_id: id.into(),
            relevance_q32: relevance,
            outcomes: if trials == 0 {
                Vec::new()
            } else {
                vec![PriorOutcome {
                    reference: OutcomeRef {
                        record_id: format!("decision:{id}"),
                        outcome_revision: trials,
                        outcome_digest: "d".into(),
                    },
                    successes,
                    trials,
                }]
            },
        }
    }

    fn hierarchy(pairs: &[(&str, Option<&str>)]) -> SkillHierarchy {
        SkillHierarchy {
            visible: pairs.iter().map(|(s, _)| s.to_string()).collect(),
            parents: pairs
                .iter()
                .filter_map(|(s, p)| p.map(|p| (s.to_string(), p.to_string())))
                .collect(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R134.9
    #[test]
    fn default_knobs_are_the_declared_defaults() {
        let k = knobs();
        assert_eq!(
            (k.min_outcomes, k.outcome_weight_micros, k.max_nodes),
            (5, 300_000, 8)
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R134.9
    #[test]
    fn outcome_rate_moves_the_score_only_at_support_and_is_clamped() {
        let k = knobs();
        assert_eq!(tree_score_q32(HALF, 4, 4, &k), HALF, "below min_outcomes");
        // 5/5 successes: +0.3 * 0.5 = +0.15 of one.
        let up = tree_score_q32(HALF, 5, 5, &k);
        assert_eq!(up, HALF + (3 * (1_i64 << 32)) / 20);
        let down = tree_score_q32(HALF, 0, 5, &k);
        // A negative delta floors, so it is one unit further from the relevance.
        assert_eq!(down, HALF - (3 * (1_i64 << 32)) / 20 - 1);
        assert_eq!(tree_score_q32(1 << 32, 5, 5, &k), 1 << 32);
        assert_eq!(tree_score_q32(0, 0, 5, &k), 0);
    }

    // spec: MEDULLA-DECISION-ENGINE-R134.9
    #[test]
    fn ranks_by_score_then_id_and_keeps_at_most_max_nodes() {
        let h = hierarchy(&[("a", None), ("b", None), ("c", None), ("d", None)]);
        let mut k = knobs();
        k.max_nodes = 3;
        let tree = rank_skill_tree(
            "t",
            "ctx",
            vec![
                candidate("c", HALF, 0, 0),
                candidate("b", HALF, 0, 0),
                candidate("a", HALF - 1, 0, 0),
                candidate("d", HALF + 5, 0, 0),
            ],
            &h,
            &k,
        )
        .unwrap();
        let order: Vec<(&str, u16)> = tree
            .nodes
            .iter()
            .map(|n| (n.skill_id.as_str(), n.rank))
            .collect();
        assert_eq!(order, vec![("d", 1), ("b", 2), ("c", 3)]);
    }

    // spec: MEDULLA-DECISION-ENGINE-R134.9
    #[test]
    fn parent_is_the_nearest_kept_ancestor() {
        let h = hierarchy(&[("leaf", Some("mid")), ("mid", Some("root")), ("root", None)]);
        let both = rank_skill_tree(
            "t",
            "ctx",
            vec![
                candidate("leaf", HALF, 0, 0),
                candidate("root", HALF + 1, 0, 0),
            ],
            &h,
            &knobs(),
        )
        .unwrap();
        let leaf = both.nodes.iter().find(|n| n.skill_id == "leaf").unwrap();
        assert_eq!(leaf.parent_skill_id.as_deref(), Some("root"));
        let root = both.nodes.iter().find(|n| n.skill_id == "root").unwrap();
        assert_eq!(root.parent_skill_id, None);
    }

    // spec: MEDULLA-DECISION-ENGINE-R134.9
    #[test]
    fn cycle_and_unknown_parent_refuse_with_their_codes() {
        let cyc = hierarchy(&[("a", Some("b")), ("b", Some("a"))]);
        let err = rank_skill_tree("t", "c", vec![candidate("a", HALF, 0, 0)], &cyc, &knobs())
            .unwrap_err();
        assert!(err.starts_with("SKILL_TREE_CYCLE: "), "{err}");
        let own = hierarchy(&[("a", Some("a"))]);
        let err = rank_skill_tree("t", "c", vec![candidate("a", HALF, 0, 0)], &own, &knobs())
            .unwrap_err();
        assert!(err.starts_with("SKILL_TREE_CYCLE: "), "{err}");
        let mut unknown = hierarchy(&[("a", None)]);
        unknown.parents.insert("a".into(), "ghost".into());
        let err = rank_skill_tree(
            "t",
            "c",
            vec![candidate("a", HALF, 0, 0)],
            &unknown,
            &knobs(),
        )
        .unwrap_err();
        assert!(err.starts_with("SKILL_PARENT_UNKNOWN: "), "{err}");
    }
}
