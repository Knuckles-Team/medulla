//! Served worker registry (MEDULLA-DURABLE-KERNEL-R074.2).
//!
//! `WorkerRegister` and `WorkerHeartbeat` drive the [`OwnedWorkerRegistry`] held in
//! `ServerState`. The registry is in-memory: a restart empties it and workers
//! register again. The owner of a worker is the verified principal; another
//! principal is refused `ACCESS_DENIED`. A heartbeat for an unknown worker answers
//! `WORKER_UNKNOWN`, and a worker whose last heartbeat is older than the policy's
//! `worker_ttl_ms` is expired. Both methods are gated by `worker:register` before
//! this handler runs. The handler takes an injected [`Clock`].

use std::sync::Arc;

use medulla_persistence::scheduler_config_store::shared_scheduler_config_store;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_protocol_types::result_contract::coordination as contract;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::{EngineErrorCode, ServerErrorCode};
use medulla_work_types::worker_registry::{
    OwnedWorkerError, OwnedWorkerRegistry, WorkerRecord, WorkerRegistration, WorkerRegistryError,
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

/// Route one worker-registry method, or hand the method back when it is not one of
/// the two.
pub async fn try_handle(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified_context: &VerifiedRequestContext,
    method: Method,
) -> Result<Response, Method> {
    if !matches!(
        method,
        Method::WorkerRegister { .. } | Method::WorkerHeartbeat { .. }
    ) {
        return Err(method);
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
