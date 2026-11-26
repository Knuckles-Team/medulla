//! Served task-queue methods over the durable job store
//! (MEDULLA-DURABLE-KERNEL-R072).
//!
//! `TaskQueueEnqueue`, `TaskQueueClaim`, `TaskQueueRenew` and `TaskQueueAck` are a typed facade
//! over the one queue store, `medulla_jobs::JobStore`: enqueue submits a job of
//! family `task.<queue>`, claim leases the next ready task through the worker
//! claim, and ack maps `Succeeded`, `Failed` and `Retry` to the fenced terminal,
//! failed and release transitions. Scopes `task:enqueue` and `task:work` gate the
//! methods before this handler runs. The tenant and principal come from the
//! verified request context; the caller supplies neither. A task is reachable
//! only through the opaque tenant capability, so another tenant's claim returns
//! none and its ack reads as not claimed.

use std::sync::Arc;

use medulla_jobs::{
    JobError, JobStore, TaskAckError, TaskAckOutcome, TaskSubmit, TaskSubmitOutcome,
    TASK_FAMILY_PREFIX,
};
use crate::worker_admission::{admission_inputs, AdmissionInputs};
use medulla_kernel::metrics::IngestJobOutcome;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_protocol_types::result_contract::coordination as contract;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_source_types::ingest_job::{
    ingest_lane_of_queue, on_ingest_ack, on_ingest_claim, IngestAckDecision, IngestRetryPolicy,
};
use medulla_types::contract::{EngineErrorCode, ServerErrorCode};
use medulla_work_types::worker_registry::{
    admit_reserved_worker_task, WorkerAdmission, WorkerAdmissionRefusal, WorkerSizingInput,
};
use medulla_work_types::task_queue::{
    ClaimedTask, TaskAck, TaskAcked, TaskClaim, TaskEnqueue, TaskEnqueued, TaskOutcome, TaskRenew,
    TaskRenewed,
};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

/// Longest `worker_id` or `grant_id`, in bytes.
const MAX_WORKER_FIELD_BYTES: usize = 256;

/// The verified identity and engine time one task-queue call runs under.
pub struct TaskQueueCall<'a> {
    /// The request id echoed in the response.
    pub req_id: u64,
    /// The tenant of the verified request context.
    pub tenant: &'a str,
    /// The principal of the verified request context.
    pub principal: &'a str,
    /// Engine time of the call, in milliseconds.
    pub now_ms: i64,
}

fn invalid(req_id: u64, detail: &str) -> Response {
    Response::err(
        req_id,
        format!("{}: {detail}", EngineErrorCode::InvalidArgument.as_str()),
    )
}

fn opaque(namespace: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    format!("eg:{namespace}:{}", hex::encode(hasher.finalize()))
}

/// The opaque placement token every task of `tenant` requires.
pub(crate) fn tenant_capability(tenant: &str) -> String {
    opaque("task_tenant", &[tenant])
}

/// The opaque lease holder of one worker: bound to the tenant and the verified
/// principal, so no other caller can ack or redeliver as this worker.
fn worker_ref(call: &TaskQueueCall<'_>, worker_id: &str) -> String {
    opaque("task_worker", &[call.tenant, call.principal, worker_id])
}

fn worker_field_valid(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= MAX_WORKER_FIELD_BYTES
}

fn store_refusal(req_id: u64, error: &JobError) -> Response {
    Response::err(req_id, crate::jobs::job_refusal(error))
}

fn answer(req_id: u64, payload: Result<ResultPayload, String>) -> Response {
    match payload {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

/// Serve `TaskQueueEnqueue`: store one task of the verified tenant.
pub fn enqueue(store: &JobStore, call: &TaskQueueCall<'_>, request: TaskEnqueue) -> Response {
    let req_id = call.req_id;
    if let Err(refusal) = request.validate() {
        return Response::err(req_id, refusal.to_string());
    }
    if request.kind.trim().is_empty() {
        return invalid(req_id, "task kind must not be blank");
    }
    if medulla_source_types::ingest_job::ingest_lane_of_queue(&request.queue).is_some() {
        return invalid(
            req_id,
            &format!(
                "queue '{}' is written only by IngestEnqueue",
                request.queue
            ),
        );
    }
    let spec = TaskSubmit {
        tenant: call.tenant.to_string(),
        actor: call.principal.to_string(),
        tenant_capability: tenant_capability(call.tenant),
        queue: request.queue,
        kind: request.kind,
        payload_ref: request.payload_ref,
        idempotency_key: request.idempotency_key,
        priority: request.priority,
        not_before_ms: i64::try_from(request.not_before_ms).unwrap_or(i64::MAX),
        job_id: None,
        max_attempts: None,
    };
    match store.submit_task(spec, call.now_ms) {
        Ok(TaskSubmitOutcome::Enqueued(job)) => answer(
            req_id,
            ResultPayload::of::<contract::TaskQueueEnqueue>(TaskEnqueued {
                task_id: job.job_id,
            }),
        ),
        Ok(TaskSubmitOutcome::Duplicate) => Response::err(
            req_id,
            format!(
                "{}: this idempotency key was already used in the queue",
                ServerErrorCode::TaskDuplicate.as_str()
            ),
        ),
        Err(error) => store_refusal(req_id, &error),
    }
}

/// The one reserved-worker admission rule of MEDULLA-DURABLE-KERNEL-R075 applied to a claim
/// (MEDULLA-INGEST-WORKER-R604.3): the worker must be live in the registry, and the grant its
/// lane load allows must not starve the reserve. A refusal names the closed code and nothing is
/// claimed or written.
fn admit_claim(
    call: &TaskQueueCall<'_>,
    request: &TaskClaim,
    admission: &AdmissionInputs,
) -> Result<(), String> {
    let now_ms = u64::try_from(call.now_ms).unwrap_or(0);
    let Some(record) = admission.registry.get_live(&request.worker_id, now_ms) else {
        let refusal = WorkerAdmissionRefusal::WorkerUnknown;
        return Err(format!(
            "{}: no live worker '{}' is registered",
            refusal.code(),
            request.worker_id
        ));
    };
    let decision = admit_reserved_worker_task(
        record,
        &admission.config,
        &admission.lane_load,
        WorkerSizingInput {
            request_ceiling: None,
            available_memory_bytes: record.memory_bytes,
            is_write_lane: false,
        },
        admission.shard_writer_width,
        request.grant_id.clone(),
    );
    match decision {
        WorkerAdmission::Granted(_) => Ok(()),
        WorkerAdmission::Refused(refusal) => {
            let detail = match &refusal {
                WorkerAdmissionRefusal::AdmitHeldForReserve { lane } => {
                    format!(
                        "lane '{lane}' would drop below its reserve; retry_after_ms {}",
                        admission.config.admit_retry_ms
                    )
                }
                WorkerAdmissionRefusal::WorkerCapacityZero { limit } => {
                    format!("worker capacity is zero at limit '{limit}'")
                }
                WorkerAdmissionRefusal::WorkerUnknown => {
                    format!("no live worker '{}' is registered", request.worker_id)
                }
            };
            Err(format!("{}: {detail}", refusal.code()))
        }
    }
}

/// The most tasks one claim looks at: a task of an ingest queue that is past its attempts is
/// dead-lettered and the claim goes on to the next (MEDULLA-INGEST-WORKER-R001.12).
const MAX_CLAIM_ROUNDS: usize = 16;

/// Serve `TaskQueueClaim`: admit the worker through the one admission rule, then lease the next
/// ready task of the verified tenant, or answer nil when none is ready. The lease length is the
/// request's. A leased task of an ingest queue whose attempts ran out (a lease that lapsed on
/// every delivery) is dead-lettered with `LEASE_EXPIRED_EXHAUSTED` instead of delivered, and the
/// claim leases again, at most `MAX_CLAIM_ROUNDS` times (MEDULLA-INGEST-WORKER-R001.12).
pub fn claim(
    store: &JobStore,
    call: &TaskQueueCall<'_>,
    request: TaskClaim,
    admission: &AdmissionInputs,
    ingest_policy: &Result<IngestRetryPolicy, String>,
) -> Response {
    let req_id = call.req_id;
    if let Err(refusal) = request.validate() {
        return Response::err(req_id, refusal.to_string());
    }
    if !worker_field_valid(&request.worker_id) || !worker_field_valid(&request.grant_id) {
        return invalid(req_id, "task worker_id and grant_id must be 1 to 256 bytes");
    }
    if let Err(refusal) = admit_claim(call, &request, admission) {
        return Response::err(req_id, refusal);
    }
    let worker = worker_ref(call, &request.worker_id);
    for _ in 0..MAX_CLAIM_ROUNDS {
        let claimed = store.claim_task(
            &worker,
            &tenant_capability(call.tenant),
            call.now_ms,
            request.lease_ms,
        );
        let claim = match claimed {
            Ok(Some(claim)) => claim,
            Ok(None) => return answer(req_id, ResultPayload::of::<contract::TaskQueueClaim>(None)),
            Err(error) => return store_refusal(req_id, &error),
        };
        let queue = claim
            .job
            .algo
            .family
            .strip_prefix(TASK_FAMILY_PREFIX)
            .unwrap_or_default()
            .to_string();
        if ingest_lane_of_queue(&queue).is_some() {
            let policy = match ingest_policy {
                Ok(policy) => policy,
                Err(error) => return Response::err(req_id, error.clone()),
            };
            if on_ingest_claim(policy, claim.job.retry.attempts_made) {
                // The claiming worker holds the live lease, so the dead-letter write is legal.
                let parked = store.ack_task(
                    call.tenant,
                    &claim.job.job_id,
                    &worker,
                    TaskAckOutcome::DeadLetter {
                        code: ServerErrorCode::LeaseExpiredExhausted.as_str(),
                    },
                    "",
                    call.now_ms,
                );
                if let Err(error) = parked {
                    return lease_refusal(req_id, error, "claim");
                }
                medulla_kernel::metrics::process()
                    .record_job(IngestJobOutcome::DeadLettered, &queue);
                continue;
            }
        }
        return answer(
            req_id,
            ResultPayload::of::<contract::TaskQueueClaim>(Some(ClaimedTask {
                queue,
                kind: claim.job.algo.algorithm.clone(),
                payload_ref: claim
                    .job
                    .input_payload
                    .as_deref()
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .unwrap_or_default(),
                attempt: claim.job.retry.attempts_made,
                lease_expires_at_ms: claim.lease.expires_at_ms,
                task_id: claim.job.job_id,
            })),
        );
    }
    answer(req_id, ResultPayload::of::<contract::TaskQueueClaim>(None))
}

/// The ack the store performs for a task of an ingest queue: the pure decision of
/// MEDULLA-INGEST-WORKER-R001.9 over the job's attempt count, or `None` when the task is not of
/// an ingest queue of this tenant (the lease check of the store then refuses it as unclaimed).
fn ingest_ack_decision(
    store: &JobStore,
    call: &TaskQueueCall<'_>,
    request: &TaskAck,
    policy: &Result<IngestRetryPolicy, String>,
) -> Result<Option<(IngestAckDecision, String)>, String> {
    let Ok(job) = store.get(&request.task_id) else {
        return Ok(None);
    };
    let Some(queue) = job.algo.family.strip_prefix(TASK_FAMILY_PREFIX) else {
        return Ok(None);
    };
    if job.policy.tenant != call.tenant || ingest_lane_of_queue(queue).is_none() {
        return Ok(None);
    }
    let policy = policy.as_ref().map_err(Clone::clone)?;
    let decision = on_ingest_ack(
        policy,
        job.retry.attempts_made,
        &request.task_id,
        request.outcome,
    );
    Ok(Some((decision, queue.to_string())))
}

/// Serve `TaskQueueAck`: complete, fail or release a task the caller's worker
/// holds under a live lease. A task of an ingest queue is acked by the decision of
/// MEDULLA-INGEST-WORKER-R001.9 under `ingest_policy`, the policy in force; every other queue acks
/// as the caller said.
pub fn ack(
    store: &JobStore,
    call: &TaskQueueCall<'_>,
    request: TaskAck,
    ingest_policy: &Result<IngestRetryPolicy, String>,
) -> Response {
    let req_id = call.req_id;
    if let Err(refusal) = request.validate() {
        return Response::err(req_id, refusal.to_string());
    }
    if !worker_field_valid(&request.worker_id) {
        return invalid(req_id, "task worker_id must be 1 to 256 bytes");
    }
    let mut outcome = match request.outcome {
        TaskOutcome::Succeeded => TaskAckOutcome::Succeeded,
        TaskOutcome::Failed => TaskAckOutcome::Failed,
        TaskOutcome::Retry => TaskAckOutcome::Retry,
    };
    let mut ingest = None;
    match ingest_ack_decision(store, call, &request, ingest_policy) {
        Ok(Some((decision, queue))) => {
            outcome = match decision {
                IngestAckDecision::Complete => TaskAckOutcome::Succeeded,
                IngestAckDecision::Redeliver { delay_ms } => {
                    TaskAckOutcome::RetryAfter { delay_ms }
                }
                IngestAckDecision::DeadLetter => TaskAckOutcome::DeadLetter {
                    code: ServerErrorCode::IngestRetriesExhausted.as_str(),
                },
            };
            ingest = Some((decision, queue));
        }
        Ok(None) => {}
        Err(error) => return Response::err(req_id, error),
    }
    let acked = store.ack_task(
        call.tenant,
        &request.task_id,
        &worker_ref(call, &request.worker_id),
        outcome,
        &request.detail,
        call.now_ms,
    );
    if let (Ok(_), Some((decision, queue))) = (&acked, &ingest) {
        let recorded = match decision {
            IngestAckDecision::Complete => IngestJobOutcome::Completed,
            IngestAckDecision::Redeliver { .. } => IngestJobOutcome::Retried,
            IngestAckDecision::DeadLetter => IngestJobOutcome::DeadLettered,
        };
        medulla_kernel::metrics::process().record_job(recorded, queue);
    }
    match acked {
        Ok(job) => answer(
            req_id,
            ResultPayload::of::<contract::TaskQueueAck>(TaskAcked {
                task_id: job.job_id,
                outcome: request.outcome,
            }),
        ),
        Err(error) => lease_refusal(req_id, error, "ack"),
    }
}

/// The refusal of a task call that needs a live lease: not claimed by this
/// worker, lapsed before the call, or a store failure.
fn lease_refusal(req_id: u64, error: TaskAckError, call: &str) -> Response {
    match error {
        TaskAckError::NotClaimed => Response::err(
            req_id,
            format!(
                "{}: the task is not leased to this worker",
                ServerErrorCode::TaskAckNotClaimed.as_str()
            ),
        ),
        TaskAckError::LeaseExpired => Response::err(
            req_id,
            format!(
                "{}: the lease lapsed before the {call}",
                ServerErrorCode::TaskAckLeaseExpired.as_str()
            ),
        ),
        TaskAckError::Store(error) => store_refusal(req_id, &error),
    }
}

/// Serve `TaskQueueRenew`: extend the live lease the caller's worker holds on a
/// task. The attempt count and the task state do not change.
pub fn renew(store: &JobStore, call: &TaskQueueCall<'_>, request: TaskRenew) -> Response {
    let req_id = call.req_id;
    if let Err(refusal) = request.validate() {
        return Response::err(req_id, refusal.to_string());
    }
    if !worker_field_valid(&request.worker_id) {
        return invalid(req_id, "task worker_id must be 1 to 256 bytes");
    }
    let renewed = store.renew_task(
        call.tenant,
        &request.task_id,
        &worker_ref(call, &request.worker_id),
        call.now_ms,
        request.lease_ms,
    );
    match renewed {
        Ok(job) => answer(
            req_id,
            ResultPayload::of::<contract::TaskQueueRenew>(TaskRenewed {
                task_id: job.job_id,
                lease_expires_at_ms: job
                    .state
                    .lease()
                    .map(|lease| lease.expires_at_ms)
                    .unwrap_or_default(),
            }),
        ),
        Err(error) => lease_refusal(req_id, error, "renewal"),
    }
}

/// The ingest retry policy in force: the stored scheduler policy, else the startup knobs. An
/// unreadable policy is an error value, so only the ack of an ingest task refuses on it.
async fn ingest_retry_policy(
    state: &Arc<RwLock<ServerState>>,
) -> Result<IngestRetryPolicy, String> {
    let store = {
        let current = state.read().await;
        medulla_persistence::scheduler_config_store::shared_scheduler_config_store(&current)
            .map_err(|error| format!("INTERNAL: {error}"))?
    };
    let config = crate::scheduler_config::effective_config(&store)?;
    Ok(IngestRetryPolicy::from_limits(&config.ingest_retry))
}

/// Route one task-queue method to its handler, or hand the method back when it
/// is not one of the three.
pub async fn try_handle(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified_context: &VerifiedRequestContext,
    method: Method,
) -> Result<Response, Method> {
    if !matches!(
        method,
        Method::TaskQueueEnqueue { .. }
            | Method::TaskQueueClaim { .. }
            | Method::TaskQueueAck { .. }
            | Method::TaskQueueRenew { .. }
            | Method::DeadLetterList { .. }
            | Method::DeadLetterRedrive { .. }
    ) {
        return Err(method);
    }
    let store = match crate::jobs::outbox_job_store(state).await {
        Ok(store) => store,
        Err(error) => return Ok(Response::err(req_id, error)),
    };
    let call = TaskQueueCall {
        req_id,
        tenant: verified_context.tenant(),
        principal: verified_context.principal(),
        now_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms() as i64,
    };
    Ok(match method {
        Method::TaskQueueEnqueue {
            queue,
            kind,
            payload_ref,
            idempotency_key,
            priority,
            not_before_ms,
        } => enqueue(
            &store,
            &call,
            TaskEnqueue {
                queue,
                kind,
                payload_ref,
                idempotency_key,
                priority,
                not_before_ms,
            },
        ),
        Method::TaskQueueClaim {
            worker_id,
            grant_id,
            lease_ms,
        } => {
            let admission = match admission_inputs(state, verified_context, &store).await {
                Ok(inputs) => inputs,
                Err(error) => return Ok(Response::err(req_id, error)),
            };
            let policy = ingest_retry_policy(state).await;
            claim(
                &store,
                &call,
                TaskClaim {
                    worker_id,
                    grant_id,
                    lease_ms,
                },
                &admission,
                &policy,
            )
        }
        Method::TaskQueueAck {
            task_id,
            worker_id,
            outcome,
            detail,
        } => {
            let policy = ingest_retry_policy(state).await;
            ack(
                &store,
                &call,
                TaskAck {
                    task_id,
                    worker_id,
                    outcome,
                    detail,
                },
                &policy,
            )
        }
        Method::TaskQueueRenew {
            task_id,
            worker_id,
            lease_ms,
        } => renew(
            &store,
            &call,
            TaskRenew {
                task_id,
                worker_id,
                lease_ms,
            },
        ),
        Method::DeadLetterList { queue, limit } => dead_letter_list(&store, &call, &queue, limit),
        Method::DeadLetterRedrive { item_ids } => dead_letter_redrive(&store, &call, &item_ids),
        other => return Err(other),
    })
}

/// A dead-letter request outside its bounds: a non-ingest queue, a limit outside 1 to 1000, an
/// empty id list or more than 1000 ids (MEDULLA-REPO-INGEST-R002.651.3).
fn dead_letter_invalid(req_id: u64, detail: &str) -> Response {
    Response::err(
        req_id,
        format!("{}: {detail}", ServerErrorCode::DeadLetterRequestInvalid.as_str()),
    )
}

/// Serve `DeadLetterList`: the parked tasks of one ingest queue of the verified tenant.
pub fn dead_letter_list(
    store: &JobStore,
    call: &TaskQueueCall<'_>,
    queue: &str,
    limit: u32,
) -> Response {
    use medulla_ingestion_types::ingest_capabilities::MAX_DEAD_LETTER_BATCH;
    if ingest_lane_of_queue(queue).is_none() {
        return dead_letter_invalid(call.req_id, "the queue is not an ingest queue");
    }
    if limit == 0 || limit > MAX_DEAD_LETTER_BATCH {
        return dead_letter_invalid(call.req_id, "limit must be 1 to 1000");
    }
    match store.list_dead_letters(call.tenant, queue, limit) {
        Ok(items) => answer(
            call.req_id,
            ResultPayload::of::<contract::DeadLetterList>(
                medulla_ingestion_types::ingest_capabilities::DeadLetterListed { items },
            ),
        ),
        Err(error) => store_refusal(call.req_id, &error),
    }
}

/// Serve `DeadLetterRedrive`: every id names a parked task of the verified tenant or the whole
/// call refuses and nothing changes.
pub fn dead_letter_redrive(
    store: &JobStore,
    call: &TaskQueueCall<'_>,
    item_ids: &[String],
) -> Response {
    use medulla_ingestion_types::ingest_capabilities::MAX_DEAD_LETTER_BATCH;
    if item_ids.is_empty() || item_ids.len() > MAX_DEAD_LETTER_BATCH as usize {
        return dead_letter_invalid(call.req_id, "item_ids must hold 1 to 1000 ids");
    }
    match store.redrive_dead_letters(call.tenant, item_ids, call.now_ms) {
        Ok(outcome) => answer(
            call.req_id,
            ResultPayload::of::<contract::DeadLetterRedrive>(
                medulla_ingestion_types::ingest_capabilities::DeadLetterRedriven {
                    redriven: outcome.redriven,
                },
            ),
        ),
        // One message for an absent id and another tenant's id: no existence oracle.
        Err(medulla_jobs::RedriveError::Unknown { item_id }) => Response::err(
            call.req_id,
            format!(
                "{}: no parked task {item_id}",
                ServerErrorCode::DeadLetterUnknownItem.as_str()
            ),
        ),
        Err(medulla_jobs::RedriveError::Store(error)) => store_refusal(call.req_id, &error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_jobs::dev_scope_grant::open_dev_store_in_dir;
    use medulla_source_types::ingest_job::{task_id, IngestJobEnvelope, IngestJobKind};
    use medulla_work_types::scheduler_config::SchedulerConfig;
    use medulla_work_types::worker_registry::{WorkerRegistration, WorkerRegistry};
    use std::collections::BTreeMap;

    fn call<'a>(now_ms: i64) -> TaskQueueCall<'a> {
        TaskQueueCall {
            req_id: 1,
            tenant: "acme",
            principal: "principal:test",
            now_ms,
        }
    }

    fn admission(workers: &[&str]) -> AdmissionInputs {
        let mut registry = WorkerRegistry::default();
        for worker in workers {
            registry
                .register(
                    WorkerRegistration {
                        worker_id: (*worker).to_string(),
                        host_iri: "host:alpha".into(),
                        cores: 16,
                        memory_bytes: 8 * 1024 * 1024 * 1024,
                        lanes_supported: ["general".to_string()].into(),
                    },
                    0,
                    u64::MAX / 2,
                )
                .unwrap();
        }
        let total_workers = 8;
        AdmissionInputs {
            registry,
            config: SchedulerConfig {
            total_workers,
            lane_floors: BTreeMap::new(),
            cores_per_mille: 1000,
            memory_per_worker_bytes: 1024 * 1024 * 1024,
            max_queue_depth: 64,
            analytics_workers: 1,
            analytics_lease_ms: 60_000,
            analytics_poll_ms: 250,
            task_lease_ms: 60_000,
            retry_after_ms: 250,
            worker_ttl_ms: 30_000,
            analytics_slots: 1,
            hydration_scope_cap: 10_000,
            hydration_split_target_files: 2_000,
            architecture_view_max_containers: 512,
            enrich_timeout_ms: 5_000,
            history_max_commits: 100_000,
            history_support_floor: 3,
            history_confidence_floor_milli: 300,
            history_max_files_per_commit: 200,
            process_import_max_elements: 100_000,
            process_compile_max_nodes: 100_000,
            document_max_bytes: 67_108_864,
            process_document_max_bytes: 16 * 1024 * 1024,
            process_document_max_depth: 64,
            changeset_max_blob_bytes:
                medulla_work_types::scheduler_config::DEFAULT_CHANGESET_MAX_BLOB_BYTES,
            tree_checkpoint_interval:
                medulla_work_types::scheduler_config::DEFAULT_TREE_CHECKPOINT_INTERVAL,
            index_max_working_bytes:
                medulla_work_types::scheduler_config::DEFAULT_INDEX_MAX_WORKING_BYTES,
            ann_storage_ceiling_bytes:
                medulla_work_types::scheduler_config::DEFAULT_ANN_STORAGE_CEILING_BYTES,
            ann_hub_pruning: medulla_work_types::scheduler_config::AnnHubPruning::Disabled,
            orchestration_limits: Default::default(),
            ingest_retry: Default::default(),
            admit_retry_ms: medulla_work_types::scheduler_config::DEFAULT_ADMIT_RETRY_MS,
        },
            lane_load: BTreeMap::new(),
            shard_writer_width: 4,
        }
    }

    fn default_policy() -> IngestRetryPolicy {
        IngestRetryPolicy::from_limits(
            &medulla_work_types::scheduler_config::IngestRetryLimits::default(),
        )
    }

    fn ingest_envelope() -> IngestJobEnvelope {
        IngestJobEnvelope {
            job_id: "job-1".into(),
            tenant: "acme".into(),
            corpus_key: "repo-a".into(),
            kind: IngestJobKind::Document,
            target: "docs/readme.md".into(),
            attempt: 1,
            page: None,
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R604.3
    #[test]
    fn claim_runs_admission_before_leasing() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_dev_store_in_dir(dir.path()).unwrap();
        store
            .submit_task(
                TaskSubmit {
                    tenant: "acme".into(),
                    actor: "principal:test".into(),
                    tenant_capability: tenant_capability("acme"),
                    queue: "ingest".into(),
                    kind: "document".into(),
                    payload_ref: "{}".into(),
                    idempotency_key: "k".into(),
                    priority: medulla_work_types::task_queue::default_priority(),
                    not_before_ms: 0,
                    job_id: None,
                    max_attempts: None,
                },
                1_000,
            )
            .unwrap();
        let request = || TaskClaim {
            worker_id: "worker-1".into(),
            grant_id: "grant-1".into(),
            lease_ms: 30_000,
        };
        // An unregistered worker is refused and the task stays ready.
        let refused = claim(&store, &call(2_000), request(), &admission(&[]), &Ok(default_policy()));
        assert!(
            refused.error.as_deref().is_some_and(|e| e.contains("WORKER_UNKNOWN")),
            "{:?}",
            refused.error
        );
        let load = store.task_queue_load("acme", "ingest").unwrap();
        assert_eq!((load.queued, load.running), (1, 0));
        // A registered worker is admitted and leases the task.
        let leased = claim(&store, &call(3_000), request(), &admission(&["worker-1"]), &Ok(default_policy()));
        assert!(leased.error.is_none(), "{:?}", leased.error);
        let load = store.task_queue_load("acme", "ingest").unwrap();
        assert_eq!((load.queued, load.running), (0, 1));
    }

    // spec: MEDULLA-INGEST-WORKER-R001.9
    #[test]
    fn ingest_failures_redeliver_with_backoff_then_dead_letter() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_dev_store_in_dir(dir.path()).unwrap();
        let envelope = ingest_envelope();
        let digest = task_id(&envelope);
        store
            .submit_task(
                TaskSubmit {
                    tenant: "acme".into(),
                    actor: "principal:test".into(),
                    tenant_capability: tenant_capability("acme"),
                    queue: "ingest".into(),
                    kind: "document".into(),
                    payload_ref: serde_json::to_string(&envelope).unwrap(),
                    idempotency_key: digest.clone(),
                    priority: medulla_work_types::task_queue::default_priority(),
                    not_before_ms: 0,
                    job_id: Some(digest.clone()),
                    max_attempts: Some(u32::MAX),
                },
                1_000,
            )
            .unwrap();
        let policy = Ok(default_policy());
        let mut now = 2_000;
        for attempt in 1..=5u32 {
            let claimed = claim(
                &store,
                &call(now),
                TaskClaim {
                    worker_id: "worker-1".into(),
                    grant_id: "grant-1".into(),
                    lease_ms: 30_000,
                },
                &admission(&["worker-1"]),
                &Ok(default_policy()),
            );
            assert!(claimed.error.is_none(), "attempt {attempt}: {:?}", claimed.error);
            let acked = ack(
                &store,
                &call(now + 1),
                TaskAck {
                    task_id: digest.clone(),
                    worker_id: "worker-1".into(),
                    outcome: TaskOutcome::Failed,
                    detail: "parse failed".into(),
                },
                &policy,
            );
            assert!(acked.error.is_none(), "attempt {attempt}: {:?}", acked.error);
            let job = store.get(&digest).unwrap();
            if attempt < 5 {
                // Redelivery waits inside the next_delay bounds of this attempt.
                let ceiling = 1_000i64 << (attempt - 1);
                match job.state {
                    medulla_jobs::JobState::Backoff { not_before_ms, .. } => {
                        let gap = not_before_ms - (now + 1);
                        assert!(gap <= ceiling && gap * 10 >= ceiling * 8, "gap {gap}");
                        now = not_before_ms;
                    }
                    other => panic!("attempt {attempt} must back off, got {other:?}"),
                }
            } else {
                match job.state {
                    medulla_jobs::JobState::Failed { reason, .. } => {
                        assert_eq!(reason, "INGEST_RETRIES_EXHAUSTED")
                    }
                    other => panic!("attempt 5 must dead-letter, got {other:?}"),
                }
            }
        }
        // No sixth delivery.
        let sixth = claim(
            &store,
            &call(now + 3_600_000),
            TaskClaim {
                worker_id: "worker-1".into(),
                grant_id: "grant-1".into(),
                lease_ms: 30_000,
            },
            &admission(&["worker-1"]),
            &Ok(default_policy()),
        );
        assert!(sixth.error.is_none());
        let load = store.task_queue_load("acme", "ingest").unwrap();
        assert_eq!((load.queued, load.running), (0, 0));
    }

    // spec: MEDULLA-INGEST-WORKER-R001.4.1
    #[test]
    fn ingest_task_keeps_its_digest_id_and_reserved_queue_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_dev_store_in_dir(dir.path()).unwrap();
        let envelope = IngestJobEnvelope {
            job_id: "job-1".into(),
            tenant: "acme".into(),
            corpus_key: "repo-a".into(),
            kind: IngestJobKind::Document,
            target: "docs/readme.md".into(),
            attempt: 1,
            page: None,
        };
        let digest = task_id(&envelope);
        // The write the jobs seam performs for `IngestEnqueue`.
        let outcome = store
            .submit_task(
                TaskSubmit {
                    tenant: "acme".into(),
                    actor: "principal:test".into(),
                    tenant_capability: tenant_capability("acme"),
                    queue: "ingest".into(),
                    kind: "document".into(),
                    payload_ref: serde_json::to_string(&envelope).unwrap(),
                    idempotency_key: digest.clone(),
                    priority: medulla_work_types::task_queue::default_priority(),
                    not_before_ms: 0,
                    job_id: Some(digest.clone()),
                    max_attempts: Some(u32::MAX),
                },
                1_000,
            )
            .unwrap();
        match outcome {
            TaskSubmitOutcome::Enqueued(job) => assert_eq!(job.job_id, digest),
            TaskSubmitOutcome::Duplicate => panic!("first write is not a duplicate"),
        }
        let claimed = claim(
            &store,
            &call(2_000),
            TaskClaim {
                worker_id: "worker-1".into(),
                grant_id: "grant-1".into(),
                lease_ms: 30_000,
            },
            &admission(&["worker-1"]),
            &Ok(default_policy()),
        );
        assert!(claimed.error.is_none(), "claim refused: {:?}", claimed.error);
        let leased = store
            .task_queue_load("acme", "ingest")
            .expect("queue load readable");
        assert_eq!((leased.queued, leased.running), (0, 1));
        let acked = ack(
            &store,
            &call(3_000),
            TaskAck {
                task_id: digest.clone(),
                worker_id: "worker-1".into(),
                outcome: TaskOutcome::Succeeded,
                detail: String::new(),
            },
            &Ok(default_policy()),
        );
        assert!(
            acked.error.is_none(),
            "the claimed id is the receipt id: {:?}",
            acked.error
        );

        // The generic enqueue refuses the reserved queue and writes nothing.
        let refused = enqueue(
            &store,
            &call(4_000),
            TaskEnqueue {
                queue: "ingest".into(),
                kind: "document".into(),
                payload_ref: "{}".into(),
                idempotency_key: "k".into(),
                priority: medulla_work_types::task_queue::default_priority(),
                not_before_ms: 0,
            },
        );
        assert!(refused.error.is_some());
        let detail = format!("{:?}", refused);
        assert!(detail.contains("INVALID_ARGUMENT"), "{detail}");
        assert!(detail.contains("written only by IngestEnqueue"), "{detail}");
        let load = store.task_queue_load("acme", "ingest").unwrap();
        assert_eq!((load.queued, load.running), (0, 0));
    }
}
