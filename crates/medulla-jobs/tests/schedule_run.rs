//! MEDULLA-PLACEMENT-R751.755, R751.756, R751.757, R751.760: the run policy of a schedule.

use medulla_jobs::schedule::{ScheduleRecord, ScheduleSpec, INVALID_SCHEDULE};
use medulla_jobs::schedule_run::{
    decide_window, usage_of, validate_group, RunUsage, ScheduleBudget, ScheduleEvent, ScheduleRun,
    ScheduleState, SkipReason, TickOutcome, WindowFacts, MAX_EXCLUSION_GROUP_CHARS,
    MAX_MIN_SPACING_S, SCHEDULE_MACHINE,
};
use medulla_types::transition_refused::TransitionRefusalReason;

const BUDGET: ScheduleBudget = ScheduleBudget {
    max_run_s: 10,
    max_tokens: 100,
    period_s: 60,
};

fn record() -> ScheduleRecord {
    ScheduleRecord {
        name: "nightly".into(),
        owner: "alice".into(),
        tenant: "t1".into(),
        spec: ScheduleSpec::OneShot { at_ms: 1 },
        task_kind: "association.refresh".into(),
        state: ScheduleState::Enabled,
        resumed_ms: 0,
        budget: BUDGET,
        exclusion_group: None,
        min_spacing_s: 0,
    }
}

fn facts(window_ms: u64) -> WindowFacts {
    WindowFacts {
        window_ms,
        live_from_ms: 0,
        own_run_active: false,
        group_run_active: false,
        group_last_fired_ms: None,
    }
}

fn skipped(reason: SkipReason) -> TickOutcome {
    TickOutcome::Skipped(reason)
}

// spec: MEDULLA-PLACEMENT-R751.755
#[test]
fn spent_run_seconds_or_tokens_skip_until_the_period_rolls() {
    let record = record();
    let mut run = ScheduleRun::default();
    run.roll_period(1_000_000, &BUDGET);
    assert_eq!(
        decide_window(&record, &run, &facts(1_000_000)),
        TickOutcome::Fired
    );

    // Run seconds spent: the next window skips with the budget reason.
    run.charge(RunUsage {
        run_s: 10,
        tokens: 0,
    });
    assert!(run.budget_spent(&BUDGET));
    assert_eq!(
        decide_window(&record, &run, &facts(1_010_000)),
        skipped(SkipReason::BudgetExhausted)
    );

    // Tokens spent alone also exhaust the budget.
    let mut tokens = ScheduleRun::default();
    tokens.roll_period(1_000_000, &BUDGET);
    tokens.charge(RunUsage {
        run_s: 1,
        tokens: 100,
    });
    assert_eq!(
        decide_window(&record, &tokens, &facts(1_010_000)),
        skipped(SkipReason::BudgetExhausted)
    );

    // A window inside the period stays skipped; the first window past it starts a new period.
    run.roll_period(1_059_999, &BUDGET);
    assert!(run.budget_spent(&BUDGET));
    run.roll_period(1_060_000, &BUDGET);
    assert_eq!(run.used_run_s, 0);
    assert_eq!(
        decide_window(&record, &run, &facts(1_060_000)),
        TickOutcome::Fired
    );

    // A run that reports no usage is charged its wall time in whole seconds, at least one.
    assert_eq!(usage_of(None, 0, 0).run_s, 1);
    assert_eq!(usage_of(None, 0, 2_001).run_s, 3);
    let reported = RunUsage {
        run_s: 4,
        tokens: 9,
    };
    assert_eq!(usage_of(Some(reported), 0, 90_000), reported);

    let zero = ScheduleBudget {
        max_run_s: 0,
        ..BUDGET
    };
    assert_eq!(zero.validate().unwrap_err().code, INVALID_SCHEDULE);
}

// spec: MEDULLA-PLACEMENT-R751.756
#[test]
fn pause_machine_has_two_legal_edges_and_refuses_every_other_pair() {
    assert_eq!(
        ScheduleState::Enabled.apply(ScheduleEvent::Pause).unwrap(),
        ScheduleState::Paused
    );
    assert_eq!(
        ScheduleState::Paused.apply(ScheduleEvent::Resume).unwrap(),
        ScheduleState::Enabled
    );
    for (state, event) in [
        (ScheduleState::Paused, ScheduleEvent::Pause),
        (ScheduleState::Enabled, ScheduleEvent::Resume),
    ] {
        let refused = state.apply(event).unwrap_err();
        assert_eq!(refused.machine, SCHEDULE_MACHINE.name);
        assert_eq!(refused.reason, TransitionRefusalReason::IllegalTransition);
        assert_eq!(ScheduleState::target(event), state);
    }
}

// spec: MEDULLA-PLACEMENT-R751.756
#[test]
fn paused_windows_and_windows_before_the_resume_skip() {
    let mut record = record();
    let run = ScheduleRun::default();
    record.state = ScheduleState::Paused;
    assert_eq!(
        decide_window(&record, &run, &facts(5_000)),
        skipped(SkipReason::Paused)
    );
    record.state = ScheduleState::Enabled;
    record.resumed_ms = 9_000;
    assert_eq!(
        decide_window(&record, &run, &facts(8_999)),
        skipped(SkipReason::Paused)
    );
    assert_eq!(
        decide_window(&record, &run, &facts(9_000)),
        TickOutcome::Fired
    );
}

// spec: MEDULLA-PLACEMENT-R751.757
#[test]
fn overlap_and_missed_windows_skip_and_nothing_is_queued() {
    let record = record();
    let run = ScheduleRun::default();
    let mut overlap = facts(5_000);
    overlap.own_run_active = true;
    assert_eq!(
        decide_window(&record, &run, &overlap),
        skipped(SkipReason::Overlap)
    );
    let mut missed = facts(5_000);
    missed.live_from_ms = 5_001;
    assert_eq!(
        decide_window(&record, &run, &missed),
        skipped(SkipReason::Missed)
    );
    // Every skip is a final outcome: it has a reason and carries no work item.
    for reason in [
        SkipReason::Paused,
        SkipReason::Missed,
        SkipReason::BudgetExhausted,
        SkipReason::Overlap,
        SkipReason::GroupActive,
        SkipReason::GroupSpacing,
    ] {
        assert_eq!(skipped(reason).as_str(), "skipped");
        assert_eq!(skipped(reason).reason(), reason.as_str());
    }
    assert_eq!(TickOutcome::Fired.reason(), "");
}

// spec: MEDULLA-PLACEMENT-R751.760
#[test]
fn group_members_skip_while_one_runs_and_inside_the_spacing() {
    let mut record = record();
    record.exclusion_group = Some("nightly-jobs".into());
    record.min_spacing_s = 30;
    let run = ScheduleRun::default();

    let mut active = facts(100_000);
    active.group_run_active = true;
    assert_eq!(
        decide_window(&record, &run, &active),
        skipped(SkipReason::GroupActive)
    );

    let mut spaced = facts(100_000);
    spaced.group_last_fired_ms = Some(80_000);
    assert_eq!(
        decide_window(&record, &run, &spaced),
        skipped(SkipReason::GroupSpacing)
    );
    spaced.window_ms = 110_000;
    assert_eq!(decide_window(&record, &run, &spaced), TickOutcome::Fired);

    // A schedule outside any group ignores group facts.
    let mut loner = record.clone();
    loner.exclusion_group = None;
    assert_eq!(decide_window(&loner, &run, &active), TickOutcome::Fired);

    // Group name and spacing bounds.
    assert!(validate_group(Some("g"), 0).is_ok());
    assert!(validate_group(None, MAX_MIN_SPACING_S).is_ok());
    for bad in [Some(""), Some("  ")] {
        assert_eq!(validate_group(bad, 0).unwrap_err().code, INVALID_SCHEDULE);
    }
    let long = "g".repeat(MAX_EXCLUSION_GROUP_CHARS + 1);
    assert_eq!(
        validate_group(Some(&long), 0).unwrap_err().code,
        INVALID_SCHEDULE
    );
    assert_eq!(
        validate_group(None, MAX_MIN_SPACING_S + 1)
            .unwrap_err()
            .code,
        INVALID_SCHEDULE
    );
    let mut bad = record;
    bad.exclusion_group = Some(String::new());
    assert_eq!(bad.validate().unwrap_err().code, INVALID_SCHEDULE);
}

// spec: MEDULLA-PLACEMENT-R751.757
#[test]
fn run_record_round_trips_through_node_fields() {
    let run = ScheduleRun {
        cursor_ms: Some(7),
        last_fired_ms: Some(5),
        active_job: Some("job-1".into()),
        period_start_ms: 3,
        used_run_s: 2,
        used_tokens: 1,
    };
    let fields = serde_json::Value::Object(run.to_fields("t1", "nightly"));
    assert_eq!(ScheduleRun::from_fields(Some(&fields)), run);
    assert_eq!(ScheduleRun::from_fields(None), ScheduleRun::default());
}
