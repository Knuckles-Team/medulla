//! Analytics job result bodies of the `coordination` domain.
//!
//! The engine's durable job record lives in `medulla-jobs`, above this crate. These are its
//! wire projections: a response decodes the record's JSON into them strictly
//! (`deny_unknown_fields`), so a field added to the record without its projection is a
//! refused response rather than an undeclared one. The executor payload
//! (`input_payload`) is durable implementation detail, handed only to the claiming
//! worker.

use serde::{Deserialize, Serialize};

use medulla_work_types::jobs::JobResult;

/// The graph snapshot a job reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct JobInputSnapshot {
    pub graph: String,
    pub version: u64,
    pub dataset_ref: String,
    pub content_digest: String,
}

/// A job's resource budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct JobResourceBudget {
    pub cpu_ms: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub io_bytes: Option<u64>,
    pub output_bytes: Option<u64>,
}

/// Where a job may run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct JobPlacement {
    pub pool: String,
    pub region: String,
    pub required_capabilities: Vec<String>,
}

/// The authority and limits a job runs under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct JobPolicy {
    pub tenant: String,
    pub actor: String,
    pub purpose: String,
    pub priority: i32,
    pub quota_cpu_ms: Option<u64>,
    pub deadline_unix_ms: Option<i64>,
    pub policy_fingerprint: String,
    pub resources: JobResourceBudget,
    pub placement: JobPlacement,
}

/// The full algorithm lineage a job's result is bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct JobAlgoVersion {
    pub family: String,
    pub algorithm: String,
    pub params_digest: String,
    pub code_version: String,
    pub env_version: String,
}

/// A job's retry policy and attempt counter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct JobRetryPolicy {
    pub max_attempts: u32,
    pub backoff_ms: u64,
    pub attempts_made: u32,
}

/// The last progress checkpoint a job wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct JobCheckpoint {
    pub progress: f64,
    pub stage: String,
    /// An opaque external state reference, as bytes.
    pub state_blob: Option<Vec<u8>>,
    pub updated_at_ms: i64,
}

/// A job's lifecycle state: one variant per stage of the analytics-job machine, carrying the
/// data its stage owns (the lease, the backoff time, the checkpoint).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum JobState {
    /// Waiting for a worker. A reclaimed job carries the checkpoint of its lapsed attempt, and
    /// the next claim resumes from it.
    Submitted {
        checkpoint: Option<JobCheckpoint>,
    },
    Running {
        lease: JobWorkerLease,
        checkpoint: JobCheckpoint,
    },
    /// The typed result is durable; its evidence-bearing claims are not yet.
    Publishing {
        lease: JobWorkerLease,
        result_ref: String,
        checkpoint: JobCheckpoint,
    },
    /// The lease holder is asked to stop.
    CancelRequested {
        lease: JobWorkerLease,
        checkpoint: JobCheckpoint,
    },
    /// A failed attempt waits until `not_before_ms` before it is claimed again.
    Backoff {
        not_before_ms: i64,
        checkpoint: Option<JobCheckpoint>,
    },
    Succeeded {
        result_ref: String,
        checkpoint: JobCheckpoint,
    },
    Failed {
        reason: String,
        checkpoint: Option<JobCheckpoint>,
    },
    Cancelled {
        checkpoint: Option<JobCheckpoint>,
    },
}

/// Fenced ownership of one job attempt by a worker slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct JobWorkerLease {
    pub worker_ref: String,
    pub epoch: u64,
    pub acquired_at_ms: i64,
    pub expires_at_ms: i64,
}

/// The durable analytics-job record, as a caller sees it. The lease, the cancel request and the
/// backoff time are part of `state`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AnalyticsJobRecord {
    pub job_id: String,
    pub input_snapshot: JobInputSnapshot,
    pub policy: JobPolicy,
    pub algo: JobAlgoVersion,
    pub retry: JobRetryPolicy,
    pub state: JobState,
    pub lease_epoch: u64,
    pub last_worker_ref: String,
    pub output: Option<JobResult>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// The job a worker claimed: its record plus the opaque executor payload the worker
/// runs, which only the claiming worker ever receives.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClaimedAnalyticsJob {
    #[serde(flatten)]
    pub record: AnalyticsJobRecord,
    pub input_payload: Option<Vec<u8>>,
}

/// `AnalyticsJob` op `WorkerClaim`: the claimed job and the lease fencing it.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WorkerJobClaim {
    pub job: ClaimedAnalyticsJob,
    pub lease: JobWorkerLease,
}

/// One owned schedule as served (MEDULLA-PLACEMENT-R751.754).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScheduleView {
    pub name: String,
    pub owner: String,
    pub tenant: String,
    pub spec: serde_json::Value,
    pub task_kind: String,
    /// The pause machine state (`ScheduleMachine`), read from the stored record.
    pub state: medulla_types::schedule_machine::ScheduleState,
    /// The instant of the last resume, 0 when never resumed.
    pub resumed_ms: u64,
    pub max_run_s: u64,
    pub max_tokens: u64,
    pub period_s: u64,
    pub exclusion_group: Option<String>,
    pub min_spacing_s: u64,
}

/// The schedules of one tenant, ordered by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScheduleViewList {
    pub schedules: Vec<ScheduleView>,
}

/// One recorded tick of a schedule (MEDULLA-PLACEMENT-R751.758).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScheduleTickView {
    /// The window the tick decided, in milliseconds.
    pub window_start_ms: u64,
    /// `fired`, `skipped` or `refused`.
    pub outcome: String,
    /// Why the window was not fired; empty for a fired one.
    pub reason: String,
    /// The engine that acted as leader for the tick.
    pub host: String,
    /// The work item the tick submitted; present only for a fired tick.
    pub work_item_id: Option<String>,
    /// Run seconds charged to the schedule's budget period when the tick was decided.
    pub used_run_s: u64,
    /// Tokens charged to the schedule's budget period when the tick was decided.
    pub used_tokens: u64,
}

/// The last ticks of one schedule, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScheduleTickList {
    pub schedule: String,
    pub ticks: Vec<ScheduleTickView>,
}

/// The name of a schedule that was removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScheduleRemoved {
    pub name: String,
}
