//! MEDULLA-SDLC-R013: a bundle is complete when its rows are delivered and the latest run of
//! its phase on main passed.

use std::collections::BTreeSet;

use medulla_sdlc::lifecycle::scenario::{bundle_completeness, latest_main_run};
use medulla_source_types::lifecycle::{ScenarioResult, ScenarioRun, StoredScenarioRun};

fn stored(phase: &str, branch: &str, result: ScenarioResult, at: u64, id: &str) -> StoredScenarioRun {
    StoredScenarioRun {
        id: id.to_string(),
        run: ScenarioRun {
            schema_version: 1,
            phase: phase.to_string(),
            scenario_id: "scenario:journey".to_string(),
            commit: format!("commit-{id}"),
            branch: branch.to_string(),
            result,
            requirement_ids: Vec::new(),
            evidence_digests: Vec::new(),
        },
        recorded_ms: at,
    }
}

fn rows(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

// spec: MEDULLA-SDLC-R013
#[test]
fn bundle_completes_when_rows_delivered_and_scenario_passed() {
    let scoped = rows(&["R1", "R2"]);
    let delivered: BTreeSet<String> = rows(&["R1", "R2"]).into_iter().collect();
    let runs = [stored("p1", "main", ScenarioResult::Passed, 10, "a")];
    let done = bundle_completeness(&scoped, &delivered, "p1", &runs);
    assert!(done.complete && done.undelivered_rows.is_empty());
}

// spec: MEDULLA-SDLC-R013
#[test]
fn a_verified_but_undelivered_row_keeps_the_bundle_open() {
    let scoped = rows(&["R1", "R2"]);
    let delivered: BTreeSet<String> = rows(&["R1"]).into_iter().collect();
    let runs = [stored("p1", "main", ScenarioResult::Passed, 10, "a")];
    let open = bundle_completeness(&scoped, &delivered, "p1", &runs);
    assert!(!open.complete);
    assert_eq!(open.undelivered_rows, rows(&["R2"]));
}

// spec: MEDULLA-SDLC-R013
#[test]
fn only_the_latest_run_of_the_phase_on_main_counts() {
    let scoped = rows(&["R1"]);
    let delivered: BTreeSet<String> = rows(&["R1"]).into_iter().collect();
    // A later failure on main reopens the bundle; a failure on a topic branch, another
    // phase's run and an earlier failure do not.
    let runs = [
        stored("p1", "main", ScenarioResult::Failed, 5, "early"),
        stored("p1", "main", ScenarioResult::Passed, 10, "pass"),
        stored("p1", "topic", ScenarioResult::Failed, 20, "topic"),
        stored("p2", "main", ScenarioResult::Failed, 30, "other"),
    ];
    assert!(bundle_completeness(&scoped, &delivered, "p1", &runs).complete);
    let mut reversed = runs.to_vec();
    reversed.reverse();
    assert!(bundle_completeness(&scoped, &delivered, "p1", &reversed).complete);
    let mut failing = runs.to_vec();
    failing.push(stored("p1", "main", ScenarioResult::Failed, 40, "late"));
    assert!(!bundle_completeness(&scoped, &delivered, "p1", &failing).complete);
    assert_eq!(latest_main_run("p1", &failing).map(|r| r.id.as_str()), Some("late"));
}

// spec: MEDULLA-SDLC-R013
#[test]
fn a_phase_without_a_run_is_not_complete() {
    let scoped = rows(&["R1"]);
    let delivered: BTreeSet<String> = rows(&["R1"]).into_iter().collect();
    let none = bundle_completeness(&scoped, &delivered, "p1", &[]);
    assert!(!none.complete && none.latest_run.is_none());
}
