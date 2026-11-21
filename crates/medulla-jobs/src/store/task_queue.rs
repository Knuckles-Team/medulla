//! The served task queue's typed view of the job store
//! (MEDULLA-DURABLE-KERNEL-R072).
//!
//! One queue store exists: a queued task is an [`AnalyticsJob`] of family
//! `task.<queue>` in this same `jobs.redb`. Enqueue submits that job, claim leases
//! it through the one worker-claim path, and ack maps `Succeeded`, `Failed` and
//! `Retry` to the fenced terminal, failed and release transitions. An unacked
//! lease lapses and the scheduler reconciliation returns the task to the ready
//! index, so redelivery survives a restart: the lease is a durable row, not
//! process memory.

use sha2::{Digest, Sha256};

use super::*;
use crate::model::{AlgoVersion, InputSnapshotHandle, JobPlacement, RetryPolicy};

/// The job family prefix of every queued task; the queue name follows it.
pub const TASK_FAMILY_PREFIX: &str = "task.";
/// Redelivery bound: a task leased this many times without an ack fails.
const TASK_MAX_ATTEMPTS: u32 = 5;
/// A task priority of 0 to 9 maps onto the job priority range 0 to 90.
const TASK_PRIORITY_STEP: i32 = 10;
const TASK_CODE_VERSION: &str = "task-queue-v1";
const TASK_ENV_VERSION: &str = "eg-jobs-v1";

/// Which job family a worker claim may lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimFamily {
    /// Analytics jobs: every family except queued tasks.
    Analytics,
    /// Queued tasks of the `task.` family.
    Task,
}

impl ClaimFamily {
    /// Whether a worker of this family may lease `job`.
    pub(super) fn admits(self, job: &AnalyticsJob) -> bool {
        let is_task = job.algo.family.starts_with(TASK_FAMILY_PREFIX);
        matches!(self, ClaimFamily::Task) == is_task
    }
}

/// One task to enqueue. The tenant, actor and tenant capability come from the
/// verified request; the capability is the only placement token, so a worker of
/// another tenant never sees the task.
#[derive(Clone, Debug)]
pub struct TaskSubmit {
    pub tenant: String,
    pub actor: String,
    /// Opaque placement token naming the tenant's task workers.
    pub tenant_capability: String,
    pub queue: String,
    pub kind: String,
    pub payload_ref: String,
    pub idempotency_key: String,
    /// 0 to 9; higher leases first.
    pub priority: u8,
    /// Earliest engine time at which the task may be leased.
    pub not_before_ms: i64,
}

/// The result of [`JobStore::submit_task`].
#[derive(Clone, Debug)]
pub enum TaskSubmitOutcome {
    /// The task was stored.
    Enqueued(AnalyticsJob),
    /// The idempotency key of this tenant and queue was already used.
    Duplicate,
}

/// The outcome a worker reports for a leased task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskAckOutcome {
    /// The task is done.
    Succeeded,
    /// The task failed for good.
    Failed,
    /// Release the task for redelivery, keeping its attempt count.
    Retry,
}

/// Why an ack was refused.
#[derive(Debug)]
pub enum TaskAckError {
    /// The task is unknown, is not leased, or is leased to another worker.
    NotClaimed,
    /// The lease of this worker lapsed before the ack.
    LeaseExpired,
    /// The store failed.
    Store(JobError),
}

impl From<JobError> for TaskAckError {
    fn from(error: JobError) -> Self {
        TaskAckError::Store(error)
    }
}

/// The ledger key of one (tenant, queue, idempotency key) triple. Every part is
/// length-prefixed so two triples never share a key, and the digest keeps the key
/// inside the identifier bounds whatever the caller sent.
fn task_ledger_key(tenant: &str, queue: &str, idempotency_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"eg-jobs.task_idempotency.v1\0");
    for part in [tenant, queue, idempotency_key] {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    format!("task:{}", hex::encode(hasher.finalize()))
}

/// The one ownership rule of a leased task: the task is running, belongs to
/// `tenant`, is of a `task.` family, and `worker_ref` holds a live lease on it.
/// Ack and renew both call this; no second rule exists. Returns the running
/// checkpoint.
fn require_task_lease(
    job: &AnalyticsJob,
    tenant: &str,
    worker_ref: &str,
    now_ms: i64,
) -> std::result::Result<Checkpoint, TaskAckError> {
    let checkpoint = match &job.state {
        JobState::Running { checkpoint } => checkpoint.clone(),
        _ => return Err(TaskAckError::NotClaimed),
    };
    if job.policy.tenant != tenant || !job.algo.family.starts_with(TASK_FAMILY_PREFIX) {
        return Err(TaskAckError::NotClaimed);
    }
    match job.lease.as_ref() {
        Some(lease) if lease.worker_ref == worker_ref => {
            if lease.expires_at_ms <= now_ms {
                return Err(TaskAckError::LeaseExpired);
            }
        }
        // A lapsed lease the scheduler already detached still names its last
        // holder, so that holder reads as expired rather than never claimed.
        None if job.last_worker_ref == worker_ref => {
            return Err(TaskAckError::LeaseExpired);
        }
        _ => return Err(TaskAckError::NotClaimed),
    }
    Ok(checkpoint)
}

impl JobStore {
    /// Enqueue one task as a job of family `task.<queue>`. The first use of an
    /// idempotency key in a tenant's queue stores the task; any later use returns
    /// [`TaskSubmitOutcome::Duplicate`] and writes nothing.
    pub fn submit_task(&self, spec: TaskSubmit, now_ms: i64) -> Result<TaskSubmitOutcome> {
        let policy = JobPolicy {
            tenant: spec.tenant.clone(),
            actor: spec.actor,
            purpose: "task_queue".to_string(),
            priority: i32::from(spec.priority) * TASK_PRIORITY_STEP,
            placement: JobPlacement {
                required_capabilities: vec![spec.tenant_capability],
                ..JobPlacement::default()
            },
            ..JobPolicy::default()
        };
        validate_priority(&policy)?;
        validate_placement(&policy)?;
        let job_id = self.next_job_id();
        let key = task_ledger_key(&spec.tenant, &spec.queue, &spec.idempotency_key);
        if !self.claim_idempotency(&key, &job_id)? {
            return Ok(TaskSubmitOutcome::Duplicate);
        }
        let payload_digest = hex::encode(Sha256::digest(spec.payload_ref.as_bytes()));
        let job = AnalyticsJob {
            input_snapshot: InputSnapshotHandle::new(
                format!("{TASK_FAMILY_PREFIX}{}", spec.queue),
                0,
            )
            .with_dataset("", job_id.clone()),
            job_id,
            policy,
            algo: AlgoVersion {
                family: format!("{TASK_FAMILY_PREFIX}{}", spec.queue),
                algorithm: spec.kind,
                params_digest: payload_digest,
                code_version: TASK_CODE_VERSION.to_string(),
                env_version: TASK_ENV_VERSION.to_string(),
            },
            input_payload: Some(spec.payload_ref.into_bytes()),
            retry: RetryPolicy {
                max_attempts: TASK_MAX_ATTEMPTS,
                backoff_ms: 0,
                attempts_made: 0,
            },
            state: JobState::Submitted,
            cancel_requested: false,
            lease_epoch: 0,
            lease: None,
            last_worker_ref: String::new(),
            not_before_ms: spec.not_before_ms,
            output: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        self.put_raw(&job)?;
        Ok(TaskSubmitOutcome::Enqueued(job))
    }

    /// Lease the next ready task that requires `tenant_capability`, through the
    /// one worker-claim path. An analytics job is never leased here.
    pub fn claim_task(
        &self,
        worker_ref: &str,
        tenant_capability: &str,
        now_ms: i64,
        lease_ms: u64,
    ) -> Result<Option<WorkerClaim>> {
        let quota = TenantJobQuota {
            max_active: usize::MAX,
            max_reserved_cpu_ms: u64::MAX,
        };
        self.claim_next_family(
            worker_ref,
            &[tenant_capability.to_string()],
            now_ms,
            lease_ms,
            quota,
            ClaimFamily::Task,
        )
    }

    /// Acknowledge a leased task of `tenant`. The ack is accepted only from the
    /// worker holding the live lease; a task of another tenant reads as not
    /// claimed so its existence never leaks.
    pub fn ack_task(
        &self,
        tenant: &str,
        task_id: &str,
        worker_ref: &str,
        outcome: TaskAckOutcome,
        detail: &str,
        now_ms: i64,
    ) -> std::result::Result<AnalyticsJob, TaskAckError> {
        let expected_version = self.live_version()?;
        let write = self.mutations.open_write(&self.owner).map_err(redb_err)?;
        let mut job = match read_job_in_wtx(&write, task_id) {
            Ok(job) => job,
            Err(JobError::NotFound(_)) => return Err(TaskAckError::NotClaimed),
            Err(error) => return Err(error.into()),
        };
        let checkpoint = require_task_lease(&job, tenant, worker_ref, now_ms)?;
        let reason = |fallback: &str| {
            if detail.is_empty() {
                fallback.to_string()
            } else {
                detail.to_string()
            }
        };
        match outcome {
            TaskAckOutcome::Succeeded => {
                job.state = JobState::Succeeded {
                    result_ref: job.result_ref(),
                    checkpoint: Checkpoint {
                        stage: "acked".to_string(),
                        updated_at_ms: now_ms,
                        ..checkpoint
                    },
                };
            }
            TaskAckOutcome::Failed => {
                job.state = JobState::Failed {
                    reason: reason("task_failed"),
                    checkpoint: Some(checkpoint),
                };
            }
            TaskAckOutcome::Retry if job.retry.retries_remaining() => {
                job.state = JobState::Submitted;
                job.not_before_ms = now_ms.saturating_add(job.retry.backoff_ms as i64);
            }
            TaskAckOutcome::Retry => {
                job.state = JobState::Failed {
                    reason: "retries_exhausted".to_string(),
                    checkpoint: Some(checkpoint),
                };
            }
        }
        job.lease = None;
        job.updated_at_ms = now_ms;
        let (batch, _next_version) =
            write_job_transition(self, &write, &job, expected_version, None)?;
        self.mutations.commit(write, &batch).map_err(redb_err)?;
        Ok(job)
    }

    /// Extend the live lease of a task `worker_ref` holds to `now_ms + lease_ms`
    /// in one fenced transition. The attempt count and the job state do not
    /// change. Ownership is the rule of [`ack_task`](Self::ack_task).
    pub fn renew_task(
        &self,
        tenant: &str,
        task_id: &str,
        worker_ref: &str,
        now_ms: i64,
        lease_ms: u64,
    ) -> std::result::Result<AnalyticsJob, TaskAckError> {
        let expected_version = self.live_version()?;
        let write = self.mutations.open_write(&self.owner).map_err(redb_err)?;
        let mut job = match read_job_in_wtx(&write, task_id) {
            Ok(job) => job,
            Err(JobError::NotFound(_)) => return Err(TaskAckError::NotClaimed),
            Err(error) => return Err(error.into()),
        };
        require_task_lease(&job, tenant, worker_ref, now_ms)?;
        let lease = job.lease.as_mut().ok_or(TaskAckError::NotClaimed)?;
        lease.expires_at_ms = now_ms.saturating_add(i64::try_from(lease_ms).unwrap_or(i64::MAX));
        job.updated_at_ms = now_ms;
        let (batch, _next_version) =
            write_job_transition(self, &write, &job, expected_version, None)?;
        self.mutations.commit(write, &batch).map_err(redb_err)?;
        Ok(job)
    }
}
