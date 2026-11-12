//! Requirement state derived from the changeset log and bound tests
//! (MEDULLA-SDLC-R005.2), per the shared spec standard:
//!
//! ```text
//! LANDED   := a default-branch changeset cites the ID
//! VERIFIED := LANDED and a bound test exists in the tree at the revision
//! rollup   := lowest state of its children
//! withdraw := a Spec-Withdraw trailer removes the evidence for its IDs
//! ```

use std::collections::{BTreeMap, BTreeSet};

use medulla_source_types::{
    lifecycle::SpecRowState, lifecycle::SpecState, lifecycle::SpecStatusAnswer,
    lifecycle::TaskRowState, lifecycle::TaskState, lifecycle::TaskStatusAnswer,
};
use medulla_types::source_changeset::Changeset;

use super::derive::TestBinding;

#[derive(Default)]
struct Evidence {
    landed_by: Vec<String>,
    withdrawn: bool,
}

fn replay_chain(chain: &[Changeset]) -> BTreeMap<String, Evidence> {
    let mut evidence: BTreeMap<String, Evidence> = BTreeMap::new();
    for changeset in chain {
        for id in &changeset.trailers.spec_ids {
            let entry = evidence.entry(id.clone()).or_default();
            entry.landed_by.push(changeset.id.clone());
            entry.withdrawn = false;
        }
        for id in &changeset.trailers.withdraw {
            let entry = evidence.entry(id.clone()).or_default();
            entry.landed_by.clear();
            entry.withdrawn = true;
        }
    }
    evidence
}

fn leaf_row(id: &str, evidence: Option<&Evidence>, bindings: &[TestBinding]) -> SpecRowState {
    let landed_by = evidence.map(|e| e.landed_by.clone()).unwrap_or_default();
    let withdrawn = evidence.is_some_and(|e| e.withdrawn);
    let mut verified_by: Vec<String> = if landed_by.is_empty() {
        Vec::new()
    } else {
        bindings
            .iter()
            .filter(|b| b.requirement == id)
            .map(|b| b.test.clone())
            .collect()
    };
    verified_by.sort();
    verified_by.dedup();
    let state = match (landed_by.is_empty(), verified_by.is_empty()) {
        (true, _) => SpecState::Specified,
        (false, true) => SpecState::Landed,
        (false, false) => SpecState::Verified,
    };
    SpecRowState {
        id: id.to_owned(),
        state,
        landed_by,
        verified_by,
        withdrawn,
    }
}

/// The nearest existing ancestor of `id` at a `.` boundary.
fn parent_of<'a>(id: &str, ids: &'a [String]) -> Option<&'a String> {
    let mut candidate = id;
    while let Some((head, _)) = candidate.rsplit_once('.') {
        if let Some(found) = ids.iter().find(|other| other.as_str() == head) {
            return Some(found);
        }
        candidate = head;
    }
    None
}

fn rollup(row: &mut SpecRowState, children: &[&SpecRowState]) {
    row.state = children.iter().map(|c| c.state).min().unwrap_or(row.state);
    for field in [0, 1] {
        let mut merged: Vec<String> = children
            .iter()
            .flat_map(|c| {
                if field == 0 {
                    &c.landed_by
                } else {
                    &c.verified_by
                }
            })
            .cloned()
            .collect();
        merged.sort();
        merged.dedup();
        if field == 0 {
            row.landed_by = merged;
        } else {
            row.verified_by = merged;
        }
    }
    row.withdrawn = children.iter().any(|c| c.withdrawn);
}

/// Derive every row state of one repository at one revision. `chain` is the
/// default-branch changeset chain, oldest first; `bindings` are the test
/// binding markers in the tree at `rev`. Rows keep the order of `ids`.
pub fn derive_spec_status(
    repo: &str,
    rev: &str,
    ids: &[String],
    chain: &[Changeset],
    bindings: &[TestBinding],
) -> SpecStatusAnswer {
    let evidence = replay_chain(chain);
    let mut rows: BTreeMap<String, SpecRowState> = ids
        .iter()
        .map(|id| (id.clone(), leaf_row(id, evidence.get(id), bindings)))
        .collect();
    let mut children: BTreeMap<&String, Vec<&String>> = BTreeMap::new();
    for id in ids {
        if let Some(parent) = parent_of(id, ids) {
            children.entry(parent).or_default().push(id);
        }
    }
    // Deepest ids first, so a child is final before its parent reads it.
    let mut order: Vec<&String> = ids.iter().collect();
    order.sort_by_key(|id| std::cmp::Reverse(id.matches('.').count()));
    for id in order {
        let Some(kids) = children.get(id) else {
            continue;
        };
        let snapshot: Vec<SpecRowState> = kids.iter().map(|k| rows[*k].clone()).collect();
        let refs: Vec<&SpecRowState> = snapshot.iter().collect();
        if let Some(row) = rows.get_mut(id) {
            rollup(row, &refs);
        }
    }
    SpecStatusAnswer {
        repo: repo.to_owned(),
        rev: rev.to_owned(),
        rows: ids.iter().map(|id| rows[id].clone()).collect(),
    }
}

/// One Task record as the parser of R014.1 reads it: the T-id (or line
/// key), whether its box is ticked on the default-branch tree, and the
/// requirement row it refers to, when it has one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRecord {
    pub id: String,
    pub ticked: bool,
    pub row: Option<String>,
}

/// Derive every task state of one repository at one revision from the
/// Task records of R014.1 and their `dependsOn` edges.
///
/// `tasks` are the Task records of the default-branch tree in row order.
/// `row_depends_on` maps each requirement row to the row ids its
/// `Depends on:` names. `row_states` is the answer of `derive_spec_status`
/// at the same revision: a `dependsOn` target that names no row is
/// unresolved. `contract_rows` names the rows an unresolved contract
/// (R014.4) touches.
///
/// A task is `Done` when its box is ticked on the default-branch tree.
/// Otherwise it is `Blocked` when its row has an unresolved `dependsOn`
/// target, or when its row is touched by an unresolved contract.
/// Otherwise it is `Open`. The order reuses the rollup order: deepest row
/// ids first, ties in task order. The state is derived on every call; no
/// file stores it.
pub fn derive_task_state(
    repo: &str,
    rev: &str,
    tasks: &[TaskRecord],
    row_depends_on: &BTreeMap<String, Vec<String>>,
    row_states: &SpecStatusAnswer,
    contract_rows: &BTreeSet<String>,
) -> TaskStatusAnswer {
    let known: BTreeMap<&str, &SpecRowState> =
        row_states.rows.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut order: Vec<usize> = (0..tasks.len()).collect();
    order.sort_by_key(|&i| {
        std::cmp::Reverse(
            tasks[i]
                .row
                .as_deref()
                .map(|row| row.matches('.').count())
                .unwrap_or(0),
        )
    });
    let mut states: BTreeMap<String, TaskRowState> = BTreeMap::new();
    for &i in &order {
        let task = &tasks[i];
        let mut blocked_by: Vec<String> = Vec::new();
        if let Some(row) = task.row.as_deref() {
            if let Some(targets) = row_depends_on.get(row) {
                for target in targets {
                    if !known.contains_key(target.as_str()) {
                        blocked_by.push(target.clone());
                    }
                }
            }
            if contract_rows.contains(row) {
                blocked_by.push(row.to_owned());
            }
        }
        blocked_by.sort();
        blocked_by.dedup();
        let state = if task.ticked {
            TaskState::Done
        } else if blocked_by.is_empty() {
            TaskState::Open
        } else {
            TaskState::Blocked
        };
        states.insert(
            task.id.clone(),
            TaskRowState {
                id: task.id.clone(),
                state,
                row: task.row.clone(),
                blocked_by,
            },
        );
    }
    TaskStatusAnswer {
        repo: repo.to_owned(),
        rev: rev.to_owned(),
        tasks: tasks.iter().map(|t| states[&t.id].clone()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    fn spec_rows(rows: &[(&str, SpecState)]) -> SpecStatusAnswer {
        SpecStatusAnswer {
            repo: "repo".to_owned(),
            rev: "r1".to_owned(),
            rows: rows
                .iter()
                .map(|(id, state)| SpecRowState {
                    id: (*id).to_owned(),
                    state: *state,
                    landed_by: Vec::new(),
                    verified_by: Vec::new(),
                    withdrawn: false,
                })
                .collect(),
        }
    }

    /// The engine derives each task state from its Task record: a ticked
    /// box is `done`; an open box is `blocked` by an unresolved `dependsOn`
    /// target or by an unresolved contract, and `open` otherwise.
    #[test]
    pub fn task_state_follows_checkbox_and_blockers() {
        // EG-R4 is absent from the tree, so it is an unresolved target.
        let rows = spec_rows(&[
            ("EG-R1", SpecState::Verified),
            ("EG-R2", SpecState::Specified),
            ("EG-R3", SpecState::Landed),
        ]);
        let tasks = vec![
            TaskRecord {
                id: "T001".to_owned(),
                ticked: false,
                row: Some("EG-R1".to_owned()),
            },
            TaskRecord {
                id: "T002".to_owned(),
                ticked: true,
                row: Some("EG-R2".to_owned()),
            },
            TaskRecord {
                id: "T003".to_owned(),
                ticked: false,
                row: Some("EG-R2".to_owned()),
            },
            TaskRecord {
                id: "T004".to_owned(),
                ticked: false,
                row: Some("EG-R3".to_owned()),
            },
            TaskRecord {
                id: "T005".to_owned(),
                ticked: true,
                row: Some("EG-R3".to_owned()),
            },
            TaskRecord {
                id: "T006".to_owned(),
                ticked: false,
                row: None,
            },
        ];
        let row_depends_on = BTreeMap::from([
            ("EG-R2".to_owned(), vec!["EG-R4".to_owned()]),
            ("EG-R3".to_owned(), vec!["EG-R2".to_owned()]),
        ]);
        // The contract over EG-R2 leaves its row unresolved (R014.4).
        let contract_rows: BTreeSet<String> = BTreeSet::from(["EG-R2".to_owned()]);
        let answer =
            derive_task_state("repo", "r1", &tasks, &row_depends_on, &rows, &contract_rows);
        let state =
            |id: &str| -> &TaskRowState { answer.tasks.iter().find(|t| t.id == id).unwrap() };
        assert_eq!(
            answer
                .tasks
                .iter()
                .map(|t| t.id.clone())
                .collect::<Vec<_>>(),
            vec!["T001", "T002", "T003", "T004", "T005", "T006"],
            "rows keep the task order"
        );
        assert_eq!(state("T001").state, TaskState::Open);
        assert_eq!(state("T002").state, TaskState::Done);
        assert_eq!(state("T003").state, TaskState::Blocked);
        assert_eq!(state("T003").blocked_by, vec!["EG-R2", "EG-R4"]);
        assert_eq!(state("T004").state, TaskState::Open);
        assert_eq!(state("T005").state, TaskState::Done);
        assert_eq!(state("T006").state, TaskState::Open);
        for t in &answer.tasks {
            let name = match t.state {
                TaskState::Open => "OPEN",
                TaskState::Blocked => "BLOCKED",
                TaskState::Done => "DONE",
            };
            println!("TASK_STATE\t{}\t{name}", t.id);
        }
    }
}
