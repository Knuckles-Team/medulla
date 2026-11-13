//! MEDULLA-PLACEMENT-R751.762: the closed task-kind to job-spec table.

use medulla_jobs::schedule::{
    job_for_task_kind, ScheduleRecord, ScheduleSpec, INVALID_SCHEDULE, SCHEDULE_TASK_KINDS,
};
use medulla_jobs::schedule_run::{ScheduleBudget, ScheduleState};

fn record(task_kind: &str) -> ScheduleRecord {
    ScheduleRecord {
        name: "nightly".into(),
        owner: "alice".into(),
        tenant: "t1".into(),
        spec: ScheduleSpec::OneShot { at_ms: 1 },
        task_kind: task_kind.into(),
        state: ScheduleState::Enabled,
        resumed_ms: 0,
        budget: ScheduleBudget {
            max_run_s: 3_600,
            max_tokens: 1_000,
            period_s: 86_400,
        },
        exclusion_group: None,
        min_spacing_s: 0,
    }
}

// spec: MEDULLA-PLACEMENT-R751.762
#[test]
fn every_schedule_kind_has_one_job_spec_and_unknown_kinds_are_refused() {
    assert!(!SCHEDULE_TASK_KINDS.is_empty());
    for job in SCHEDULE_TASK_KINDS {
        let kinds = SCHEDULE_TASK_KINDS
            .iter()
            .filter(|other| other.task_kind == job.task_kind)
            .count();
        assert_eq!(kinds, 1, "{} appears once", job.task_kind);
        assert_eq!(job_for_task_kind(job.task_kind).unwrap(), job);
        assert!(record(job.task_kind).validate().is_ok());
    }
    let refused = record("sweep").validate().unwrap_err();
    assert_eq!(refused.code, INVALID_SCHEDULE);
    assert_eq!(
        job_for_task_kind("sweep").unwrap_err().code,
        INVALID_SCHEDULE
    );
}
