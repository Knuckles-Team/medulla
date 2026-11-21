//! Served ingest queue entry: `enqueue_ingest_job` maps a validated job onto
//! the served task queue with deterministic ids
//! (MEDULLA-INGEST-WORKER-R001.4).
//!
//! The claim, ack, backpressure read and dead-letter routing are later rows
//! (R001.9 to R001.12). The write reaches the task queue only through
//! `JobsApi`; this crate never calls a sibling handler.

use std::fmt;
use std::sync::Arc;

use medulla_server_ops::api::ingest::{IngestTaskWrite, JobsApi};
use medulla_source_types::ingest_job::{
    partition_key, target_embeds_credentials, task_id, IngestEnqueueReceipt, IngestJobEnvelope,
    IngestJobError, IngestJobKind, IngestQueueDepthReport, IngestQueueDepthRow, IngestRetryPolicy,
    INGEST_TASK_PAYLOAD_MAX_BYTES,
};
use medulla_kernel::metrics::IngestJobOutcome;
use medulla_types::contract::ServerErrorCode;

/// One refusal of a served `IngestEnqueue`. Every refusal changes no state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestEnqueueError {
    /// The stable wire code the client sees.
    pub code: ServerErrorCode,
    /// The detail carried with the code.
    pub detail: String,
}

impl fmt::Display for IngestEnqueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.detail)
    }
}

impl std::error::Error for IngestEnqueueError {}

fn rejected(code: ServerErrorCode, detail: String) -> IngestEnqueueError {
    IngestEnqueueError { code, detail }
}

/// The verified identity and store location one ingest enqueue runs under
/// (MEDULLA-INGEST-WORKER-R001.4.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestCaller {
    /// The configured persist directory of the engine.
    pub persist_dir: Option<String>,
    /// The tenant of the verified request context.
    pub tenant: String,
    /// The principal of the verified request context.
    pub principal: String,
}

/// The snake-case name of a job kind, the `kind` of its task.
fn kind_name(kind: IngestJobKind) -> &'static str {
    match kind {
        IngestJobKind::Document => "document",
        IngestJobKind::Codebase => "codebase",
        IngestJobKind::SkillWorkflows => "skill_workflows",
        IngestJobKind::CapabilityHydration => "capability_hydration",
        IngestJobKind::Extraction => "extraction",
    }
}

/// Map one ingest job onto the served task queue.
///
/// `caller.tenant` is the tenant of the verified request context; the
/// envelope tenant must equal it. The envelope is validated against `policy`,
/// the `ingest_retry` key of `SchedulerConfig`. The queue is
/// `lane_for(kind).queue_name()`, the partition is `partition_key(envelope)`
/// and the task id is the framed digest `task_id(envelope)`.
/// `current_depth` and `max_depth` are the queue depth read before this write
/// and its configured limit; the depth refusal runs before any write
/// (MEDULLA-INGEST-WORKER-R001.11 owns the depth read). A duplicate task id
/// returns the first receipt with `deduplicated` true and writes nothing.
pub fn enqueue_ingest_job(
    jobs: Option<Arc<dyn JobsApi>>,
    caller: &IngestCaller,
    envelope: &IngestJobEnvelope,
    policy: &IngestRetryPolicy,
    current_depth: u64,
    max_depth: u64,
) -> Result<IngestEnqueueReceipt, IngestEnqueueError> {
    let outcome = enqueue_unrecorded(
        jobs,
        caller,
        envelope,
        policy,
        current_depth,
        max_depth,
    );
    let queue = envelope.lane().queue_name();
    let counted = match &outcome {
        Ok(receipt) if receipt.deduplicated => IngestJobOutcome::Deduplicated,
        Ok(_) => IngestJobOutcome::Enqueued,
        Err(_) => IngestJobOutcome::Refused,
    };
    medulla_kernel::metrics::process().record_job(counted, queue);
    outcome
}

fn enqueue_unrecorded(
    jobs: Option<Arc<dyn JobsApi>>,
    caller: &IngestCaller,
    envelope: &IngestJobEnvelope,
    policy: &IngestRetryPolicy,
    current_depth: u64,
    max_depth: u64,
) -> Result<IngestEnqueueReceipt, IngestEnqueueError> {
    let verified_tenant = caller.tenant.as_str();
    if envelope.tenant != verified_tenant {
        return Err(rejected(
            ServerErrorCode::IngestTenantMismatch,
            format!(
                "envelope tenant '{}' does not match the verified tenant '{verified_tenant}'",
                envelope.tenant
            ),
        ));
    }
    if target_embeds_credentials(&envelope.target) {
        return Err(rejected(
            ServerErrorCode::IngestJobInvalid,
            IngestJobError::TargetHasCredential.to_string(),
        ));
    }
    if let Err(error) = envelope.validate(policy) {
        return Err(rejected(
            ServerErrorCode::IngestJobInvalid,
            error.to_string(),
        ));
    }
    let queue = envelope.lane().queue_name();
    if current_depth >= max_depth {
        return Err(rejected(
            ServerErrorCode::IngestQueueFull,
            format!("queue \'{queue}\' depth {current_depth} is at limit {max_depth}"),
        ));
    }
    let jobs = jobs.ok_or_else(|| {
        rejected(
            ServerErrorCode::IngestJobInvalid,
            "the jobs seam is not installed; the task queue is unreachable".into(),
        )
    })?;
    let task = task_id(envelope);
    let partition = partition_key(envelope);
    let payload = serde_json::to_string(envelope)
        .map_err(|error| rejected(ServerErrorCode::IngestJobInvalid, error.to_string()))?;
    if payload.len() > INGEST_TASK_PAYLOAD_MAX_BYTES {
        return Err(rejected(
            ServerErrorCode::IngestJobInvalid,
            format!(
                "ingest payload of {} bytes exceeds the limit of {INGEST_TASK_PAYLOAD_MAX_BYTES}",
                payload.len()
            ),
        ));
    }
    let write = IngestTaskWrite {
        tenant: verified_tenant,
        actor: caller.principal.as_str(),
        queue,
        kind: kind_name(envelope.kind),
        task_id: &task,
        payload: &payload,
    };
    let deduplicated = jobs
        .enqueue_ingest_task(&caller.persist_dir, &write)
        .map_err(|error| rejected(ServerErrorCode::IngestJobInvalid, error))?;
    Ok(IngestEnqueueReceipt {
        task_id: task,
        queue: queue.to_string(),
        partition: partition.render(),
        deduplicated,
    })
}

/// The depth report of the reserved ingest queues (MEDULLA-INGEST-WORKER-R001.11). Each load is
/// `(queue, (queued, running, oldest_enqueued_at_ms, dead_letters))`: the first three as
/// `JobsApi::task_queue_load` answers them, the one depth count the enqueue refusal reads too,
/// and the dead-letter count of `JobsApi::task_queue_dead_letters`. Rows keep the order of `loads`. `depth` is
/// queued plus running; `oldest_age_ms` is `now_ms - oldest_enqueued_at_ms` saturated at 0, and 0
/// for a queue with no pending task.
pub fn queue_depth_report(
    loads: &[(&'static str, (u64, u64, Option<i64>, u64))],
    now_ms: i64,
) -> IngestQueueDepthReport {
    IngestQueueDepthReport {
        queues: loads
            .iter()
            .map(|(queue, (queued, running, oldest, dead))| IngestQueueDepthRow {
                queue: (*queue).to_string(),
                depth: queued.saturating_add(*running),
                oldest_age_ms: oldest
                    .map(|enqueued| u64::try_from(now_ms.saturating_sub(enqueued)).unwrap_or(0))
                    .unwrap_or(0),
                dead_letters: *dead,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // spec: MEDULLA-INGEST-WORKER-R001.11
    #[test]
    fn depth_report_orders_queues_and_saturates_age() {
        let report = queue_depth_report(
            &[
                ("ingest", (3, 2, Some(1_000), 4)),
                ("ingest-hydration", (0, 0, None, 0)),
            ],
            4_500,
        );
        assert_eq!(
            report.queues,
            vec![
                IngestQueueDepthRow {
                    queue: "ingest".into(),
                    depth: 5,
                    oldest_age_ms: 3_500,
                    dead_letters: 4,
                },
                IngestQueueDepthRow {
                    queue: "ingest-hydration".into(),
                    depth: 0,
                    oldest_age_ms: 0,
                    dead_letters: 0,
                },
            ]
        );
        // An enqueue time after now (clock skew) saturates at zero, never wraps.
        let skewed = queue_depth_report(&[("ingest", (1, 0, Some(9_000), 0))], 4_500);
        assert_eq!(skewed.queues[0].oldest_age_ms, 0);
    }

    use medulla_source_types::ingest_job::IngestJobKind;

    /// A recording `JobsApi` standing in for the served task queue.
    struct RecordingJobs {
        calls: Mutex<Vec<(String, String, String, String)>>,
        preexisting_task_id: Option<String>,
    }

    impl JobsApi for RecordingJobs {
        fn enqueue_ingest_task(
            &self,
            _persist_dir: &Option<String>,
            write: &IngestTaskWrite<'_>,
        ) -> Result<bool, String> {
            self.calls.lock().unwrap().push((
                write.queue.to_string(),
                write.task_id.to_string(),
                write.kind.to_string(),
                write.payload.to_string(),
            ));
            Ok(self.preexisting_task_id.as_deref() == Some(write.task_id))
        }
    }

    fn recording_jobs(preexisting_task_id: Option<String>) -> Arc<RecordingJobs> {
        Arc::new(RecordingJobs {
            calls: Mutex::new(Vec::new()),
            preexisting_task_id,
        })
    }

    fn env() -> IngestJobEnvelope {
        IngestJobEnvelope {
            job_id: "job-1".into(),
            tenant: "t1".into(),
            corpus_key: "repo-a".into(),
            kind: IngestJobKind::Document,
            target: "docs/readme.md".into(),
            attempt: 1,
            page: None,
        }
    }

    fn caller(tenant: &str) -> IngestCaller {
        IngestCaller {
            persist_dir: None,
            tenant: tenant.to_string(),
            principal: "principal:test".to_string(),
        }
    }

    fn policy() -> IngestRetryPolicy {
        IngestRetryPolicy {
            max_attempts: 5,
            base_delay_ms: 100,
            max_delay_ms: 2_000,
            jitter: 0.5,
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R001.4
    #[test]
    fn enqueue_maps_onto_the_served_queue_with_a_deterministic_id() {
        let e = env();
        let expected = task_id(&e);
        let jobs = recording_jobs(None);
        let receipt =
            enqueue_ingest_job(Some(jobs.clone()), &caller("t1"), &e, &policy(), 3, 10_000).unwrap();
        assert_eq!(receipt.task_id, expected);
        assert_eq!(receipt.queue, "ingest");
        assert_eq!(receipt.partition, "t1/repo-a/document");
        assert!(!receipt.deduplicated);
        let calls = jobs.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "ingest");
        assert_eq!(calls[0].1, expected);
        assert_eq!(calls[0].2, "document");
        let stored: IngestJobEnvelope = serde_json::from_str(&calls[0].3).unwrap();
        assert_eq!(stored, e);
    }

    // spec: MEDULLA-INGEST-WORKER-R001.4
    #[test]
    fn duplicate_task_id_returns_the_first_receipt_with_deduplicated_true() {
        let e = env();
        let first =
            enqueue_ingest_job(Some(recording_jobs(None)), &caller("t1"), &e, &policy(), 0, 10_000).unwrap();
        assert!(!first.deduplicated);
        let jobs = recording_jobs(Some(first.task_id.clone()));
        let second =
            enqueue_ingest_job(Some(jobs.clone()), &caller("t1"), &e, &policy(), 1, 10_000).unwrap();
        assert_eq!(second.task_id, first.task_id);
        assert_eq!(second.queue, first.queue);
        assert_eq!(second.partition, first.partition);
        assert!(second.deduplicated);
    }

    // spec: MEDULLA-INGEST-WORKER-R001.4
    #[test]
    fn refusals_name_their_code_and_change_nothing() {
        let e = env();
        // A forged tenant is refused before any queue write.
        let jobs = recording_jobs(None);
        let err =
            enqueue_ingest_job(Some(jobs.clone()), &caller("t2"), &e, &policy(), 0, 10_000).unwrap_err();
        assert_eq!(err.code, ServerErrorCode::IngestTenantMismatch);
        assert!(err.detail.contains("t2"));
        assert!(jobs.calls.lock().unwrap().is_empty());
        // A credential target is refused with the IngestJobError named.
        let cred = IngestJobEnvelope {
            target: String::from(concat!("postgres://user:pass", "@host/db")),
            ..e.clone()
        };
        let jobs = recording_jobs(None);
        let err =
            enqueue_ingest_job(Some(jobs.clone()), &caller("t1"), &cred, &policy(), 0, 10_000).unwrap_err();
        assert_eq!(err.code, ServerErrorCode::IngestJobInvalid);
        assert_eq!(err.detail, IngestJobError::TargetHasCredential.to_string());
        assert!(jobs.calls.lock().unwrap().is_empty());
        // An envelope that fails validation is refused the same way.
        let over = IngestJobEnvelope {
            attempt: 9,
            ..e.clone()
        };
        let err = enqueue_ingest_job(
            Some(recording_jobs(None)),
            &caller("t1"),
            &over,
            &policy(),
            0,
            10_000,
        )
        .unwrap_err();
        assert_eq!(err.code, ServerErrorCode::IngestJobInvalid);
        assert!(err.detail.contains("attempt"));
        // A full queue refuses before the write.
        let jobs = recording_jobs(None);
        let err = enqueue_ingest_job(Some(jobs.clone()), &caller("t1"), &e, &policy(), 10_000, 10_000)
            .unwrap_err();
        assert_eq!(err.code, ServerErrorCode::IngestQueueFull);
        assert!(err.detail.contains("ingest"));
        assert!(jobs.calls.lock().unwrap().is_empty());
        // Without the seam the refusal stays in the closed vocabulary.
        let err = enqueue_ingest_job(None, &caller("t1"), &e, &policy(), 0, 10_000).unwrap_err();
        assert_eq!(err.code, ServerErrorCode::IngestJobInvalid);
        assert!(err.detail.contains("seam"));
    }
}
