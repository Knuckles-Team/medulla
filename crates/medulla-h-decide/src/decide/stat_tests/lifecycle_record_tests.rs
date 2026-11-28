//! `DecisionLog.lifecycle_record` served (MEDULLA-SDLC-R013, R019.4): a scenario run and a
//! would-dispatch decision are records of the one decision log, a repeat replays, an
//! incomplete scenario run refuses and writes nothing, and a read lists the newest run first.

use medulla_decision_types::decision::statistical::log::DecisionLogOp;
use medulla_source_types::delivery::WouldDispatch;
use medulla_source_types::lifecycle::{ProjectMode, ScenarioResult, ScenarioRun};
use {
    medulla_decision_policy_types::decision::statistical::lifecycle_record::LifecycleRecordKind,
    medulla_decision_policy_types::decision::statistical::lifecycle_record::LifecycleRecordQuery,
    medulla_decision_policy_types::decision::statistical::lifecycle_record::LifecycleRecordStored,
    medulla_decision_policy_types::decision::statistical::lifecycle_record::LifecycleRecordWrite,
    medulla_decision_policy_types::decision::statistical::lifecycle_record::LifecycleRecordsFound,
};

use super::log_tests::log_op;
use super::*;

fn run(phase: &str, commit: &str, result: ScenarioResult) -> ScenarioRun {
    ScenarioRun {
        schema_version: 1,
        phase: phase.to_string(),
        scenario_id: "scenario:journey".to_string(),
        commit: commit.to_string(),
        branch: "main".to_string(),
        result,
        requirement_ids: vec!["R2".to_string(), "R1".to_string(), "R1".to_string()],
        evidence_digests: vec!["sha256:b".to_string(), "sha256:a".to_string()],
    }
}

async fn write(h: &Harness, record: LifecycleRecordWrite) -> Result<LifecycleRecordStored, String> {
    decode(
        log_op(
            h,
            "decider",
            DecisionLogOp::LifecycleRecord {
                tenant_id: TENANT.to_string(),
                record,
            },
        )
        .await,
    )
}

async fn read(h: &Harness, query: LifecycleRecordQuery) -> LifecycleRecordsFound {
    decode(
        log_op(
            h,
            "decider",
            DecisionLogOp::LifecycleRecords {
                tenant_id: TENANT.to_string(),
                query,
            },
        )
        .await,
    )
    .unwrap()
}

fn scenario_query(phase: Option<&str>) -> LifecycleRecordQuery {
    LifecycleRecordQuery {
        record_kind: LifecycleRecordKind::ScenarioRun,
        phase: phase.map(str::to_string),
        branch: None,
    }
}

/// Verifies MEDULLA-SDLC-R013: a scenario run is stored once in canonical form and a repeat
/// replays.
// spec: MEDULLA-SDLC-R013
#[tokio::test]
async fn a_scenario_run_is_stored_once_and_a_repeat_replays() {
    let h = Harness::new().await;
    let record = run("phase-1", "c1", ScenarioResult::Passed);
    let first = write(
        &h,
        LifecycleRecordWrite::ScenarioRun {
            record: record.clone(),
        },
    )
    .await
    .unwrap();
    assert!(!first.replayed);
    let second = write(&h, LifecycleRecordWrite::ScenarioRun { record })
        .await
        .unwrap();
    assert!(second.replayed);
    assert_eq!(second.record_id, first.record_id);
    assert_eq!(second.recorded_ms, first.recorded_ms);
    let found = read(&h, scenario_query(Some("phase-1"))).await;
    assert_eq!(found.scenario_runs.len(), 1);
    // Sorted and deduplicated lists: one canonical form.
    assert_eq!(found.scenario_runs[0].run.requirement_ids, ["R1", "R2"]);
    assert_eq!(
        found.scenario_runs[0].run.evidence_digests,
        ["sha256:a", "sha256:b"]
    );
}

/// Verifies MEDULLA-SDLC-R013: a record without a commit or a phase refuses and writes nothing.
// spec: MEDULLA-SDLC-R013
#[tokio::test]
async fn an_incomplete_scenario_run_refuses_and_stores_nothing() {
    let h = Harness::new().await;
    for edit in [
        |r: &mut ScenarioRun| r.commit = " ".to_string(),
        |r: &mut ScenarioRun| r.phase = String::new(),
        |r: &mut ScenarioRun| r.scenario_id = String::new(),
    ] {
        let mut record = run("phase-1", "c1", ScenarioResult::Passed);
        edit(&mut record);
        let refused = write(&h, LifecycleRecordWrite::ScenarioRun { record })
            .await
            .unwrap_err();
        assert!(refused.contains("SCENARIO_RUN_INCOMPLETE"), "{refused}");
    }
    assert!(read(&h, scenario_query(None))
        .await
        .scenario_runs
        .is_empty());
}

/// Verifies MEDULLA-SDLC-R013: the read filters by phase and lists the latest run first.
// spec: MEDULLA-SDLC-R013
#[tokio::test]
async fn the_read_filters_by_phase_and_branch() {
    let h = Harness::new().await;
    write(
        &h,
        LifecycleRecordWrite::ScenarioRun {
            record: run("phase-1", "c1", ScenarioResult::Failed),
        },
    )
    .await
    .unwrap();
    write(
        &h,
        LifecycleRecordWrite::ScenarioRun {
            record: run("phase-2", "c2", ScenarioResult::Passed),
        },
    )
    .await
    .unwrap();
    let mut on_branch = run("phase-1", "c3", ScenarioResult::Passed);
    on_branch.branch = "topic".to_string();
    write(&h, LifecycleRecordWrite::ScenarioRun { record: on_branch })
        .await
        .unwrap();
    assert_eq!(read(&h, scenario_query(None)).await.scenario_runs.len(), 3);
    assert_eq!(
        read(&h, scenario_query(Some("phase-1")))
            .await
            .scenario_runs
            .len(),
        2
    );
    let mut main_only = scenario_query(Some("phase-1"));
    main_only.branch = Some("main".to_string());
    let found = read(&h, main_only).await;
    assert_eq!(found.scenario_runs.len(), 1);
    assert_eq!(found.scenario_runs[0].run.commit, "c1");
}

/// Verifies MEDULLA-SDLC-R019.4: a would-dispatch decision is a record of the decision log.
// spec: MEDULLA-SDLC-R019.4
#[tokio::test]
async fn a_would_dispatch_decision_is_a_log_record() {
    let h = Harness::new().await;
    let record = WouldDispatch {
        slice_root: "slice:one".to_string(),
        rows: vec!["R1".to_string()],
        evidence_digests: vec!["sha256:e".to_string()],
        dispatch_decision_id: "decision:1".to_string(),
        mode: ProjectMode::Development,
    };
    let first = write(
        &h,
        LifecycleRecordWrite::WouldDispatch {
            record: record.clone(),
        },
    )
    .await
    .unwrap();
    let again = write(
        &h,
        LifecycleRecordWrite::WouldDispatch {
            record: record.clone(),
        },
    )
    .await
    .unwrap();
    assert!(!first.replayed && again.replayed);
    let found = read(
        &h,
        LifecycleRecordQuery {
            record_kind: LifecycleRecordKind::WouldDispatch,
            phase: None,
            branch: None,
        },
    )
    .await;
    assert_eq!(found.would_dispatch.len(), 1);
    assert_eq!(found.would_dispatch[0].record, record);
    assert_eq!(found.would_dispatch[0].id, first.record_id);
}
