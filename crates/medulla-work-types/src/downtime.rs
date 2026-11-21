//! The wire types of the served `DowntimeSelections` method
//! (MEDULLA-DURABLE-KERNEL-R101.3, R101.5).

use serde::{Deserialize, Serialize};

/// One downtime job type a tick submitted a job for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DowntimeSelected {
    /// The IRI of the selected `dw:JobType`.
    pub iri: String,
    /// The intent whose job the tick submitted.
    pub intent: String,
    /// The id of the submitted job.
    pub job_id: String,
    /// The job as it stands when the selection is read (MEDULLA-DURABLE-KERNEL-R101.5 4a).
    /// Filled at read time and never stored on the selection node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<DowntimeJobObservation>,
}

/// The stored state of a downtime job, one name per `JobState` variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DowntimeJobState {
    /// Queued for a worker.
    Submitted,
    /// A worker holds the lease.
    Running,
    /// The typed result is durable and its claims are committing.
    Publishing,
    /// The lease holder is asked to stop.
    CancelRequested,
    /// A failed attempt waits to be claimed again.
    Backoff,
    /// The job finished.
    Succeeded,
    /// The job failed for good.
    Failed,
    /// The job was cancelled.
    Cancelled,
}

/// What a read of a selection shows of one chosen job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DowntimeJobObservation {
    /// The job's state.
    pub state: DowntimeJobState,
    /// Attempts made so far.
    pub attempts_made: u32,
    /// The stage label of the job's checkpoint, when it has one.
    pub checkpoint_stage: Option<String>,
    /// Whether the job holds a checkpoint: a `Submitted` job that has one was left by a
    /// preempted step and resumes from it (MEDULLA-DURABLE-KERNEL-R101.6).
    pub has_checkpoint: bool,
    /// Whether a worker lease is held (`Running`, `Publishing` or `CancelRequested`).
    pub has_lease: bool,
    /// CPU milliseconds the job used, once it has a result.
    pub cpu_ms: Option<u64>,
}

/// Why a tick did not submit a job for a type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DowntimeSkipReason {
    /// A precondition did not hold.
    PreconditionFalse,
    /// The type's budget did not fit what was left of the window.
    BudgetDoesNotFit,
    /// No background capacity lease was granted.
    LeaseNotGranted,
    /// The type declares no budget.
    BudgetRequired,
}

impl DowntimeSkipReason {
    /// The snake-case name stored on a selection entry and sent on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreconditionFalse => "precondition_false",
            Self::BudgetDoesNotFit => "budget_does_not_fit",
            Self::LeaseNotGranted => "lease_not_granted",
            Self::BudgetRequired => "budget_required",
        }
    }

    /// The reason a stored name stands for.
    pub fn from_name(name: &str) -> Option<Self> {
        [
            Self::PreconditionFalse,
            Self::BudgetDoesNotFit,
            Self::LeaseNotGranted,
            Self::BudgetRequired,
        ]
        .into_iter()
        .find(|reason| reason.as_str() == name)
    }
}

/// One downtime job type a tick skipped, with the reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DowntimeSkipped {
    /// The IRI of the skipped `dw:JobType`.
    pub iri: String,
    /// Why it was skipped.
    pub reason: DowntimeSkipReason,
}

/// What one deciding downtime tick chose and skipped.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DowntimeSelectionRecord {
    /// The id of the `dw:Selection` node; ascending id order is newest first.
    pub selection_id: String,
    /// Engine time of the tick, in milliseconds.
    pub at_ms: i64,
    /// The types a job was submitted for, in run order.
    pub chosen: Vec<DowntimeSelected>,
    /// The types skipped, in record order.
    pub skipped: Vec<DowntimeSkipped>,
}

/// One page of the recorded selections, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DowntimeSelections {
    /// The selections of the page.
    pub selections: Vec<DowntimeSelectionRecord>,
    /// The cursor of the next page; absent on the last page.
    pub next_after: Option<String>,
}
