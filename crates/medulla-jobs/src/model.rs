//! The `AnalyticsJob` record + its state machine (CONCEPT:INT-P2-1).
//!
//! A durable, resumable analytics run. Everything a caller needs to reason about
//! WITHOUT re-reading the underlying data: who ran it and why (tenant/actor/
//! purpose), what it ran against (an immutable snapshot HANDLE — a graph name +
//! OCC version, never a copy of rows), what it ran (algorithm + params + code/env
//! versions), how it is governed (policy/quota/priority/deadline/retry), and where
//! it is in its lifecycle (state + checkpoint/progress).

use crate::job_machine::{JobFacts, JobStage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Opaque job identifier. Consensus submissions derive it from their immutable
/// MutationBatch identity; direct crate-local submissions use a monotonic id.
pub type JobId = String;

/// An IMMUTABLE handle to the input a job ran against: a graph name plus the OCC
/// `GraphCore::version()` pinned at submit time. NOT a copy of rows — re-reading the
/// same `(graph, version)` later (via an `AS OF` query) reproduces the exact input a
/// job saw, the same way the engine's own bitemporal `AS OF` queries do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct InputSnapshotHandle {
    pub graph: String,
    pub version: u64,
    /// Opaque immutable dataset identity. It is not a path, query, or caller label.
    #[serde(default)]
    pub dataset_ref: String,
    /// Digest of the actual input records, closing result-ref collisions between
    /// different datasets read at the same graph version.
    #[serde(default)]
    pub content_digest: String,
}

impl InputSnapshotHandle {
    pub fn new(graph: impl Into<String>, version: u64) -> Self {
        Self {
            graph: graph.into(),
            version,
            dataset_ref: String::new(),
            content_digest: String::new(),
        }
    }

    pub fn with_dataset(
        mut self,
        dataset_ref: impl Into<String>,
        content_digest: impl Into<String>,
    ) -> Self {
        self.dataset_ref = dataset_ref.into();
        self.content_digest = content_digest.into();
        self
    }
}

/// Hard resource envelope enforced by the worker scheduler and executor.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ResourceBudget {
    pub cpu_ms: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub io_bytes: Option<u64>,
    pub output_bytes: Option<u64>,
}

/// Placement constraints used by a distributed worker pool.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobPlacement {
    pub pool: String,
    pub region: String,
    pub required_capabilities: Vec<String>,
}

/// Algorithm + reproducibility lineage: WHAT ran, with WHICH knobs, on WHICH build.
///
/// `params_digest` is a caller-supplied stable digest of the algorithm's own
/// parameters (e.g. `sha256(canonical_json(params))`) rather than the params
/// themselves, so this struct stays small and hashable regardless of how large a
/// given family's parameter set is. Two jobs with byte-identical
/// `(family, algorithm, params_digest, code_version, env_version)` over the SAME
/// `InputSnapshotHandle` are, by construction, the same computation — see
/// [`AnalyticsJob::result_ref`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct AlgoVersion {
    /// Compute family, e.g. `"mining.association"`, `"mining.cluster"`.
    pub family: String,
    /// The specific algorithm within the family, e.g. `"fpgrowth"`.
    pub algorithm: String,
    /// Stable digest of the algorithm's own parameters (hex).
    pub params_digest: String,
    /// The engine build that ran it (e.g. `CARGO_PKG_VERSION`, or a git sha).
    pub code_version: String,
    /// The runtime/feature-set fingerprint the algorithm ran under.
    pub env_version: String,
}

/// Tenancy/governance metadata carried on every job (CONCEPT:INT-P2-1).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JobPolicy {
    pub tenant: String,
    pub actor: String,
    pub purpose: String,
    /// Relative scheduling priority; higher runs first. Advisory — the reference
    /// runner in this crate does not itself schedule, a caller-supplied executor
    /// (e.g. the facade's async task pool) is expected to honor it.
    pub priority: i32,
    /// Soft CPU-time budget in milliseconds, if the caller wants one enforced.
    pub quota_cpu_ms: Option<u64>,
    /// Wall-clock deadline (Unix ms). A job whose deadline has passed while still
    /// `Running` is a candidate for the caller's own deadline-sweep — this crate
    /// only stores the field; see [`AnalyticsJob::deadline_exceeded`].
    pub deadline_unix_ms: Option<i64>,
    /// Digest of the authorization decision used when the input was admitted.
    #[serde(default)]
    pub policy_fingerprint: String,
    #[serde(default)]
    pub resources: ResourceBudget,
    #[serde(default)]
    pub placement: JobPlacement,
}

/// Renewable, fenced ownership of one durable job attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerLease {
    pub worker_ref: String,
    pub epoch: u64,
    pub acquired_at_ms: i64,
    pub expires_at_ms: i64,
}

/// Retry policy + counter for a job's failure path.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of run attempts (including the first). `0` or `1` ⇒ no retry.
    pub max_attempts: u32,
    /// Backoff between attempts, in milliseconds (advisory; the executor sleeps).
    pub backoff_ms: u64,
    /// Attempts made so far (starts at `0`, incremented by [`crate::store::JobStore::fail`]).
    pub attempts_made: u32,
}

impl RetryPolicy {
    /// Whether another attempt is allowed after the current one failed.
    pub fn retries_remaining(&self) -> bool {
        self.attempts_made < self.max_attempts.max(1)
    }
}

/// Resumable progress marker (CONCEPT:INT-P2-1): a monotonic `[0,1]` fraction, a
/// free-form stage label, and an OPAQUE resumable-state blob the job kind defines
/// the shape of (this crate never interprets it). Persisted with the job record, so
/// a restart reads back exactly the last checkpoint a running job wrote.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub progress: f64,
    pub stage: String,
    #[serde(default)]
    pub state_blob: Option<Vec<u8>>,
    pub updated_at_ms: i64,
}

/// The job lifecycle (CONCEPT:INT-P2-1) as the payload of `AnalyticsJobMachine`
/// (MEDULLA-CQ-ORCH-R811.1): exactly one variant per [`JobStage`], and the data its stage owns.
/// The lease belongs to `Running`, `Publishing` and `CancelRequested`; `not_before_ms` belongs
/// to `Backoff`; no flag beside the state decides an edge.
/// `Submitted -> Running -> Publishing(typed result) -> Succeeded`, with a failed attempt
/// waiting in `Backoff` before it is claimed again, `CancelRequested` asking the lease holder
/// to stop, and expired leases returning `Running`/`Publishing` work to `Submitted` under a
/// higher fencing epoch. `Succeeded` is impossible until both the typed result and its
/// evidence-bearing claim batch are durable. A `Failed` job never revives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum JobState {
    Submitted,
    Running {
        lease: WorkerLease,
        checkpoint: Checkpoint,
    },
    /// Compute finished and its typed result is durable, but the evidence-bearing
    /// claims have not yet committed. This is deliberately non-terminal.
    Publishing {
        lease: WorkerLease,
        result_ref: String,
        checkpoint: Checkpoint,
    },
    /// The lease holder is asked to stop and confirms with `ConfirmCancel`.
    CancelRequested {
        lease: WorkerLease,
        checkpoint: Checkpoint,
    },
    /// A failed attempt waits until `not_before_ms` before a worker claims it again.
    Backoff {
        not_before_ms: i64,
        checkpoint: Option<Checkpoint>,
    },
    Succeeded {
        result_ref: String,
        checkpoint: Checkpoint,
    },
    Failed {
        reason: String,
        checkpoint: Option<Checkpoint>,
    },
    Cancelled {
        checkpoint: Option<Checkpoint>,
    },
}

impl JobState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobState::Succeeded { .. } | JobState::Failed { .. } | JobState::Cancelled { .. }
        )
    }

    /// The machine state of this payload: one exhaustive match, one stage per variant.
    pub fn stage(&self) -> JobStage {
        match self {
            JobState::Submitted => JobStage::Submitted,
            JobState::Running { .. } => JobStage::Running,
            JobState::Publishing { .. } => JobStage::Publishing,
            JobState::CancelRequested { .. } => JobStage::CancelRequested,
            JobState::Backoff { .. } => JobStage::Backoff,
            JobState::Succeeded { .. } => JobStage::Succeeded,
            JobState::Failed { .. } => JobStage::Failed,
            JobState::Cancelled { .. } => JobStage::Cancelled,
        }
    }

    /// The worker lease this state carries: only `Running`, `Publishing` and `CancelRequested` hold one.
    pub fn lease(&self) -> Option<&WorkerLease> {
        match self {
            JobState::Running { lease, .. }
            | JobState::Publishing { lease, .. }
            | JobState::CancelRequested { lease, .. } => Some(lease),
            _ => None,
        }
    }

    /// The lease of a leased state, for a renewal. A renewal is a data update of the state,
    /// never an event.
    pub fn lease_mut(&mut self) -> Option<&mut WorkerLease> {
        match self {
            JobState::Running { lease, .. }
            | JobState::Publishing { lease, .. }
            | JobState::CancelRequested { lease, .. } => Some(lease),
            _ => None,
        }
    }

    /// The most recent checkpoint, if this state carries one.
    pub fn checkpoint(&self) -> Option<&Checkpoint> {
        match self {
            JobState::Running { checkpoint, .. }
            | JobState::Publishing { checkpoint, .. }
            | JobState::CancelRequested { checkpoint, .. }
            | JobState::Succeeded { checkpoint, .. } => Some(checkpoint),
            JobState::Backoff { checkpoint, .. }
            | JobState::Failed { checkpoint, .. }
            | JobState::Cancelled { checkpoint } => checkpoint.as_ref(),
            JobState::Submitted => None,
        }
    }

    /// The checkpoint of this state for an in-place data update; a checkpoint write is not an
    /// event and never assigns the state.
    pub fn checkpoint_mut(&mut self) -> Option<&mut Checkpoint> {
        match self {
            JobState::Running { checkpoint, .. }
            | JobState::Publishing { checkpoint, .. }
            | JobState::CancelRequested { checkpoint, .. }
            | JobState::Succeeded { checkpoint, .. } => Some(checkpoint),
            JobState::Backoff { checkpoint, .. }
            | JobState::Failed { checkpoint, .. }
            | JobState::Cancelled { checkpoint } => checkpoint.as_mut(),
            JobState::Submitted => None,
        }
    }

    /// Whether a cancellation was requested of the lease holder.
    pub fn cancel_requested(&self) -> bool {
        matches!(self, JobState::CancelRequested { .. })
    }

    /// The guard facts this state carries: its lease expiry and, in `Backoff`, `not_before_ms`.
    /// The caller adds who is acting and the retry budget.
    pub fn facts(&self) -> JobFacts {
        JobFacts {
            lease_expires_at_ms: self.lease().map(|lease| lease.expires_at_ms),
            not_before_ms: match self {
                JobState::Backoff { not_before_ms, .. } => *not_before_ms,
                _ => 0,
            },
            ..JobFacts::default()
        }
    }
}

/// The durable analytics-job record (CONCEPT:INT-P2-1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnalyticsJob {
    pub job_id: JobId,
    pub input_snapshot: InputSnapshotHandle,
    pub policy: JobPolicy,
    pub algo: AlgoVersion,
    /// Opaque/pseudonymized executor payload. Raw source labels, query text,
    /// filesystem paths and identities are forbidden here.
    #[serde(default)]
    pub input_payload: Option<Vec<u8>>,
    pub retry: RetryPolicy,
    /// The one stored state of the job. Its only writer is [`AnalyticsJob::transition`]; the
    /// lease, the cancel request and the backoff time are part of the state, not fields beside it.
    pub state: JobState,
    /// Monotonic source for lease fencing; never decreases on reassignment.
    #[serde(default)]
    pub lease_epoch: u64,
    /// Most recent authenticated worker reference. Retained after lease release so
    /// a lost terminal RPC response can be retried idempotently by the same slot.
    #[serde(default)]
    pub last_worker_ref: String,
    /// Durable typed result dataset. Populated before entering ``Publishing`` and
    /// retained after success so analytics output is never discarded.
    #[serde(default)]
    pub output: Option<crate::result::TypedJobResult>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl AnalyticsJob {
    /// The deterministic result reference (CONCEPT:INT-P2-1 determinism): a pure
    /// function of the input snapshot + full algorithm lineage, NOT of `job_id`. Two
    /// jobs (e.g. an original run and a retry submitted as a fresh `job_id`) that ran
    /// the same algorithm+params over the same snapshot converge on the SAME
    /// `result_ref` — the basis for idempotent result commit (see `claim.rs`).
    pub fn result_ref(&self) -> String {
        compute_result_ref(&self.input_snapshot, &self.algo)
    }

    /// Whether `policy.deadline_unix_ms` has passed as of `now_unix_ms`. Advisory —
    /// this crate never auto-fails a job on deadline; a caller's sweep does.
    pub fn deadline_exceeded(&self, now_unix_ms: i64) -> bool {
        matches!(self.policy.deadline_unix_ms, Some(d) if now_unix_ms >= d)
    }
}

/// Compute the deterministic `result_ref` for an `(input_snapshot, algo)` pair
/// (CONCEPT:INT-P2-1). Exposed standalone (not just as `AnalyticsJob::result_ref`) so
/// a caller can compute the WOULD-BE result_ref before submitting — e.g. to check
/// "has this exact computation already been committed?" without running it again.
pub fn compute_result_ref(snapshot: &InputSnapshotHandle, algo: &AlgoVersion) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"eg-jobs.result_ref.v2\0");
    hasher.update(snapshot.graph.as_bytes());
    hasher.update([0u8]);
    hasher.update(snapshot.version.to_le_bytes());
    hasher.update([0u8]);
    hasher.update(snapshot.dataset_ref.as_bytes());
    hasher.update([0u8]);
    hasher.update(snapshot.content_digest.as_bytes());
    hasher.update([0u8]);
    hasher.update(algo.family.as_bytes());
    hasher.update([0u8]);
    hasher.update(algo.algorithm.as_bytes());
    hasher.update([0u8]);
    hasher.update(algo.params_digest.as_bytes());
    hasher.update([0u8]);
    hasher.update(algo.code_version.as_bytes());
    hasher.update([0u8]);
    hasher.update(algo.env_version.as_bytes());
    let digest = hasher.finalize();
    format!("eg:job_result:{}", hex::encode(&digest[..16]))
}

/// Stable hex digest of a canonicalized JSON value — the standard way a caller
/// derives `AlgoVersion::params_digest` from its actual (arbitrarily-shaped)
/// parameter set without this crate needing to know that shape.
pub fn digest_params(params: &serde_json::Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"eg-jobs.params_digest.v1\0");
    // `serde_json::Value`'s `Display`/`to_string` serializes maps with a
    // deterministic (insertion- or BTree-ordered, depending on the `preserve_order`
    // feature) key order; either way the SAME `Value` always serializes identically
    // within one build, which is all determinism here requires (byte-identical
    // input ⇒ byte-identical digest).
    hasher.update(params.to_string().as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}
