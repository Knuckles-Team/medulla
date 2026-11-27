//! Startup step records (MEDULLA-OBSERVE-R008.1).
//!
//! [`StartupReport::step`] opens one boot or maintenance step and returns a [`StepGuard`].
//! The guard records one [`StartupStep`] when it drops: the name, the graph the step ran
//! for, when it started, how long it took and how it ended. A guard dropped without a
//! verdict records `Failed(STARTUP_STEP_ABANDONED)`, so an early return reads as a failure
//! and never as a success. Each record also logs at INFO with target `medulla::startup`.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// The code a guard records when it drops without a verdict.
pub const STARTUP_STEP_ABANDONED: &str = "STARTUP_STEP_ABANDONED";

/// How a step ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepOutcome {
    /// The step ran to its end.
    Ok,
    /// The step had nothing to do, for the stated reason.
    Skipped(String),
    /// The step was refused with a declared code.
    Refused(String),
    /// The step failed with a declared code.
    Failed(String),
}

impl StepOutcome {
    /// The outcome word: `Ok`, `Skipped`, `Refused` or `Failed`.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Ok => "Ok",
            Self::Skipped(_) => "Skipped",
            Self::Refused(_) => "Refused",
            Self::Failed(_) => "Failed",
        }
    }

    /// The reason or code the outcome carries.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Ok => None,
            Self::Skipped(value) | Self::Refused(value) | Self::Failed(value) => Some(value),
        }
    }
}

/// One recorded step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupStep {
    /// The step name, for example `store-migration`.
    pub name: String,
    /// The graph the step ran for; empty for a step that spans the store.
    pub graph: String,
    /// Unix milliseconds when the step started.
    pub started_unix_ms: u64,
    /// How long the step took, in milliseconds.
    pub duration_ms: u64,
    /// How the step ended.
    pub outcome: StepOutcome,
}

/// One step of a stored boot report (MEDULLA-OBSERVE-R008.2): the outcome as its word and its
/// reason or code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredStep {
    /// The step name.
    pub name: String,
    /// The graph the step ran for; empty for a step that spans the store.
    pub graph: String,
    /// Unix milliseconds when the step started.
    pub started_unix_ms: u64,
    /// How long the step took, in milliseconds.
    pub duration_ms: u64,
    /// `Ok`, `Skipped`, `Refused` or `Failed`.
    pub outcome: String,
    /// The reason or code the outcome carries.
    pub detail: Option<String>,
}

/// The report of one boot as the control store keeps it: the boot sequence number, when the boot
/// began and every step it recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredBoot {
    /// The boot sequence number: one more than the highest stored before this boot.
    pub boot_seq: u64,
    /// Unix milliseconds when this report began.
    pub started_unix_ms: u64,
    /// The steps, ascending by start time.
    pub steps: Vec<StoredStep>,
}

/// The steps of one boot, in the order they finished, and the reports of the boots before it.
pub struct StartupReport {
    steps: Mutex<Vec<StartupStep>>,
    boot_seq: std::sync::atomic::AtomicU64,
    started_unix_ms: u64,
    previous: Mutex<Vec<StoredBoot>>,
}

impl Default for StartupReport {
    fn default() -> Self {
        Self {
            steps: Mutex::new(Vec::new()),
            boot_seq: std::sync::atomic::AtomicU64::new(1),
            started_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as u64),
            previous: Mutex::new(Vec::new()),
        }
    }
}

impl StartupReport {
    /// An empty report. Tests build isolated instances; the process uses
    /// [`startup`](crate::startup()).
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a step. It is recorded when the returned guard drops.
    pub fn step(&self, name: &str, graph: &str) -> StepGuard<'_> {
        StepGuard {
            report: self,
            name: name.to_string(),
            graph: graph.to_string(),
            started_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis() as u64)
                .unwrap_or(0),
            started: Instant::now(),
            outcome: None,
        }
    }

    /// Load the reports of the boots before this one, oldest first (MEDULLA-OBSERVE-R008.2).
    /// The boot sequence number becomes one more than the highest of them.
    pub fn load_previous(&self, mut previous: Vec<StoredBoot>) {
        previous.sort_by_key(|boot| boot.boot_seq);
        let next = previous.last().map_or(1, |boot| boot.boot_seq + 1);
        self.boot_seq.store(next, std::sync::atomic::Ordering::Relaxed);
        *self.previous.lock() = previous;
    }

    /// The reports of the boots before this one, oldest first.
    pub fn previous(&self) -> Vec<StoredBoot> {
        self.previous.lock().clone()
    }

    /// The sequence number of this boot.
    pub fn boot_seq(&self) -> u64 {
        self.boot_seq.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The report of this boot as the control store keeps it: what `Introspect startup` calls
    /// the current report once the boot is over (MEDULLA-OBSERVE-R008.2).
    pub fn finish(&self) -> StoredBoot {
        StoredBoot {
            boot_seq: self.boot_seq(),
            started_unix_ms: self.started_unix_ms,
            steps: self
                .steps()
                .into_iter()
                .map(|step| StoredStep {
                    name: step.name,
                    graph: step.graph,
                    started_unix_ms: step.started_unix_ms,
                    duration_ms: step.duration_ms,
                    outcome: step.outcome.word().to_string(),
                    detail: step.outcome.detail().map(str::to_string),
                })
                .collect(),
        }
    }

    /// The recorded steps, ascending by start time and then by name.
    pub fn steps(&self) -> Vec<StartupStep> {
        let mut steps = self.steps.lock().clone();
        steps.sort_by(|a, b| {
            (a.started_unix_ms, &a.name, &a.graph).cmp(&(b.started_unix_ms, &b.name, &b.graph))
        });
        steps
    }
}

/// An open step. It records itself on drop.
pub struct StepGuard<'a> {
    report: &'a StartupReport,
    name: String,
    graph: String,
    started_unix_ms: u64,
    started: Instant,
    outcome: Option<StepOutcome>,
}

impl StepGuard<'_> {
    /// End the step successfully.
    pub fn ok(mut self) {
        self.outcome = Some(StepOutcome::Ok);
    }

    /// End the step because there was nothing to do.
    pub fn skipped(mut self, reason: &str) {
        self.outcome = Some(StepOutcome::Skipped(reason.to_string()));
    }

    /// End the step refused with a declared code.
    pub fn refused(mut self, code: &str) {
        self.outcome = Some(StepOutcome::Refused(code.to_string()));
    }

    /// End the step failed with a declared code.
    pub fn failed(mut self, code: &str) {
        self.outcome = Some(StepOutcome::Failed(code.to_string()));
    }
}

impl Drop for StepGuard<'_> {
    fn drop(&mut self) {
        let outcome = self
            .outcome
            .take()
            .unwrap_or_else(|| StepOutcome::Failed(STARTUP_STEP_ABANDONED.to_string()));
        let step = StartupStep {
            name: std::mem::take(&mut self.name),
            graph: std::mem::take(&mut self.graph),
            started_unix_ms: self.started_unix_ms,
            duration_ms: self.started.elapsed().as_millis() as u64,
            outcome,
        };
        tracing::info!(
            target: "medulla::startup",
            step = %step.name,
            graph = %step.graph,
            duration_ms = step.duration_ms,
            outcome = step.outcome.word(),
            detail = step.outcome.detail().unwrap_or(""),
            "startup step"
        );
        self.report.steps.lock().push(step);
    }
}
