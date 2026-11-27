//! MEDULLA-OBSERVE-R008.1: step records.

use medulla_observe::startup::{StartupReport, StepOutcome, STARTUP_STEP_ABANDONED};

// spec: MEDULLA-OBSERVE-R008.1
// Proves a step guard records its duration and its outcome, and an abandoned guard records a failure.
#[test]
fn step_guard_records_duration_and_outcome() {
    let report = StartupReport::new();
    {
        let step = report.step("store-migration", "");
        std::thread::sleep(std::time::Duration::from_millis(5));
        step.ok();
    }
    report.step("index-rebuild", "alpha").skipped("no index declared");
    report.step("graph-materialization", "beta").refused("MIGRATION_REFUSED");
    report.step("matview-reload", "beta").failed("MATVIEW_RELOAD_FAILED");
    {
        // A guard dropped without a verdict is a failure, never a success.
        let _abandoned = report.step("ts-reconcile", "gamma");
    }
    let steps = report.steps();
    assert_eq!(steps.len(), 5);
    let by_name = |name: &str| steps.iter().find(|step| step.name == name).unwrap();
    let migration = by_name("store-migration");
    assert_eq!(migration.outcome, StepOutcome::Ok);
    assert!(migration.duration_ms >= 5, "the duration spans the guard");
    assert!(migration.started_unix_ms > 0);
    assert_eq!(by_name("index-rebuild").graph, "alpha");
    assert_eq!(
        by_name("index-rebuild").outcome,
        StepOutcome::Skipped("no index declared".into())
    );
    assert_eq!(
        by_name("graph-materialization").outcome,
        StepOutcome::Refused("MIGRATION_REFUSED".into())
    );
    assert_eq!(by_name("matview-reload").outcome.word(), "Failed");
    assert_eq!(
        by_name("ts-reconcile").outcome,
        StepOutcome::Failed(STARTUP_STEP_ABANDONED.into())
    );
}
