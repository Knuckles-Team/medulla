//! Run policy of an owned schedule: pause state machine, budget, skip rules and exclusion
//! groups (MEDULLA-PLACEMENT-R751.755, R751.756, R751.757, R751.760).
//!
//! [`decide_window`] sits beside `ScheduleSpec::due_windows`: for one due window it returns
//! either `Fired` or `Skipped(reason)`. A skipped window submits nothing and is never queued,
//! so the engine holds no backlog of scheduled jobs. [`ScheduleRun`] is the per-schedule run
//! record the tick task keeps in the tenant graph (cursor, last fired window, active job,
//! budget usage of the current period). [`ScheduleState`] is the explicit pause machine; its
//! transition function [`ScheduleState::apply`] is the only writer of the state.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};

use crate::model::AnalyticsJob;
use crate::schedule::{ScheduleError, ScheduleRecord, INVALID_SCHEDULE};

/// Node type of the per-schedule run record.
pub const SCHEDULE_RUN_NODE_TYPE: &str = "ScheduleRun";
/// Longest exclusion group name, in characters.
pub const MAX_EXCLUSION_GROUP_CHARS: usize = 64;
/// Largest minimum spacing between group runs, in seconds (seven days).
pub const MAX_MIN_SPACING_S: u64 = 604_800;

/// The closed state set of the schedule machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleState {
    #[default]
    Enabled,
    Paused,
}

impl MachineState for ScheduleState {
    fn stored(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Paused => "paused",
        }
    }
}

/// The closed event set of the schedule machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduleEvent {
    Pause,
    Resume,
}

impl MachineEvent for ScheduleEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
        }
    }
}

/// The schedule machine has no guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduleGuard {}

/// The schedule machine: `Enabled` start, no terminal state. Pausing a paused schedule and
/// resuming an enabled one are illegal transitions; the served methods answer them
/// idempotently before calling the machine.
pub const SCHEDULE_MACHINE: MachineDef<ScheduleState, ScheduleEvent, ScheduleGuard> = MachineDef {
    name: "ScheduleMachine",
    states: &[ScheduleState::Enabled, ScheduleState::Paused],
    initial: ScheduleState::Enabled,
    terminals: &[],
    events: &[ScheduleEvent::Pause, ScheduleEvent::Resume],
    guards: &[],
    edges: &[
        Edge {
            id: "S01",
            from: ScheduleState::Enabled,
            event: ScheduleEvent::Pause,
            guard: None,
            to: ScheduleState::Paused,
        },
        Edge {
            id: "S02",
            from: ScheduleState::Paused,
            event: ScheduleEvent::Resume,
            guard: None,
            to: ScheduleState::Enabled,
        },
    ],
};

impl ScheduleState {
    /// The one transition function: the next state, or the typed refusal.
    pub fn apply(self, event: ScheduleEvent) -> Result<Self, TransitionRefused> {
        transition(&SCHEDULE_MACHINE, self, event, |guard| match guard {}).map(|edge| edge.to)
    }

    /// The state a pause or resume leaves the schedule in.
    pub fn target(event: ScheduleEvent) -> Self {
        match event {
            ScheduleEvent::Pause => Self::Paused,
            ScheduleEvent::Resume => Self::Enabled,
        }
    }
}

/// The budget of one schedule: most run seconds and most tokens its runs may use per budget
/// period of `period_s` seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleBudget {
    pub max_run_s: u64,
    pub max_tokens: u64,
    pub period_s: u64,
}

impl ScheduleBudget {
    /// Refuse a budget with a zero limit or period.
    pub fn validate(&self) -> Result<(), ScheduleError> {
        if self.max_run_s == 0 || self.max_tokens == 0 || self.period_s == 0 {
            return Err(ScheduleError::new(
                INVALID_SCHEDULE,
                "budget run seconds, tokens and period must be positive",
            ));
        }
        Ok(())
    }
}

/// What one finished run used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunUsage {
    pub run_s: u64,
    pub tokens: u64,
}

/// The usage a run reports through its completed work item: the usage the result carries, or
/// the wall time between submit and completion charged in whole seconds, at least one.
pub fn usage_of(reported: Option<RunUsage>, created_ms: i64, finished_ms: i64) -> RunUsage {
    reported.unwrap_or_else(|| {
        let wall_ms = u64::try_from(finished_ms.saturating_sub(created_ms)).unwrap_or(0);
        RunUsage {
            run_s: wall_ms.div_ceil(1_000).max(1),
            tokens: 0,
        }
    })
}

/// The usage of a work item that has finished, `None` while it is still active.
pub fn finished_usage(job: &AnalyticsJob) -> Option<RunUsage> {
    job.state.is_terminal().then(|| {
        let reported = job
            .output
            .as_ref()
            .and_then(|output| output.usage)
            .map(|usage| RunUsage {
                run_s: usage.run_s,
                tokens: usage.tokens,
            });
        usage_of(reported, job.created_at_ms, job.updated_at_ms)
    })
}

/// The per-schedule run record kept in the tenant graph by the tick task.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleRun {
    /// The last window evaluated; the next pass starts after it.
    pub cursor_ms: Option<u64>,
    /// The last window that fired a job.
    pub last_fired_ms: Option<u64>,
    /// The work item of the last fired window until it has finished and been charged.
    pub active_job: Option<String>,
    /// Start of the current budget period.
    pub period_start_ms: u64,
    pub used_run_s: u64,
    pub used_tokens: u64,
}

impl ScheduleRun {
    /// The graph node id of a schedule's run record.
    pub fn node_id(tenant: &str, name: &str) -> String {
        format!("schedule-run:{tenant}:{name}")
    }

    /// Start a new budget period when `window_ms` is past the current one.
    pub fn roll_period(&mut self, window_ms: u64, budget: &ScheduleBudget) {
        let period_end = self
            .period_start_ms
            .saturating_add(budget.period_s.saturating_mul(1_000));
        if window_ms >= period_end {
            self.period_start_ms = window_ms;
            self.used_run_s = 0;
            self.used_tokens = 0;
        }
    }

    /// Charge a finished run and release the active work item.
    pub fn charge(&mut self, usage: RunUsage) {
        self.used_run_s = self.used_run_s.saturating_add(usage.run_s);
        self.used_tokens = self.used_tokens.saturating_add(usage.tokens);
        self.active_job = None;
    }

    /// Whether this period's usage has reached either limit.
    pub fn budget_spent(&self, budget: &ScheduleBudget) -> bool {
        self.used_run_s >= budget.max_run_s || self.used_tokens >= budget.max_tokens
    }

    /// The stored node fields.
    pub fn to_fields(
        &self,
        tenant: &str,
        name: &str,
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut fields = match serde_json::to_value(self) {
            Ok(serde_json::Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        };
        fields.insert("type".into(), SCHEDULE_RUN_NODE_TYPE.into());
        fields.insert("tenant".into(), tenant.into());
        fields.insert("schedule".into(), name.into());
        fields
    }

    /// Read a stored record; a missing record is the initial one.
    pub fn from_fields(fields: Option<&serde_json::Value>) -> Self {
        fields
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default()
    }
}

/// Why a due window did not fire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    Paused,
    Missed,
    BudgetExhausted,
    Overlap,
    GroupActive,
    GroupSpacing,
}

impl SkipReason {
    /// The stored reason text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Paused => "paused",
            Self::Missed => "missed",
            Self::BudgetExhausted => "budget_exhausted",
            Self::Overlap => "overlap",
            Self::GroupActive => "group_active",
            Self::GroupSpacing => "group_spacing",
        }
    }
}

/// The outcome of one due window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickOutcome {
    Fired,
    Skipped(SkipReason),
}

impl TickOutcome {
    /// The stored outcome text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fired => "fired",
            Self::Skipped(_) => "skipped",
        }
    }

    /// The stored reason text; empty when the window fired.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Fired => "",
            Self::Skipped(reason) => reason.as_str(),
        }
    }
}

/// What the tick task knows about one due window when it decides.
#[derive(Clone, Copy, Debug)]
pub struct WindowFacts {
    pub window_ms: u64,
    /// Windows before this instant came due while no leader was acting.
    pub live_from_ms: u64,
    /// The schedule's own last work item is still active.
    pub own_run_active: bool,
    /// Another member of the schedule's exclusion group has an active work item.
    pub group_run_active: bool,
    /// The newest fired window of any member of the schedule's exclusion group.
    pub group_last_fired_ms: Option<u64>,
}

/// Decide one due window. The checks run in a fixed order and the first that holds wins:
/// paused (including every window before the last resume), missed, budget, overlap, group
/// active, group spacing. Only when none holds does the window fire. `run` must already be
/// rolled to the window's budget period and charged for finished work.
pub fn decide_window(
    record: &ScheduleRecord,
    run: &ScheduleRun,
    facts: &WindowFacts,
) -> TickOutcome {
    let skip = TickOutcome::Skipped;
    if record.state == ScheduleState::Paused || facts.window_ms < record.resumed_ms {
        return skip(SkipReason::Paused);
    }
    if facts.window_ms < facts.live_from_ms {
        return skip(SkipReason::Missed);
    }
    if run.budget_spent(&record.budget) {
        return skip(SkipReason::BudgetExhausted);
    }
    if facts.own_run_active {
        return skip(SkipReason::Overlap);
    }
    if record.exclusion_group.is_some() {
        if facts.group_run_active {
            return skip(SkipReason::GroupActive);
        }
        let spacing_ms = record.min_spacing_s.saturating_mul(1_000);
        let recent = facts
            .group_last_fired_ms
            .is_some_and(|last| facts.window_ms < last.saturating_add(spacing_ms));
        if recent {
            return skip(SkipReason::GroupSpacing);
        }
    }
    TickOutcome::Fired
}

/// Refuse an empty or over-long exclusion group name and an out-of-range spacing.
pub fn validate_group(group: Option<&str>, min_spacing_s: u64) -> Result<(), ScheduleError> {
    if let Some(group) = group {
        let length = group.chars().count();
        if group.trim().is_empty() || length > MAX_EXCLUSION_GROUP_CHARS {
            return Err(ScheduleError::new(
                INVALID_SCHEDULE,
                format!("exclusion group must be 1 to {MAX_EXCLUSION_GROUP_CHARS} characters"),
            ));
        }
    }
    if min_spacing_s > MAX_MIN_SPACING_S {
        return Err(ScheduleError::new(
            INVALID_SCHEDULE,
            format!("min_spacing_s must be at most {MAX_MIN_SPACING_S}"),
        ));
    }
    Ok(())
}
