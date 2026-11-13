//! Planning over the skill library (MEDULLA-DECISION-ENGINE-R134.4).
//!
//! The harness produces atomic subtasks; this turns them into a
//! [`TaskPlanProposal`] whose tasks each name one library skill. Relevance is
//! the candidate-local BM25 of the subtask text against the visible skill
//! texts, saturated as `s / (s + k)` and quantised once onto `Q32`. The
//! proposal is canonicalized, cycle-checked and digested by
//! `plan_explicit_tasks`; this module adds no second canonicalizer.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::decision::task_plan::skill_plan::{
    SkillPlanKnobs, SkillPlanQuery, EMPTY_SKILL_INDEX, NO_SKILL_FOR_SUBTASK,
};
use medulla_decision_types::decision::task_plan::{
    independent_ids, TaskPlanProposal, TaskPlanTask, MAX_CONCURRENCY,
};
use medulla_types::contract::BoundedVec;

use super::bm25;
use super::quant::q32;

/// The metadata text of one tenant-visible published skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDocument {
    /// The skill's component id; the value written to `assignee`.
    pub id: String,
    /// The metadata text relevance is scored against.
    pub text: String,
}

/// One subtask after skill choice, before merging and splitting.
struct Chosen {
    id: String,
    text: String,
    deps: BTreeSet<String>,
    skill: String,
    /// The `Q32` relevance of the chosen skill; a merged task keeps the lowest
    /// of its members, so the claim is the weakest one.
    relevance_q32: i64,
}

/// A skill plan: the proposal and the relevance each planned task was chosen
/// with (MEDULLA-DECISION-ENGINE-R134.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPlan {
    pub proposal: TaskPlanProposal,
    /// `Q32` relevance per task id; split parts share their subtask's value.
    pub relevance_q32: BTreeMap<String, i64>,
}

fn refusal(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

/// Whether a `Q32` relevance reaches a floor given in millionths, in exact
/// integer arithmetic.
fn reaches(relevance_q32: i64, floor_micros: i64) -> bool {
    i128::from(relevance_q32) * 1_000_000 >= i128::from(floor_micros) << 32
}

/// The relevance of every visible skill to each subtask, as `Q32`: subtasks in
/// id order, skills in id order. A duplicate subtask id refuses.
pub fn skill_relevances(
    query: &SkillPlanQuery,
    skills: &[SkillDocument],
    knobs: &SkillPlanKnobs,
) -> Result<Vec<(String, Vec<(String, i64)>)>, String> {
    let mut ordered: Vec<&SkillDocument> = skills.iter().collect();
    ordered.sort_by(|a, b| a.id.cmp(&b.id));
    let documents: Vec<&str> = ordered.iter().map(|s| s.text.as_str()).collect();
    let saturation = knobs.bm25_saturation_micros as f64 / 1_000_000.0;
    let mut subtasks: Vec<_> = query.subtasks.iter().collect();
    subtasks.sort_by(|a, b| a.id.cmp(&b.id));
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(subtasks.len());
    for subtask in subtasks {
        if !seen.insert(subtask.id.as_str()) {
            return Err(refusal("DUPLICATE_TASK_ID", &subtask.id));
        }
        let scores = bm25::scores(&documents, &subtask.text);
        let mut row = Vec::with_capacity(ordered.len());
        for (skill, score) in ordered.iter().zip(scores) {
            let relevance = q32(score / (score + saturation))
                .map_err(|r| r.render())?
                .value;
            row.push((skill.id.clone(), relevance));
        }
        out.push((subtask.id.clone(), row));
    }
    Ok(out)
}

/// Whether a `Q32` relevance reaches a floor given in millionths.
pub fn relevance_reaches(relevance_q32: i64, floor_micros: i64) -> bool {
    reaches(relevance_q32, floor_micros)
}

/// Choose, per subtask, the most relevant skill at or above the floor. Ties go
/// to the lowest skill id. The first uncovered subtask id (in id order) refuses.
fn choose(
    query: &SkillPlanQuery,
    skills: &[SkillDocument],
    knobs: &SkillPlanKnobs,
) -> Result<Vec<Chosen>, String> {
    let rows = skill_relevances(query, skills, knobs)?;
    let mut chosen = Vec::with_capacity(rows.len());
    for (id, row) in rows {
        let mut best: Option<(i64, &str)> = None;
        for (skill, relevance) in &row {
            // Strictly greater replaces, so the lowest skill id wins a tie.
            if best.map_or(true, |(have, _)| *relevance > have) {
                best = Some((*relevance, skill.as_str()));
            }
        }
        let Some((relevance_q32, skill)) =
            best.filter(|(r, _)| reaches(*r, knobs.relevance_floor_micros))
        else {
            return Err(refusal(NO_SKILL_FOR_SUBTASK, &id));
        };
        let subtask = query
            .subtasks
            .iter()
            .find(|subtask| subtask.id == id)
            .expect("the relevance rows name query subtasks");
        chosen.push(Chosen {
            id,
            text: subtask.text.clone(),
            deps: subtask.depends_on.iter().cloned().collect(),
            skill: skill.to_string(),
            relevance_q32,
        });
    }
    Ok(chosen)
}

/// Merge subtasks that pick the same skill and have equal dependencies while
/// the joined prompt stays within the cap. The merged task keeps the lowest id.
/// Returns the surviving tasks and the id each merged-away id now maps to.
fn merge(chosen: Vec<Chosen>, cap: usize) -> (Vec<Chosen>, BTreeMap<String, String>) {
    let mut groups: BTreeMap<(String, Vec<String>), Vec<Chosen>> = BTreeMap::new();
    for item in chosen {
        let key = (item.skill.clone(), item.deps.iter().cloned().collect());
        groups.entry(key).or_default().push(item);
    }
    let mut kept: Vec<Chosen> = Vec::new();
    let mut map = BTreeMap::new();
    for (_, items) in groups {
        let mut current: Option<Chosen> = None;
        for item in items {
            current = match current.take() {
                Some(mut open) if open.text.len() + 1 + item.text.len() <= cap => {
                    map.insert(item.id.clone(), open.id.clone());
                    open.relevance_q32 = open.relevance_q32.min(item.relevance_q32);
                    open.text.push('\n');
                    open.text.push_str(&item.text);
                    Some(open)
                }
                Some(open) => {
                    kept.push(open);
                    Some(item)
                }
                None => Some(item),
            };
        }
        kept.extend(current);
    }
    kept.sort_by(|a, b| a.id.cmp(&b.id));
    (kept, map)
}

/// Cut `text` into parts of at most `cap` bytes at line boundaries. A line
/// longer than the cap is cut at the last character boundary that fits.
fn cut(text: &str, cap: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut open = String::new();
    for line in text.split_inclusive('\n') {
        let mut rest = line;
        while open.len() + rest.len() > cap {
            if open.is_empty() {
                let mut end = cap.min(rest.len());
                while !rest.is_char_boundary(end) {
                    end -= 1;
                }
                parts.push(rest[..end].to_string());
                rest = &rest[end..];
            } else {
                parts.push(std::mem::take(&mut open));
            }
        }
        open.push_str(rest);
    }
    if !open.is_empty() {
        parts.push(open);
    }
    parts
}

/// Plan `query` over `skills`: choose, merge, split and bound.
pub fn plan_over_skills(
    query: &SkillPlanQuery,
    skills: &[SkillDocument],
    knobs: &SkillPlanKnobs,
) -> Result<SkillPlan, String> {
    if skills.is_empty() {
        return Err(refusal(EMPTY_SKILL_INDEX, "no published skill is visible"));
    }
    let cap = knobs.split_prompt_bytes;
    let (merged, merged_into) = merge(choose(query, skills, knobs)?, cap);
    // A split subtask is replaced by a chain; its dependants wait on the last part.
    let mut last_part: BTreeMap<String, String> = BTreeMap::new();
    let mut chains: Vec<(Chosen, Vec<String>)> = Vec::new();
    for item in merged {
        let parts = if item.text.len() > cap {
            cut(&item.text, cap)
        } else {
            vec![item.text.clone()]
        };
        if parts.len() > 1 {
            last_part.insert(item.id.clone(), format!("{}.{}", item.id, parts.len()));
        }
        chains.push((item, parts));
    }
    let resolve = |id: &str| {
        let kept = merged_into.get(id).map_or(id, String::as_str);
        last_part
            .get(kept)
            .map_or_else(|| kept.to_string(), Clone::clone)
    };
    let mut tasks = Vec::new();
    let mut relevance_q32 = BTreeMap::new();
    for (item, parts) in &chains {
        let deps: BTreeSet<String> = item.deps.iter().map(|d| resolve(d)).collect();
        let total = parts.len();
        for (index, part) in parts.iter().enumerate() {
            let (task_id, depends_on) = if total == 1 {
                (item.id.clone(), deps.iter().cloned().collect::<Vec<_>>())
            } else if index == 0 {
                (format!("{}.1", item.id), deps.iter().cloned().collect())
            } else {
                (
                    format!("{}.{}", item.id, index + 1),
                    vec![format!("{}.{}", item.id, index)],
                )
            };
            relevance_q32.insert(task_id.clone(), item.relevance_q32);
            tasks.push(TaskPlanTask {
                task_id,
                prompt: part.clone(),
                depends_on: BoundedVec::new(depends_on).map_err(|d| refusal("PLAN_BOUNDS", d))?,
                call: None,
                aggregation: None,
                assignee: Some(item.skill.clone()),
                branch: None,
                join: None,
                decision: None,
                approval: None,
                duration_ms: None,
                priority_weight: None,
            });
        }
    }
    let independent = independent_ids(&tasks).len();
    let max_steps = tasks.len() as u16;
    Ok(SkillPlan {
        proposal: TaskPlanProposal {
            tasks: BoundedVec::new(tasks).map_err(|d| refusal("PLAN_BOUNDS", d))?,
            output_task_id: resolve(&query.output_task_id),
            max_concurrency: independent.clamp(1, usize::from(MAX_CONCURRENCY)) as u16,
            max_steps,
            final_artifact: None,
        },
        relevance_q32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::task_plan::plan_explicit_tasks;
    use medulla_decision_types::decision::task_plan::skill_plan::SkillSubtask;

    fn knobs() -> SkillPlanKnobs {
        SkillPlanKnobs {
            bm25_saturation_micros: 2_000_000,
            relevance_floor_micros: 200_000,
            split_prompt_bytes: 4096,
        }
    }

    fn skills() -> Vec<SkillDocument> {
        [
            ("skill-b-search", "search documents archive retrieval index"),
            ("skill-a-search", "search documents archive retrieval index"),
            ("skill-deploy", "deploy cluster rollout kubernetes release"),
            (
                "skill-report",
                "summarise findings report writing narrative",
            ),
        ]
        .into_iter()
        .map(|(id, text)| SkillDocument {
            id: id.into(),
            text: text.into(),
        })
        .collect()
    }

    fn subtask(id: &str, text: &str, deps: &[&str]) -> SkillSubtask {
        SkillSubtask {
            id: id.into(),
            text: text.into(),
            depends_on: BoundedVec::new(deps.iter().map(|d| d.to_string()).collect()).unwrap(),
        }
    }

    fn query(output: &str, subtasks: Vec<SkillSubtask>) -> SkillPlanQuery {
        SkillPlanQuery {
            output_task_id: output.into(),
            subtasks: BoundedVec::new(subtasks).unwrap(),
            outcome_pins: BoundedVec::default(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R134.4
    #[test]
    fn skill_plan_respects_dependencies() {
        let q = query(
            "z-report",
            vec![
                subtask("a1", "search documents archive retrieval", &[]),
                subtask("a2", "search index retrieval documents", &[]),
                subtask("b1", "deploy cluster rollout release", &["a1", "a2"]),
                subtask(
                    "z-report",
                    "summarise findings report narrative",
                    &["b1", "a1"],
                ),
            ],
        );
        let proposal = plan_over_skills(&q, &skills(), &knobs()).unwrap().proposal;
        let plan = plan_explicit_tasks(&proposal).unwrap();
        let position: BTreeMap<&str, usize> = plan
            .tasks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.task_id.as_str(), i))
            .collect();
        for task in plan.tasks.iter() {
            assert!(task.assignee.is_some(), "{} names one skill", task.task_id);
            for dependency in task.depends_on.iter() {
                assert!(position[dependency.as_str()] < position[task.task_id.as_str()]);
            }
        }
        // Equal-skill tasks with equal dependencies merge into the lowest id;
        // the tie between the two search skills goes to the lowest skill id.
        assert_eq!(plan.tasks.len(), 3);
        let merged = plan.tasks.iter().find(|t| t.task_id == "a1").unwrap();
        assert_eq!(merged.assignee.as_deref(), Some("skill-a-search"));
        assert!(merged.prompt.contains("retrieval documents"));
        assert!(plan.tasks.iter().all(|t| t.task_id != "a2"));
        let deploy = plan.tasks.iter().find(|t| t.task_id == "b1").unwrap();
        assert_eq!(deploy.depends_on.as_slice(), ["a1"]);
        assert_eq!(deploy.assignee.as_deref(), Some("skill-deploy"));
        assert_eq!(plan.output_task_id, "z-report");
        // An oversize prompt splits into a chain whose last part keeps the dependants.
        let long: String = (0..20)
            .map(|n| format!("search documents archive retrieval line {n:03}\n"))
            .collect();
        let tight = SkillPlanKnobs {
            split_prompt_bytes: 256,
            ..knobs()
        };
        let q = query(
            "z-report",
            vec![
                subtask("big", &long, &[]),
                subtask("z-report", "summarise findings report narrative", &["big"]),
            ],
        );
        let plan = plan_explicit_tasks(&plan_over_skills(&q, &skills(), &tight).unwrap().proposal)
            .unwrap();
        let parts: Vec<&TaskPlanTask> = plan
            .tasks
            .iter()
            .filter(|t| t.task_id.starts_with("big."))
            .collect();
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|p| p.prompt.len() <= 256));
        assert_eq!(
            parts.iter().map(|p| p.prompt.as_str()).collect::<String>(),
            long
        );
        assert_eq!(parts[0].task_id, "big.1");
        assert!(parts[0].depends_on.is_empty());
        for pair in parts.windows(2) {
            assert_eq!(pair[1].depends_on.as_slice(), [pair[0].task_id.clone()]);
        }
        let last = format!("big.{}", parts.len());
        let report = plan.tasks.iter().find(|t| t.task_id == "z-report").unwrap();
        assert_eq!(report.depends_on.as_slice(), [last]);
        // Subtasks that would exceed the cap together do not merge.
        let q = query(
            "z",
            vec![
                subtask("m1", &long[..200], &[]),
                subtask("m2", &long[..200], &[]),
                subtask("z", "summarise findings report narrative", &["m1", "m2"]),
            ],
        );
        let plan = plan_explicit_tasks(&plan_over_skills(&q, &skills(), &tight).unwrap().proposal)
            .unwrap();
        assert_eq!(plan.tasks.len(), 3);
    }

    // spec: MEDULLA-DECISION-ENGINE-R134.4
    #[test]
    fn uncovered_subtask_and_empty_index_refuse_with_their_codes() {
        let q = query(
            "z",
            vec![
                subtask("a", "search documents archive retrieval", &[]),
                subtask("m", "quantum chromodynamics lattice", &[]),
                subtask("n", "orbital mechanics ephemeris", &[]),
                subtask("z", "summarise findings report narrative", &["a", "m", "n"]),
            ],
        );
        let error = plan_over_skills(&q, &skills(), &knobs()).unwrap_err();
        assert_eq!(error, "NO_SKILL_FOR_SUBTASK: m");
        let error = plan_over_skills(&q, &[], &knobs()).unwrap_err();
        assert!(error.starts_with("EMPTY_SKILL_INDEX"));
    }

    // spec: MEDULLA-DECISION-ENGINE-R134.4
    #[test]
    fn a_higher_floor_leaves_a_weak_match_uncovered() {
        let q = query(
            "z",
            vec![
                subtask("a", "search", &[]),
                subtask("z", "summarise findings report narrative", &["a"]),
            ],
        );
        assert!(plan_over_skills(&q, &skills(), &knobs()).is_ok());
        let strict = SkillPlanKnobs {
            relevance_floor_micros: 990_000,
            ..knobs()
        };
        assert_eq!(
            plan_over_skills(&q, &skills(), &strict).unwrap_err(),
            "NO_SKILL_FOR_SUBTASK: a"
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R134.4
    #[test]
    fn a_cycle_is_left_to_the_shipped_cycle_check() {
        let q = query(
            "b",
            vec![
                subtask("a", "search documents archive retrieval", &["b"]),
                subtask("b", "deploy cluster rollout release", &["a"]),
            ],
        );
        let proposal = plan_over_skills(&q, &skills(), &knobs()).unwrap().proposal;
        assert!(plan_explicit_tasks(&proposal)
            .unwrap_err()
            .starts_with("PLAN_CYCLE"));
    }
}
