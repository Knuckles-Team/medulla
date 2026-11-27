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
/// The domain tag of a dead-letter item's payload digest.
const DEAD_LETTER_PAYLOAD_DOMAIN: &[u8] = b"eg/ingest-dead-letter-payload/v1";

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
    /// When set, the job is stored under this id instead of a minted one.
    pub job_id: Option<String>,
    /// When set, replaces the default redelivery bound.
    pub max_attempts: Option<u32>,
}

/// The queued and running task counts of one tenant queue and the enqueue time
/// of its oldest pending task (MEDULLA-INGEST-WORKER-R001.4.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskQueueLoad {
    pub queued: u64,
    pub running: u64,
    pub oldest_enqueued_at_ms: Option<i64>,
}

/// The result of [`JobStore::submit_task`].
#[derive(Clone, Debug)]
pub enum TaskSubmitOutcome {
    /// The task was stored.
    Enqueued(Box<AnalyticsJob>),
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
    /// Redeliver after `delay_ms` without reading the retry budget: the caller's policy
    /// decided the attempt count (MEDULLA-INGEST-WORKER-R001.9).
    RetryAfter { delay_ms: u64 },
    /// Fail the task for good and park one dead-letter item for it in the same commit.
    DeadLetter { code: &'static str },
}

/// What a redrive did (MEDULLA-REPO-INGEST-R002.651.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RedriveOutcome {
    /// Tasks returned to the queue.
    pub redriven: u32,
}

/// Why a redrive was refused.
#[derive(Debug)]
pub enum RedriveError {
    /// An item id names no parked task of the tenant. Nothing changed.
    Unknown { item_id: String },
    /// The store failed.
    Store(JobError),
}

impl From<JobError> for RedriveError {
    fn from(error: JobError) -> Self {
        RedriveError::Store(error)
    }
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
    if job.policy.tenant != tenant || !job.algo.family.starts_with(TASK_FAMILY_PREFIX) {
        return Err(TaskAckError::NotClaimed);
    }
    match &job.state {
        JobState::Running { lease, checkpoint } if lease.worker_ref == worker_ref => {
            if lease.expires_at_ms <= now_ms {
                return Err(TaskAckError::LeaseExpired);
            }
            Ok(checkpoint.clone())
        }
        // A lapsed lease the scheduler already detached returned the task to `Submitted`; it
        // still names its last holder, so that holder reads as expired rather than never claimed.
        JobState::Submitted { .. } if job.last_worker_ref == worker_ref => {
            Err(TaskAckError::LeaseExpired)
        }
        _ => Err(TaskAckError::NotClaimed),
    }
}

/// The canonical JSON of the dead-letter item of a task that just failed for good. The tenant,
/// corpus key, kind and target stay in the failed task's stored envelope and are read by
/// `item_id`; the item copies none of them (MEDULLA-INGEST-WORKER-R001.9).
fn dead_letter_item_bytes(
    job: &AnalyticsJob,
    code: &str,
    detail: &str,
    now_ms: i64,
) -> Result<Vec<u8>> {
    let payload = job.input_payload.as_deref().unwrap_or_default();
    let payload_digest =
        medulla_types::contract::Digest256::framed(DEAD_LETTER_PAYLOAD_DOMAIN, &[payload])
            .map_err(codec_err)?;
    let item = medulla_ingestion_types::ingest_capabilities::DeadLetterItem {
        schema_version:
            medulla_ingestion_types::ingest_capabilities::INGEST_CAPABILITIES_SCHEMA_VERSION,
        item_id: job.job_id.clone(),
        queue: task_queue_of(job).unwrap_or_default().to_string(),
        reason: code.to_string(),
        attempts: job.retry.attempts_made,
        last_error: detail.to_string(),
        parked_at_ms: u64::try_from(now_ms).unwrap_or(0),
        payload_digest,
    };
    serde_json::to_vec(&item).map_err(codec_err)
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
        let job_id = match spec.job_id {
            Some(id) => id,
            None => self.next_job_id(),
        };
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
                max_attempts: spec.max_attempts.unwrap_or(TASK_MAX_ATTEMPTS),
                backoff_ms: 0,
                attempts_made: 0,
            },
            // A task enqueued for a later time waits in `Backoff` until it is due.
            state: if spec.not_before_ms > 0 {
                JobState::Backoff {
                    not_before_ms: spec.not_before_ms,
                    checkpoint: None,
                }
            } else {
                JobState::Submitted { checkpoint: None }
            },
            lease_epoch: 0,
            last_worker_ref: String::new(),
            routing: Vec::new(),
            output: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        self.put_raw(&job)?;
        Ok(TaskSubmitOutcome::Enqueued(Box::new(job)))
    }

    /// The queued and running counts and the oldest enqueue time of one tenant
    /// queue. Two index seeks; the cost does not grow with the queue.
    pub fn task_queue_load(&self, tenant: &str, queue: &str) -> Result<TaskQueueLoad> {
        let read = self.scoped_read()?;
        let (queued, running) = read
            .open_owner_table(TASK_QUEUE_TOTALS)
            .map_err(redb_err)?
            .get((tenant, queue))
            .map_err(redb_err)?
            .map(|value| value.value())
            .unwrap_or((0, 0));
        let oldest_table = read.open_owner_table(TASK_QUEUE_OLDEST).map_err(redb_err)?;
        let oldest = oldest_table
            .range((tenant, queue, i64::MIN, "")..=(tenant, queue, i64::MAX, "\u{10ffff}"))
            .map_err(redb_err)?
            .next()
            .transpose()
            .map_err(redb_err)?
            .map(|(key, _)| key.value().2);
        Ok(TaskQueueLoad {
            queued,
            running,
            oldest_enqueued_at_ms: oldest,
        })
    }

    /// The number of tasks of `tenant`'s `queue` parked as dead letters
    /// (MEDULLA-INGEST-WORKER-R001.9). A read of the dead-letter table, kept apart from
    /// [`task_queue_load`](Self::task_queue_load) so the hot enqueue and claim paths never pay
    /// for it. The parked item keeps no tenant, so the failed task's own row names it.
    pub fn task_queue_dead_letters(&self, tenant: &str, queue: &str) -> Result<u64> {
        let read = self.scoped_read()?;
        let mut parked = Vec::new();
        {
            let table = read.open_owner_table(TASK_DEAD_LETTERS).map_err(redb_err)?;
            for row in table.iter().map_err(redb_err)? {
                let (_, bytes) = row.map_err(redb_err)?;
                let item: medulla_ingestion_types::ingest_capabilities::DeadLetterItem =
                    serde_json::from_slice(bytes.value()).map_err(codec_err)?;
                if item.queue == queue {
                    parked.push(item.item_id);
                }
            }
        }
        drop(read);
        let mut count = 0u64;
        for item_id in parked {
            if self.get_raw(&item_id)?.policy.tenant == tenant {
                count += 1;
            }
        }
        Ok(count)
    }

    /// The parked items of `tenant`'s `queue`, ordered by `parked_at_ms` then `item_id`, at most
    /// `limit` (MEDULLA-REPO-INGEST-R002.651.3). The parked item keeps no tenant, so the failed
    /// task's own row names it; an item of another tenant is never listed.
    pub fn list_dead_letters(
        &self,
        tenant: &str,
        queue: &str,
        limit: u32,
    ) -> Result<Vec<medulla_ingestion_types::ingest_capabilities::DeadLetterItem>> {
        let read = self.scoped_read()?;
        let mut parked = Vec::new();
        {
            let table = read.open_owner_table(TASK_DEAD_LETTERS).map_err(redb_err)?;
            for row in table.iter().map_err(redb_err)? {
                let (_, bytes) = row.map_err(redb_err)?;
                let item: medulla_ingestion_types::ingest_capabilities::DeadLetterItem =
                    serde_json::from_slice(bytes.value()).map_err(codec_err)?;
                if item.queue == queue {
                    parked.push(item);
                }
            }
        }
        drop(read);
        let mut own = Vec::with_capacity(parked.len());
        for item in parked {
            if self.get_raw(&item.item_id)?.policy.tenant == tenant {
                own.push(item);
            }
        }
        own.sort_by(|a, b| {
            (a.parked_at_ms, a.item_id.as_str()).cmp(&(b.parked_at_ms, b.item_id.as_str()))
        });
        own.truncate(limit as usize);
        Ok(own)
    }

    /// Return the parked tasks `item_ids` of `tenant` to the queue (MEDULLA-REPO-INGEST-R002.651.3).
    ///
    /// Every id is checked first: one that names no parked task of the tenant refuses the whole
    /// call with nothing changed. A `Failed` job never revives (the machine keeps it terminal), so
    /// a redrive replaces the parked job by a fresh job of the same id: same payload and policy,
    /// `Submitted`, due now, zero attempts. The dead-letter row is deleted in the same commit.
    pub fn redrive_dead_letters(
        &self,
        tenant: &str,
        item_ids: &[String],
        now_ms: i64,
    ) -> std::result::Result<RedriveOutcome, RedriveError> {
        let mut parked = Vec::with_capacity(item_ids.len());
        for item_id in item_ids {
            let unknown = || RedriveError::Unknown {
                item_id: item_id.clone(),
            };
            let in_table = {
                let read = self.scoped_read()?;
                let table = read.open_owner_table(TASK_DEAD_LETTERS).map_err(redb_err)?;
                let found = table.get(item_id.as_str()).map_err(redb_err)?.is_some();
                found
            };
            if !in_table {
                return Err(unknown());
            }
            let job = self.get_raw(item_id)?;
            if job.policy.tenant != tenant || !matches!(job.state, JobState::Failed { .. }) {
                return Err(unknown());
            }
            parked.push(job);
        }
        let fresh_jobs: Vec<AnalyticsJob> = parked
            .into_iter()
            .map(|failed| AnalyticsJob {
                state: JobState::Submitted { checkpoint: None },
                retry: RetryPolicy {
                    attempts_made: 0,
                    ..failed.retry
                },
                output: None,
                last_worker_ref: String::new(),
                updated_at_ms: now_ms,
                ..failed
            })
            .collect();
        // All-or-nothing (fail-closed): every replacement job and every dead-letter row removal
        // lands in ONE jobs-store write transaction, so a store fault leaves none redriven. The
        // closure re-reads each parked job inside the write, so a job that changed after the
        // check above refuses the whole call instead of being overwritten.
        let redriven = self
            .maintain("dead-letter-redrive", |wtx| {
                for fresh in &fresh_jobs {
                    let current = read_job_in_wtx_owner(wtx, fresh.job_id.as_str())?;
                    if current.policy.tenant != tenant
                        || !matches!(current.state, JobState::Failed { .. })
                    {
                        return Err(JobError::NotFound(fresh.job_id.clone()));
                    }
                    let bytes = encode_job(fresh)?;
                    persist_job_image(wtx, fresh, bytes.as_slice())?;
                    wtx.open_table(TASK_DEAD_LETTERS)
                        .map_err(redb_err)?
                        .remove(fresh.job_id.as_str())
                        .map_err(redb_err)?;
                }
                Ok(fresh_jobs.len() as u32)
            })
            .map_err(|error| match error {
                JobError::NotFound(item_id) => RedriveError::Unknown { item_id },
                other => RedriveError::Store(other),
            })?;
        Ok(RedriveOutcome { redriven })
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
            None,
            None,
        )
    }

    /// Per-queue load of the tasks that require `tenant_capability`
    /// (MEDULLA-PLACEMENT-R002.4): for each queue name, the number of tasks
    /// holding a live lease (running) and the number ready for scheduling
    /// (queued). One read transaction over the job table, bounded by
    /// `MAX_JOB_LIST_ITEMS`; nothing is cached.
    pub fn task_lane_counts(
        &self,
        tenant_capability: &str,
        now_ms: i64,
    ) -> Result<std::collections::BTreeMap<String, (u32, u32)>> {
        let read = self.scoped_read()?;
        let table = read.open_owner_table(JOBS).map_err(redb_err)?;
        let mut counts: std::collections::BTreeMap<String, (u32, u32)> =
            std::collections::BTreeMap::new();
        let mut scanned = 0usize;
        for entry in table.iter().map_err(redb_err)? {
            scanned = scanned
                .checked_add(1)
                .filter(|count| *count <= MAX_JOB_LIST_ITEMS)
                .ok_or_else(|| codec_err("task lane count scan exceeds limits"))?;
            let (_, value) = entry.map_err(redb_err)?;
            let job = decode_job(value.value())?;
            let Some(queue) = job.algo.family.strip_prefix(TASK_FAMILY_PREFIX) else {
                continue;
            };
            if !job
                .policy
                .placement
                .required_capabilities
                .iter()
                .any(|capability| capability == tenant_capability)
            {
                continue;
            }
            let live_lease = job
                .state
                .lease()
                .is_some_and(|lease| lease.expires_at_ms > now_ms);
            let entry = counts.entry(queue.to_string()).or_insert((0, 0));
            if live_lease {
                entry.0 = entry.0.saturating_add(1);
            }
            if job_is_ready_for_scheduling(&job, live_lease, now_ms) {
                entry.1 = entry.1.saturating_add(1);
            }
        }
        Ok(counts)
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
        let owner = JobFacts {
            lease_owner_matches: true,
            ..job.state.facts()
        };
        let mut dead_letter: Option<Vec<u8>> = None;
        match outcome {
            TaskAckOutcome::Succeeded => {
                // Running reaches Succeeded through Publishing (J03, J04) inside the one transaction.
                let result_ref = job.result_ref();
                let lease = job.state.lease().cloned().ok_or(TaskAckError::NotClaimed)?;
                let publishing = JobState::Publishing {
                    lease,
                    result_ref: result_ref.clone(),
                    checkpoint: checkpoint.clone(),
                };
                job.transition(JobEvent::Publish, &owner, now_ms, publishing)
                    .map_err(JobError::from)?;
                let succeeded = JobState::Succeeded {
                    result_ref,
                    checkpoint: Checkpoint {
                        label: "acked".to_string(),
                        updated_at_ms: now_ms,
                        ..checkpoint
                    },
                };
                let owner = JobFacts {
                    lease_owner_matches: true,
                    ..job.state.facts()
                };
                job.transition(JobEvent::Complete, &owner, now_ms, succeeded)
                    .map_err(JobError::from)?;
            }
            TaskAckOutcome::Failed => {
                let facts = JobFacts {
                    attempts_below_max: false,
                    ..owner
                };
                let failed = JobState::Failed {
                    reason: reason("task_failed"),
                    checkpoint: Some(checkpoint),
                };
                job.transition(JobEvent::Fail, &facts, now_ms, failed)
                    .map_err(JobError::from)?;
            }
            TaskAckOutcome::Retry if job.retry.retries_remaining() => {
                let facts = JobFacts {
                    attempts_below_max: true,
                    ..owner
                };
                let backoff = JobState::Backoff {
                    not_before_ms: now_ms.saturating_add(job.retry.backoff_ms as i64),
                    checkpoint: Some(checkpoint),
                };
                job.transition(JobEvent::Fail, &facts, now_ms, backoff)
                    .map_err(JobError::from)?;
            }
            TaskAckOutcome::Retry => {
                let facts = JobFacts {
                    attempts_below_max: false,
                    ..owner
                };
                let failed = JobState::Failed {
                    reason: "retries_exhausted".to_string(),
                    checkpoint: Some(checkpoint),
                };
                job.transition(JobEvent::Fail, &facts, now_ms, failed)
                    .map_err(JobError::from)?;
            }
            TaskAckOutcome::RetryAfter { delay_ms } => {
                let facts = JobFacts {
                    attempts_below_max: true,
                    ..owner
                };
                let backoff = JobState::Backoff {
                    not_before_ms: now_ms
                        .saturating_add(i64::try_from(delay_ms).unwrap_or(i64::MAX)),
                    checkpoint: Some(checkpoint),
                };
                job.transition(JobEvent::Fail, &facts, now_ms, backoff)
                    .map_err(JobError::from)?;
            }
            TaskAckOutcome::DeadLetter { code } => {
                let facts = JobFacts {
                    attempts_below_max: false,
                    ..owner
                };
                let failed = JobState::Failed {
                    reason: code.to_string(),
                    checkpoint: Some(checkpoint),
                };
                job.transition(JobEvent::Fail, &facts, now_ms, failed)
                    .map_err(JobError::from)?;
                dead_letter = Some(dead_letter_item_bytes(&job, code, detail, now_ms)?);
            }
        }
        job.updated_at_ms = now_ms;
        let side = dead_letter
            .as_deref()
            .map(|bytes| JobSideRow::DeadLetter(task_id, bytes));
        let (batch, _next_version) =
            write_job_transition(self, &write, &job, expected_version, side)?;
        self.mutations.commit(write, &batch).map_err(redb_err)?;
        Ok(job)
    }

    /// Whether `worker_ref` holds the live lease of a task of `tenant`, by the rule of
    /// [`ack_task`](Self::ack_task) and without writing. A task of another tenant and an unknown
    /// task read as not claimed (MEDULLA-INGEST-WORKER-R605.4).
    pub fn check_task_lease(
        &self,
        tenant: &str,
        task_id: &str,
        worker_ref: &str,
        now_ms: i64,
    ) -> std::result::Result<(), TaskAckError> {
        let job = match self.get(task_id) {
            Ok(job) => job,
            Err(JobError::NotFound(_)) => return Err(TaskAckError::NotClaimed),
            Err(error) => return Err(error.into()),
        };
        require_task_lease(&job, tenant, worker_ref, now_ms).map(|_| ())
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
        let lease = job.state.lease_mut().ok_or(TaskAckError::NotClaimed)?;
        lease.expires_at_ms = now_ms.saturating_add(i64::try_from(lease_ms).unwrap_or(i64::MAX));
        job.updated_at_ms = now_ms;
        let (batch, _next_version) =
            write_job_transition(self, &write, &job, expected_version, None)?;
        self.mutations.commit(write, &batch).map_err(redb_err)?;
        Ok(job)
    }
}

#[cfg(test)]
mod lane_count_tests {
    use super::*;
    use crate::dev_scope_grant::open_dev_store_in_dir;

    fn task(queue: &str, key: &str, capability: &str) -> TaskSubmit {
        TaskSubmit {
            tenant: "tenant-a".to_string(),
            actor: "actor".to_string(),
            tenant_capability: capability.to_string(),
            queue: queue.to_string(),
            kind: "kind".to_string(),
            payload_ref: format!("payload-{key}"),
            idempotency_key: key.to_string(),
            priority: 5,
            not_before_ms: 0,
            job_id: None,
            max_attempts: None,
        }
    }

    // spec: MEDULLA-PLACEMENT-R002.4
    #[test]
    fn task_lane_counts_split_running_and_queued() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_dev_store_in_dir(dir.path()).unwrap();
        store
            .submit_task(task("general", "g1", "cap-a"), 1_000)
            .unwrap();
        store
            .submit_task(task("general", "g2", "cap-a"), 1_000)
            .unwrap();
        store
            .submit_task(task("hydration", "h1", "cap-a"), 1_000)
            .unwrap();
        store
            .submit_task(task("general", "other", "cap-b"), 1_000)
            .unwrap();

        let before = store.task_lane_counts("cap-a", 1_001).unwrap();
        assert_eq!(before.get("general"), Some(&(0, 2)));
        assert_eq!(before.get("hydration"), Some(&(0, 1)));
        assert_eq!(store.task_lane_counts("cap-b", 1_001).unwrap().len(), 1);
        assert!(store
            .task_lane_counts("cap-none", 1_001)
            .unwrap()
            .is_empty());

        let claim = store
            .claim_task("worker-ref", "cap-a", 1_002, 60_000)
            .unwrap();
        assert!(claim.is_some());
        let after = store.task_lane_counts("cap-a", 1_003).unwrap();
        let running: u32 = after.values().map(|(running, _)| *running).sum();
        let queued: u32 = after.values().map(|(_, queued)| *queued).sum();
        assert_eq!(running, 1);
        assert_eq!(queued, 2);

        // The lease lapses: the task is no longer running.
        let lapsed = store.task_lane_counts("cap-a", 1_002 + 60_001).unwrap();
        assert_eq!(lapsed.values().map(|(running, _)| *running).sum::<u32>(), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dev_scope_grant::open_dev_store_in_dir;

    const CAPABILITY: &str = "eg:task_tenant:test";

    fn submit(store: &JobStore, key: &str, queue: &str, now_ms: i64) -> String {
        let spec = TaskSubmit {
            tenant: "acme".to_string(),
            actor: "principal:test".to_string(),
            tenant_capability: CAPABILITY.to_string(),
            queue: queue.to_string(),
            kind: "work".to_string(),
            payload_ref: "{}".to_string(),
            idempotency_key: key.to_string(),
            priority: 5,
            not_before_ms: 0,
            job_id: None,
            max_attempts: None,
        };
        match store.submit_task(spec, now_ms).unwrap() {
            TaskSubmitOutcome::Enqueued(job) => job.job_id,
            TaskSubmitOutcome::Duplicate => panic!("unexpected duplicate"),
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R001.9
    #[test]
    fn dead_letter_ack_fails_task_and_parks_item_in_one_commit() {
        use medulla_ingestion_types::ingest_capabilities::DeadLetterItem;

        let dir = tempfile::tempdir().unwrap();
        let store = open_dev_store_in_dir(dir.path()).unwrap();
        let id = submit(&store, "a", "ingest", 1_000);
        let claim = store
            .claim_task("worker-1", CAPABILITY, 2_000, 60_000)
            .unwrap()
            .expect("a ready task");
        assert_eq!(claim.job.job_id, id);
        let before = store.live_version().unwrap();

        let job = store
            .ack_task(
                "acme",
                &id,
                "worker-1",
                TaskAckOutcome::DeadLetter {
                    code: "INGEST_RETRIES_EXHAUSTED",
                },
                "parser refused the page",
                3_000,
            )
            .unwrap();
        match &job.state {
            JobState::Failed { reason, .. } => assert_eq!(reason, "INGEST_RETRIES_EXHAUSTED"),
            other => panic!("expected a failed task, got {other:?}"),
        }
        // The failed state and the parked item share one committed version step.
        assert_eq!(store.live_version().unwrap(), before + 1);
        let read = store.scoped_read().unwrap();
        let table = read.open_owner_table(TASK_DEAD_LETTERS).unwrap();
        let row = table.get(id.as_str()).unwrap().expect("one parked item");
        let item: DeadLetterItem = serde_json::from_slice(row.value()).unwrap();
        assert_eq!(item.item_id, id);
        assert_eq!(item.queue, "ingest");
        assert_eq!(item.reason, "INGEST_RETRIES_EXHAUSTED");
        assert_eq!(item.attempts, 1);
        assert_eq!(item.last_error, "parser refused the page");
        assert_eq!(item.parked_at_ms, 3_000);
        let expected = medulla_types::contract::Digest256::framed(
            b"eg/ingest-dead-letter-payload/v1",
            &[b"{}".as_slice()],
        )
        .unwrap();
        assert_eq!(item.payload_digest, expected);
        assert_eq!(redb::ReadableTableMetadata::len(&table).unwrap(), 1);
        // The failed task left the queue load.
        let load = store.task_queue_load("acme", "ingest").unwrap();
        assert_eq!((load.queued, load.running), (0, 0));
    }

    fn park(store: &JobStore, key: &str, now_ms: i64) -> String {
        let id = submit(store, key, "ingest", now_ms);
        store
            .claim_task("worker-1", CAPABILITY, now_ms + 1, 60_000)
            .unwrap()
            .expect("a ready task");
        store
            .ack_task(
                "acme",
                &id,
                "worker-1",
                TaskAckOutcome::DeadLetter {
                    code: "INGEST_RETRIES_EXHAUSTED",
                },
                "boom",
                now_ms + 2,
            )
            .unwrap();
        id
    }

    // spec: MEDULLA-REPO-INGEST-R002.651.3
    #[test]
    fn redrive_is_all_or_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_dev_store_in_dir(dir.path()).unwrap();
        let first = park(&store, "a", 1_000);
        let second = park(&store, "b", 10_000);
        let listed = store.list_dead_letters("acme", "ingest", 10).unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|item| item.item_id.as_str())
                .collect::<Vec<_>>(),
            [first.as_str(), second.as_str()]
        );
        assert!(store
            .list_dead_letters("other", "ingest", 10)
            .unwrap()
            .is_empty());
        assert_eq!(
            store.list_dead_letters("acme", "ingest", 1).unwrap().len(),
            1
        );

        // One unknown id refuses the whole call: nothing is redriven, both rows stay.
        let before = store.live_version().unwrap();
        let refused = store
            .redrive_dead_letters("acme", &[first.clone(), "ghost".to_string()], 20_000)
            .unwrap_err();
        assert!(matches!(refused, RedriveError::Unknown { ref item_id } if item_id == "ghost"));
        assert_eq!(store.live_version().unwrap(), before);
        assert_eq!(
            store.list_dead_letters("acme", "ingest", 10).unwrap().len(),
            2
        );
        // A tenant that does not own the item reads it as unknown.
        assert!(matches!(
            store.redrive_dead_letters("other", std::slice::from_ref(&first), 20_000),
            Err(RedriveError::Unknown { .. })
        ));

        let done = store
            .redrive_dead_letters("acme", std::slice::from_ref(&first), 20_000)
            .unwrap();
        assert_eq!(done.redriven, 1);
        assert_eq!(
            store.list_dead_letters("acme", "ingest", 10).unwrap().len(),
            1
        );
        let load = store.task_queue_load("acme", "ingest").unwrap();
        assert_eq!(load.queued, 1);
        // The task is claimable again with a fresh attempt count.
        let claim = store
            .claim_task("worker-2", CAPABILITY, 21_000, 60_000)
            .unwrap()
            .expect("the redriven task is ready");
        assert_eq!(claim.job.job_id, first);
        assert_eq!(claim.job.retry.attempts_made, 1);
        // A second redrive of the same id refuses with the store unchanged.
        let again = store.live_version().unwrap();
        assert!(matches!(
            store.redrive_dead_letters("acme", &[first], 22_000),
            Err(RedriveError::Unknown { .. })
        ));
        assert_eq!(store.live_version().unwrap(), again);
    }

    // spec: MEDULLA-REPO-INGEST-R002.651.3
    #[test]
    fn a_redrive_that_fails_midway_redrives_none_and_one_commit_redrives_all() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_dev_store_in_dir(dir.path()).unwrap();
        let first = park(&store, "a", 1_000);
        let second = park(&store, "b", 5_000);
        // The second occurrence of one id finds the job already replaced inside the write,
        // so the whole transaction rolls back: neither id is redriven.
        let before = store.live_version().unwrap();
        let refused = store
            .redrive_dead_letters("acme", &[first.clone(), first.clone()], 20_000)
            .unwrap_err();
        assert!(matches!(refused, RedriveError::Unknown { .. }));
        assert_eq!(store.live_version().unwrap(), before);
        assert_eq!(
            store.list_dead_letters("acme", "ingest", 10).unwrap().len(),
            2
        );
        // Both ids in one call commit as one version step.
        let done = store
            .redrive_dead_letters("acme", &[first, second], 21_000)
            .unwrap();
        assert_eq!(done.redriven, 2);
        assert_eq!(store.live_version().unwrap(), before + 1);
        assert!(store
            .list_dead_letters("acme", "ingest", 10)
            .unwrap()
            .is_empty());
    }

    // spec: MEDULLA-INGEST-WORKER-R001.9
    #[test]
    fn retry_after_redelivers_after_the_delay_without_reading_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_dev_store_in_dir(dir.path()).unwrap();
        let id = submit(&store, "a", "ingest", 1_000);
        store
            .claim_task("worker-1", CAPABILITY, 2_000, 60_000)
            .unwrap()
            .expect("a ready task");
        store
            .ack_task(
                "acme",
                &id,
                "worker-1",
                TaskAckOutcome::RetryAfter { delay_ms: 5_000 },
                "",
                3_000,
            )
            .unwrap();
        assert!(store
            .claim_task("worker-1", CAPABILITY, 7_999, 60_000)
            .unwrap()
            .is_none());
        let again = store
            .claim_task("worker-1", CAPABILITY, 8_000, 60_000)
            .unwrap()
            .expect("due after the delay");
        assert_eq!(again.job.job_id, id);
        assert_eq!(again.job.retry.attempts_made, 2);
    }

    // proves R001.4.1: Wiring: the served ingest enqueue and the shared ingest queue stack
    // spec: MEDULLA-INGEST-WORKER-R001.4.1
    #[test]
    fn task_queue_load_counts_queued_running_and_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_dev_store_in_dir(dir.path()).unwrap();
        let empty = store.task_queue_load("acme", "ingest").unwrap();
        assert_eq!(empty, TaskQueueLoad::default());

        let first = submit(&store, "a", "ingest", 1_000);
        submit(&store, "b", "ingest", 2_000);
        submit(&store, "c", "ingest", 3_000);
        let load = store.task_queue_load("acme", "ingest").unwrap();
        assert_eq!(
            load,
            TaskQueueLoad {
                queued: 3,
                running: 0,
                oldest_enqueued_at_ms: Some(1_000)
            }
        );
        assert_eq!(
            store.task_queue_load("other-tenant", "ingest").unwrap(),
            TaskQueueLoad::default()
        );

        let claim = store
            .claim_task("worker-1", CAPABILITY, 4_000, 60_000)
            .unwrap()
            .expect("a ready task");
        let claimed_id = claim.job.job_id.clone();
        let load = store.task_queue_load("acme", "ingest").unwrap();
        assert_eq!((load.queued, load.running), (2, 1));

        store
            .ack_task(
                "acme",
                &claimed_id,
                "worker-1",
                TaskAckOutcome::Succeeded,
                "",
                5_000,
            )
            .unwrap();
        let load = store.task_queue_load("acme", "ingest").unwrap();
        assert_eq!((load.queued, load.running), (2, 0));
        // The acked task was the oldest or one of the others; the oldest left is
        // the earliest of the two still pending.
        let expected_oldest = if claimed_id == first { 2_000 } else { 1_000 };
        assert_eq!(load.oldest_enqueued_at_ms, Some(expected_oldest));
    }
}
