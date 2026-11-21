//! Served worker registry (MEDULLA-DURABLE-KERNEL-R074.2).
//!
//! `WorkerRegister`, `WorkerHeartbeat` and `AdmitWorkerTask` drive the [`OwnedWorkerRegistry`] held in
//! `ServerState`. The registry is in-memory: a restart empties it and workers
//! register again. The owner of a worker is the verified principal; another
//! principal is refused `ACCESS_DENIED`. A heartbeat for an unknown worker answers
//! `WORKER_UNKNOWN`, and a worker whose last heartbeat is older than the policy's
//! `worker_ttl_ms` is expired. Both methods are gated by `worker:register` before
//! this handler runs. The handler takes an injected [`Clock`].

use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_persistence::scheduler_config_store::shared_scheduler_config_store;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_protocol_types::result_contract::coordination as contract;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::Digest256;
use medulla_types::contract::{EngineErrorCode, ServerErrorCode};
use medulla_work_types::worker_registry::{
    LaneLoad, OwnedWorkerError, OwnedWorkerRegistry, WorkerRecord, WorkerRegistration, WorkerRegistryError,
};
use parking_lot::Mutex;
use tokio::sync::RwLock;

/// The time source of the handler, in milliseconds.
pub trait Clock: Send + Sync {
    /// The current engine time.
    fn now_ms(&self) -> u64;
}

/// The engine clock: the authoritative time of the request being served.
#[derive(Clone, Copy, Debug, Default)]
pub struct EngineClock;

impl Clock for EngineClock {
    fn now_ms(&self) -> u64 {
        medulla_kernel::kernel::dispatch_support::authoritative_now_ms()
    }
}

/// The wire code of one registry refusal.
pub fn refusal_code(error: &OwnedWorkerError) -> &'static str {
    match error {
        OwnedWorkerError::NotOwner => EngineErrorCode::AccessDenied.as_str(),
        OwnedWorkerError::Registry(inner) => match inner {
            WorkerRegistryError::AlreadyRegistered => {
                ServerErrorCode::WorkerAlreadyRegistered.as_str()
            }
            WorkerRegistryError::UnknownWorker => ServerErrorCode::WorkerUnknown.as_str(),
            WorkerRegistryError::ExpiredWorker => ServerErrorCode::WorkerExpired.as_str(),
            WorkerRegistryError::ClockRegression => {
                ServerErrorCode::WorkerClockRegression.as_str()
            }
            WorkerRegistryError::ZeroTtl | WorkerRegistryError::DeadlineOverflow => {
                ServerErrorCode::WorkerTtlInvalid.as_str()
            }
            WorkerRegistryError::EmptyWorkerId
            | WorkerRegistryError::ZeroCores
            | WorkerRegistryError::ZeroMemory
            | WorkerRegistryError::NoSupportedLanes
            | WorkerRegistryError::EmptyLaneId => {
                ServerErrorCode::WorkerRegistrationInvalid.as_str()
            }
        },
    }
}

fn refusal(req_id: u64, error: &OwnedWorkerError) -> Response {
    if matches!(error, OwnedWorkerError::NotOwner) {
        medulla_kernel::metrics::process().access_denied();
    }
    Response::err(
        req_id,
        format!("{}: worker registry refused the call", refusal_code(error)),
    )
}

fn answered<M>(req_id: u64, outcome: Result<WorkerRecord, OwnedWorkerError>) -> Response
where
    M: medulla_protocol_types::result_contract::MethodResult<Body = WorkerRecord>,
{
    match outcome {
        Ok(record) => match ResultPayload::of::<M>(record) {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        },
        Err(error) => refusal(req_id, &error),
    }
}

/// Serve `WorkerRegister` for `owner` with a worker lease of `ttl_ms`.
pub fn register(
    registry: &Mutex<OwnedWorkerRegistry>,
    clock: &dyn Clock,
    req_id: u64,
    owner: &str,
    ttl_ms: u64,
    registration: WorkerRegistration,
) -> Response {
    let outcome = registry
        .lock()
        .register(owner, registration, clock.now_ms(), ttl_ms);
    answered::<contract::WorkerRegister>(req_id, outcome)
}

/// Serve `WorkerHeartbeat` for `owner` with a worker lease of `ttl_ms`.
pub fn heartbeat(
    registry: &Mutex<OwnedWorkerRegistry>,
    clock: &dyn Clock,
    req_id: u64,
    owner: &str,
    ttl_ms: u64,
    worker_id: &str,
) -> Response {
    let outcome = registry
        .lock()
        .heartbeat(owner, worker_id, clock.now_ms(), ttl_ms);
    answered::<contract::WorkerHeartbeat>(req_id, outcome)
}

/// The worker lease length in force: the stored scheduler policy, else the startup
/// knob value.
async fn worker_ttl_ms(state: &Arc<RwLock<ServerState>>) -> u64 {
    let store = {
        let current = state.read().await;
        shared_scheduler_config_store(&current)
    };
    if let Ok(store) = store {
        if let Ok(config) = crate::scheduler_config::effective_config(&store) {
            return config.worker_ttl_ms;
        }
    }
    medulla_server_api::knob_config::knobs().scheduler_worker_ttl_ms
}

/// Serve `AdmitWorkerTask` (MEDULLA-PLACEMENT-R002.4): the reserved-worker admission
/// for a live worker of the verified principal, sized from the stored scheduler
/// policy, the shard-writer width and the engine's own per-lane task load. The
/// durable job store is the one source of lane load; nothing is cached.
async fn admit(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified_context: &VerifiedRequestContext,
    worker_id: &str,
    requested_max: u32,
) -> Response {
    let tenant = verified_context.tenant();
    let principal = verified_context.principal();
    let (config_store, persistence, registry) = {
        let current = state.read().await;
        (
            shared_scheduler_config_store(&current),
            current.persistence.clone(),
            current.worker_registry.clone(),
        )
    };
    let config = match config_store
        .and_then(|store| crate::scheduler_config::effective_config(&store))
    {
        Ok(config) => config,
        Err(error) => return Response::err(req_id, error),
    };
    let shard_writer_width = match crate::worker_admission::shard_writer_width(tenant, &persistence)
    {
        Ok(width) => width.shard_writers,
        Err(error) => return Response::err(req_id, error),
    };
    let now_ms = EngineClock.now_ms();
    let job_store = match crate::jobs::outbox_job_store(state).await {
        Ok(store) => store,
        Err(error) => return Response::err(req_id, error),
    };
    let counts = match job_store.task_lane_counts(
        &crate::task_queue::tenant_capability(tenant),
        i64::try_from(now_ms).unwrap_or(i64::MAX),
    ) {
        Ok(counts) => counts,
        Err(error) => return Response::err(req_id, format!("INTERNAL: {error}")),
    };
    let lane_load: BTreeMap<String, LaneLoad> = counts
        .into_iter()
        .map(|(lane, (running, queued))| (lane, LaneLoad { running, queued }))
        .collect();
    let grant_id = match Digest256::framed(
        b"eg/worker-grant/v1",
        &[
            tenant.as_bytes(),
            principal.as_bytes(),
            worker_id.as_bytes(),
            &now_ms.to_be_bytes(),
        ],
    ) {
        Ok(digest) => format!("grant:{}", digest.to_hex()),
        Err(error) => return Response::err(req_id, format!("INTERNAL: {error}")),
    };
    let owned = registry.lock();
    if owned.get_live(worker_id, now_ms).is_some()
        && owned
            .owner_of(worker_id)
            .is_some_and(|owner| owner != principal)
    {
        return refusal(req_id, &OwnedWorkerError::NotOwner);
    }
    crate::worker_admission::admit_worker_task(crate::worker_admission::AdmitWorkerTaskInput {
        req_id,
        worker_id,
        requested_max,
        tenant,
        verified_tenant: tenant,
        now_ms,
        registry: owned.registry(),
        config: &config,
        lane_load: &lane_load,
        shard_writer_width,
        grant_id,
    })
}

/// Route one worker-registry method, or hand the method back when it is not one of
/// the three.
pub async fn try_handle(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified_context: &VerifiedRequestContext,
    method: Method,
) -> Result<Response, Method> {
    if !matches!(
        method,
        Method::WorkerRegister { .. }
            | Method::WorkerHeartbeat { .. }
            | Method::AdmitWorkerTask { .. }
    ) {
        return Err(method);
    }
    if let Method::AdmitWorkerTask {
        worker_id,
        requested_max,
    } = &method
    {
        return Ok(admit(state, req_id, verified_context, worker_id, *requested_max).await);
    }
    let ttl_ms = worker_ttl_ms(state).await;
    let registry = state.read().await.worker_registry.clone();
    let owner = verified_context.principal();
    Ok(match method {
        Method::WorkerRegister { registration } => {
            register(&registry, &EngineClock, req_id, owner, ttl_ms, registration)
        }
        Method::WorkerHeartbeat { worker_id } => {
            heartbeat(&registry, &EngineClock, req_id, owner, ttl_ms, &worker_id)
        }
        other => return Err(other),
    })
}
