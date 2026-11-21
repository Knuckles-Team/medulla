//! Served task-queue methods over the durable job store
//! (MEDULLA-DURABLE-KERNEL-R072).
//!
//! `TaskQueueEnqueue`, `TaskQueueClaim` and `TaskQueueAck` are a typed facade
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
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_protocol_types::result_contract::coordination as contract;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::{EngineErrorCode, ServerErrorCode};
use medulla_work_types::task_queue::{
    ClaimedTask, TaskAck, TaskAcked, TaskClaim, TaskEnqueue, TaskEnqueued, TaskOutcome,
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
fn tenant_capability(tenant: &str) -> String {
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

/// Serve `TaskQueueClaim`: lease the next ready task of the verified tenant, or
/// answer nil when none is ready. The lease length is the request's.
pub fn claim(store: &JobStore, call: &TaskQueueCall<'_>, request: TaskClaim) -> Response {
    let req_id = call.req_id;
    if let Err(refusal) = request.validate() {
        return Response::err(req_id, refusal.to_string());
    }
    if !worker_field_valid(&request.worker_id) || !worker_field_valid(&request.grant_id) {
        return invalid(req_id, "task worker_id and grant_id must be 1 to 256 bytes");
    }
    let claimed = store.claim_task(
        &worker_ref(call, &request.worker_id),
        &tenant_capability(call.tenant),
        call.now_ms,
        request.lease_ms,
    );
    match claimed {
        Ok(claim) => answer(
            req_id,
            ResultPayload::of::<contract::TaskQueueClaim>(claim.map(|claim| {
                ClaimedTask {
                    queue: claim
                        .job
                        .algo
                        .family
                        .strip_prefix(TASK_FAMILY_PREFIX)
                        .unwrap_or_default()
                        .to_string(),
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
                }
            })),
        ),
        Err(error) => store_refusal(req_id, &error),
    }
}

/// Serve `TaskQueueAck`: complete, fail or release a task the caller's worker
/// holds under a live lease.
pub fn ack(store: &JobStore, call: &TaskQueueCall<'_>, request: TaskAck) -> Response {
    let req_id = call.req_id;
    if let Err(refusal) = request.validate() {
        return Response::err(req_id, refusal.to_string());
    }
    if !worker_field_valid(&request.worker_id) {
        return invalid(req_id, "task worker_id must be 1 to 256 bytes");
    }
    let outcome = match request.outcome {
        TaskOutcome::Succeeded => TaskAckOutcome::Succeeded,
        TaskOutcome::Failed => TaskAckOutcome::Failed,
        TaskOutcome::Retry => TaskAckOutcome::Retry,
    };
    let acked = store.ack_task(
        call.tenant,
        &request.task_id,
        &worker_ref(call, &request.worker_id),
        outcome,
        &request.detail,
        call.now_ms,
    );
    match acked {
        Ok(job) => answer(
            req_id,
            ResultPayload::of::<contract::TaskQueueAck>(TaskAcked {
                task_id: job.job_id,
                outcome: request.outcome,
            }),
        ),
        Err(TaskAckError::NotClaimed) => Response::err(
            req_id,
            format!(
                "{}: the task is not leased to this worker",
                ServerErrorCode::TaskAckNotClaimed.as_str()
            ),
        ),
        Err(TaskAckError::LeaseExpired) => Response::err(
            req_id,
            format!(
                "{}: the lease lapsed before the ack",
                ServerErrorCode::TaskAckLeaseExpired.as_str()
            ),
        ),
        Err(TaskAckError::Store(error)) => store_refusal(req_id, &error),
    }
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
        } => claim(
            &store,
            &call,
            TaskClaim {
                worker_id,
                grant_id,
                lease_ms,
            },
        ),
        Method::TaskQueueAck {
            task_id,
            worker_id,
            outcome,
            detail,
        } => ack(
            &store,
            &call,
            TaskAck {
                task_id,
                worker_id,
                outcome,
                detail,
            },
        ),
        other => return Err(other),
    })
}
