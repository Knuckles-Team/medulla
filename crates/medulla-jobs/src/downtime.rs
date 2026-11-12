//! Downtime work (MEDULLA-DURABLE-KERNEL-R101.1, R101.2): job types the engine runs
//! while it is idle or inside a declared window.
//!
//! Everything here is pure. [`select_for_window`] takes the window, the budget and
//! the foreground-activity reading as plain arguments, so the server crate owns
//! every read of the clock, the queue and the graph.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::ResourceBudget;

/// Most preconditions one job type may carry.
pub const MAX_PRECONDITIONS: usize = 8;
/// Most bytes one precondition may hold.
pub const MAX_PRECONDITION_BYTES: usize = 4096;

/// `knob downtime.idle_max_rps` default (requests per second), range [0, 100].
pub const IDLE_MAX_RPS_DEFAULT: u32 = 1;
/// `knob downtime.idle_quiet_secs` default (seconds), range [10, 3600].
pub const IDLE_QUIET_SECS_DEFAULT: u32 = 60;
/// `knob downtime.window_cpu_permille` default, range [10, 1000].
pub const WINDOW_CPU_PERMILLE_DEFAULT: u32 = 250;
/// `knob downtime.window_memory_bytes` default (2 GiB), range [256 MiB, 64 GiB].
pub const WINDOW_MEMORY_BYTES_DEFAULT: u64 = 2 * 1024 * 1024 * 1024;

/// How a window opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    /// Opens when the foreground has been quiet.
    Idle,
    /// Opens at its declared start time.
    Declared,
}

/// A span in which downtime types may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DowntimeWindow {
    pub kind: WindowKind,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// One downtime job type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DowntimeJobType {
    pub iri: String,
    /// Names a registered `JobIntent`.
    pub intent: String,
    /// A higher priority runs first.
    pub priority: u16,
    pub budget: ResourceBudget,
    pub preconditions: Vec<String>,
    pub window: WindowKind,
}

impl DowntimeJobType {
    /// Check the type against the model rules; `is_registered` answers whether an
    /// intent name is registered.
    pub fn validate(&self, is_registered: impl Fn(&str) -> bool) -> Result<(), String> {
        if self.iri.is_empty() {
            return Err("downtime job type has no iri".into());
        }
        if !is_registered(&self.intent) {
            return Err(format!(
                "downtime job type {} names unregistered intent {}",
                self.iri, self.intent
            ));
        }
        if self.budget.cpu_ms.is_none() || self.budget.memory_bytes.is_none() {
            return Err(format!(
                "downtime job type {} must set a budget with cpu_ms and memory_bytes",
                self.iri
            ));
        }
        if self.preconditions.len() > MAX_PRECONDITIONS {
            return Err(format!(
                "downtime job type {} has more than {MAX_PRECONDITIONS} preconditions",
                self.iri
            ));
        }
        if self
            .preconditions
            .iter()
            .any(|p| p.len() > MAX_PRECONDITION_BYTES)
        {
            return Err(format!(
                "downtime job type {} has a precondition over {MAX_PRECONDITION_BYTES} bytes",
                self.iri
            ));
        }
        Ok(())
    }
}

/// Evaluates one precondition query.
pub trait PreconditionEval {
    fn holds(&self, precondition: &str) -> bool;
}

/// The foreground-activity reading the idle window rule consumes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ForegroundActivity {
    pub queue_empty: bool,
    /// Highest request rate seen over the quiet period.
    pub peak_rps: f64,
    /// How long the rate has stayed at or under the ceiling.
    pub quiet_secs: u32,
}

/// Plain inputs of one selection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowContext {
    pub now_ms: i64,
    pub activity: ForegroundActivity,
    /// The window budget (`cpu_ms` and `memory_bytes` must be set).
    pub budget: ResourceBudget,
    pub idle_max_rps: u32,
    pub idle_quiet_secs: u32,
}

/// The types chosen for one window, in run order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DowntimeSelection {
    pub selected: Vec<String>,
    pub skipped_precondition: Vec<String>,
    pub skipped_budget: Vec<String>,
}

/// Whether the window is open at `ctx.now_ms`.
pub fn window_open(window: &DowntimeWindow, ctx: &WindowContext) -> bool {
    match window.kind {
        WindowKind::Declared => ctx.now_ms >= window.start_ms && ctx.now_ms < window.end_ms,
        WindowKind::Idle => {
            ctx.activity.queue_empty
                && ctx.activity.peak_rps <= f64::from(ctx.idle_max_rps)
                && ctx.activity.quiet_secs >= ctx.idle_quiet_secs
        }
    }
}

/// Rule `priority_then_budget_fit`: order eligible types by priority (high first),
/// then oldest last run, then IRI; take types while the summed budgets fit.
/// `last_run` maps an IRI to the millisecond of its last run; absent means never.
pub fn select_for_window(
    types: &[DowntimeJobType],
    window: &DowntimeWindow,
    last_run: &BTreeMap<String, i64>,
    eval: &dyn PreconditionEval,
    ctx: &WindowContext,
) -> DowntimeSelection {
    let mut out = DowntimeSelection::default();
    if !window_open(window, ctx) {
        return out;
    }
    let mut eligible: Vec<&DowntimeJobType> =
        types.iter().filter(|t| t.window == window.kind).collect();
    eligible.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| {
                let la = last_run.get(&a.iri).copied().unwrap_or(i64::MIN);
                let lb = last_run.get(&b.iri).copied().unwrap_or(i64::MIN);
                la.cmp(&lb)
            })
            .then_with(|| a.iri.cmp(&b.iri))
    });
    let cpu_cap = ctx.budget.cpu_ms.unwrap_or(0);
    let mem_cap = ctx.budget.memory_bytes.unwrap_or(0);
    let (mut cpu, mut mem) = (0u64, 0u64);
    for t in eligible {
        if !t.preconditions.iter().all(|p| eval.holds(p)) {
            out.skipped_precondition.push(t.iri.clone());
            continue;
        }
        let c = t.budget.cpu_ms.unwrap_or(u64::MAX);
        let m = t.budget.memory_bytes.unwrap_or(u64::MAX);
        match (cpu.checked_add(c), mem.checked_add(m)) {
            (Some(nc), Some(nm)) if nc <= cpu_cap && nm <= mem_cap => {
                cpu = nc;
                mem = nm;
                out.selected.push(t.iri.clone());
            }
            _ => out.skipped_budget.push(t.iri.clone()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jt(iri: &str, priority: u16, pre: &[&str]) -> DowntimeJobType {
        DowntimeJobType {
            iri: iri.into(),
            intent: "plan_evolution".into(),
            priority,
            budget: ResourceBudget {
                cpu_ms: Some(100),
                memory_bytes: Some(1 << 20),
                ..Default::default()
            },
            preconditions: pre.iter().map(|s| s.to_string()).collect(),
            window: WindowKind::Idle,
        }
    }

    struct Eval;
    impl PreconditionEval for Eval {
        fn holds(&self, p: &str) -> bool {
            p != "ASK { false }"
        }
    }

    fn ctx(cpu: u64) -> WindowContext {
        WindowContext {
            now_ms: 1_000,
            activity: ForegroundActivity {
                queue_empty: true,
                peak_rps: 0.0,
                quiet_secs: 120,
            },
            budget: ResourceBudget {
                cpu_ms: Some(cpu),
                memory_bytes: Some(1 << 30),
                ..Default::default()
            },
            idle_max_rps: IDLE_MAX_RPS_DEFAULT,
            idle_quiet_secs: IDLE_QUIET_SECS_DEFAULT,
        }
    }

    const IDLE: DowntimeWindow = DowntimeWindow {
        kind: WindowKind::Idle,
        start_ms: 0,
        end_ms: 0,
    };

    #[test]
    fn job_type_round_trips() {
        let t = jt("dw:a", 7, &["ASK { true }"]);
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<DowntimeJobType>(&json).unwrap(), t);
        let w = DowntimeWindow {
            kind: WindowKind::Declared,
            start_ms: 5,
            end_ms: 9,
        };
        let json = serde_json::to_string(&w).unwrap();
        assert_eq!(serde_json::from_str::<DowntimeWindow>(&json).unwrap(), w);
        assert!(t.validate(|i| i == "plan_evolution").is_ok());
        assert!(t.validate(|_| false).is_err());
        let mut no_budget = t.clone();
        no_budget.budget = ResourceBudget::default();
        assert!(no_budget.validate(|_| true).is_err());
        let mut many = t.clone();
        many.preconditions = vec!["x".into(); MAX_PRECONDITIONS + 1];
        assert!(many.validate(|_| true).is_err());
        let mut big = t;
        big.preconditions = vec!["x".repeat(MAX_PRECONDITION_BYTES + 1)];
        assert!(big.validate(|_| true).is_err());
    }

    #[test]
    fn priority_then_budget_fit() {
        let types = vec![
            jt("dw:low", 1, &[]),
            jt("dw:high", 9, &[]),
            jt("dw:skip", 5, &["ASK { false }"]),
            jt("dw:mid", 5, &[]),
        ];
        let sel = select_for_window(&types, &IDLE, &BTreeMap::new(), &Eval, &ctx(1_000));
        assert_eq!(sel.selected, ["dw:high", "dw:mid", "dw:low"]);
        assert_eq!(sel.skipped_precondition, ["dw:skip"]);
        // Budget fits two of three.
        let sel = select_for_window(&types, &IDLE, &BTreeMap::new(), &Eval, &ctx(200));
        assert_eq!(sel.selected, ["dw:high", "dw:mid"]);
        assert_eq!(sel.skipped_budget, ["dw:low"]);
        // Equal priority: oldest last run first, then IRI.
        let tie = vec![jt("dw:b", 3, &[]), jt("dw:a", 3, &[])];
        let mut last = BTreeMap::new();
        last.insert("dw:a".to_string(), 50);
        last.insert("dw:b".to_string(), 10);
        let sel = select_for_window(&tie, &IDLE, &last, &Eval, &ctx(1_000));
        assert_eq!(sel.selected, ["dw:b", "dw:a"]);
        // A busy foreground keeps the idle window closed.
        let mut busy = ctx(1_000);
        busy.activity.queue_empty = false;
        assert!(
            select_for_window(&types, &IDLE, &BTreeMap::new(), &Eval, &busy)
                .selected
                .is_empty()
        );
    }
}
