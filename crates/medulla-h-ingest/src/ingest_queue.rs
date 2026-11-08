//! Served ingest queue entry: `enqueue_ingest_job` maps a validated job onto
//! the served task queue with deterministic ids
//! (MEDULLA-INGEST-WORKER-R001.4).
//!
//! The claim, ack, backpressure read and dead-letter routing are later rows
//! (R001.9 to R001.12). The write reaches the task queue only through
//! `JobsApi`; this crate never calls a sibling handler.

use std::fmt;
use std::sync::Arc;

use medulla_server_ops::api::ingest::JobsApi;
use medulla_source_types::ingest_job::{
    partition_key, target_embeds_credentials, task_id, IngestEnqueueReceipt, IngestJobEnvelope,
    IngestJobError, IngestRetryPolicy,
};
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

/// Map one ingest job onto the served task queue.
///
/// `verified_tenant` is the tenant of the verified request context; the
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
    verified_tenant: &str,
    envelope: &IngestJobEnvelope,
    policy: &IngestRetryPolicy,
    current_depth: u64,
    max_depth: u64,
) -> Result<IngestEnqueueReceipt, IngestEnqueueError> {
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
    let body = serde_json::to_value(envelope)
        .map_err(|error| rejected(ServerErrorCode::IngestJobInvalid, error.to_string()))?;
    let deduplicated = jobs
        .enqueue_ingest_task(&queue, &task, &partition.render(), body)
        .map_err(|error| rejected(ServerErrorCode::IngestJobInvalid, error))?;
    Ok(IngestEnqueueReceipt {
        task_id: task,
        queue: queue.to_string(),
        partition: partition.render(),
        deduplicated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use medulla_source_types::ingest_job::IngestJobKind;

    /// A recording `JobsApi` standing in for the served task queue.
    struct RecordingJobs {
        calls: Mutex<Vec<(String, String, String, String)>>,
        preexisting_task_id: Option<String>,
    }

    impl JobsApi for RecordingJobs {
        fn enqueue_ingest_task(
            &self,
            queue: &str,
            task_id: &str,
            partition: &str,
            envelope: serde_json::Value,
        ) -> Result<bool, String> {
            self.calls.lock().unwrap().push((
                queue.to_string(),
                task_id.to_string(),
                partition.to_string(),
                envelope.to_string(),
            ));
            Ok(self.preexisting_task_id.as_deref() == Some(task_id))
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
            enqueue_ingest_job(Some(jobs.clone()), "t1", &e, &policy(), 3, 10_000).unwrap();
        assert_eq!(receipt.task_id, expected);
        assert_eq!(receipt.queue, "ingest");
        assert_eq!(receipt.partition, "t1/repo-a/document");
        assert!(!receipt.deduplicated);
        let calls = jobs.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "ingest");
        assert_eq!(calls[0].1, expected);
        assert_eq!(calls[0].2, "t1/repo-a/document");
        let stored: IngestJobEnvelope = serde_json::from_str(&calls[0].3).unwrap();
        assert_eq!(stored, e);
    }

    // spec: MEDULLA-INGEST-WORKER-R001.4
    #[test]
    fn duplicate_task_id_returns_the_first_receipt_with_deduplicated_true() {
        let e = env();
        let first =
            enqueue_ingest_job(Some(recording_jobs(None)), "t1", &e, &policy(), 0, 10_000).unwrap();
        assert!(!first.deduplicated);
        let jobs = recording_jobs(Some(first.task_id.clone()));
        let second =
            enqueue_ingest_job(Some(jobs.clone()), "t1", &e, &policy(), 1, 10_000).unwrap();
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
            enqueue_ingest_job(Some(jobs.clone()), "t2", &e, &policy(), 0, 10_000).unwrap_err();
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
            enqueue_ingest_job(Some(jobs.clone()), "t1", &cred, &policy(), 0, 10_000).unwrap_err();
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
            "t1",
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
        let err = enqueue_ingest_job(Some(jobs.clone()), "t1", &e, &policy(), 10_000, 10_000)
            .unwrap_err();
        assert_eq!(err.code, ServerErrorCode::IngestQueueFull);
        assert!(err.detail.contains("ingest"));
        assert!(jobs.calls.lock().unwrap().is_empty());
        // Without the seam the refusal stays in the closed vocabulary.
        let err = enqueue_ingest_job(None, "t1", &e, &policy(), 0, 10_000).unwrap_err();
        assert_eq!(err.code, ServerErrorCode::IngestJobInvalid);
        assert!(err.detail.contains("seam"));
    }
}
