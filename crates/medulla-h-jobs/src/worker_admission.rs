//! Served scheduler worker-admission reads (MEDULLA-DURABLE-KERNEL-R077).
//!
//! `GetShardWriterWidth` answers the configured durable shard-writer count `k`
//! for the verified tenant's graphs. `granted_slots`
//! (`medulla_work_types::worker_registry`) caps write-lane workers at it. The
//! method is gated by `scheduler:read` before this handler runs; here the body
//! tenant must equal the verified carrier tenant, and only the redb authority
//! holds shard writers, so any other backend fails closed.

use std::sync::Arc;

use medulla_persistence::AsRedb;
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_work_types::worker_registry::ShardWriterWidth;

const NATIVE_UNAVAILABLE: &str =
    "shard-writer width requires the native redb backend (no shard writers in this build)";

/// Answer `GetShardWriterWidth` for the verified carrier tenant.
pub fn answer_shard_writer_width(
    req_id: u64,
    tenant: &str,
    verified_tenant: &str,
    persistence: &Option<Arc<dyn PersistenceBackend>>,
) -> Response {
    if let Err(denied) =
        super::native_write_authority::require_carrier_tenant(tenant, verified_tenant)
    {
        medulla_kernel::metrics::process().access_denied();
        return Response::err(req_id, denied);
    }
    let payload = shard_writer_width(tenant, persistence).and_then(
        ResultPayload::of::<
            medulla_protocol_types::result_contract::coordination::GetShardWriterWidth,
        >,
    );
    match payload {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

/// The configured writer count of the durable backend, as `u32`.
pub fn shard_writer_width(
    tenant: &str,
    persistence: &Option<Arc<dyn PersistenceBackend>>,
) -> Result<ShardWriterWidth, String> {
    let backend = persistence
        .as_ref()
        .and_then(|backend| backend.as_redb())
        .ok_or_else(|| NATIVE_UNAVAILABLE.to_string())?;
    let shard_writers = u32::try_from(backend.shard_count())
        .map_err(|_| "shard-writer count exceeds u32".to_string())?;
    Ok(ShardWriterWidth {
        tenant: tenant.to_string(),
        shard_writers,
    })
}

use std::collections::BTreeMap;

use medulla_jobs::JobStore;
use medulla_persistence::scheduler_config_store::shared_scheduler_config_store;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_source_types::ingest_job::IngestLane;
use medulla_work_types::scheduler_config::SchedulerConfig;
use tokio::sync::RwLock;
use medulla_work_types::worker_registry::{
    admit_reserved_worker_task, LaneLoad, WorkerAdmission, WorkerAdmissionRefusal, WorkerRegistry,
    WorkerSizingInput,
};

/// The inputs of the one reserved-worker admission rule, built once per call so
/// `AdmitWorkerTask` and `TaskQueueClaim` never restate the rule or its inputs
/// (MEDULLA-INGEST-WORKER-R604.3).
pub struct AdmissionInputs {
    /// A snapshot of the live worker registry.
    pub registry: WorkerRegistry,
    /// The scheduler policy in force.
    pub config: SchedulerConfig,
    /// The load of each lane id: its ingest queue from `JobStore::task_queue_load`, plus the
    /// tasks of any other queue named like the lane (`JobStore::task_lane_counts`).
    pub lane_load: BTreeMap<String, LaneLoad>,
    /// The shard-writer width (MEDULLA-DURABLE-KERNEL-R077).
    pub shard_writer_width: u32,
}

fn saturate(count: u64) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Build the admission inputs of the verified tenant: the registry snapshot, the policy in force,
/// the shard-writer width and the lane load. The durable job store is the one source of lane
/// load; nothing is cached.
pub async fn admission_inputs(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    store: &JobStore,
) -> Result<AdmissionInputs, String> {
    let tenant = verified.tenant();
    let (config_store, persistence, registry) = {
        let current = state.read().await;
        (
            shared_scheduler_config_store(&current),
            current.persistence.clone(),
            current.worker_registry.clone(),
        )
    };
    let config_store = config_store.map_err(|error| format!("INTERNAL: {error}"))?;
    let config = crate::scheduler_config::effective_config(&config_store)?;
    let shard_writer_width = shard_writer_width(tenant, &persistence)?.shard_writers;
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let mut counts = store
        .task_lane_counts(
            &crate::task_queue::tenant_capability(tenant),
            i64::try_from(now_ms).unwrap_or(i64::MAX),
        )
        .map_err(|error| format!("INTERNAL: {error}"))?;
    // The ingest queues are counted once, by lane id, from their own totals.
    for lane in [IngestLane::General, IngestLane::Hydration] {
        counts.remove(lane.queue_name());
    }
    let mut lane_load: BTreeMap<String, LaneLoad> = counts
        .into_iter()
        .map(|(lane, (running, queued))| (lane, LaneLoad { running, queued }))
        .collect();
    for lane in [IngestLane::General, IngestLane::Hydration] {
        let load = store
            .task_queue_load(tenant, lane.queue_name())
            .map_err(|error| format!("INTERNAL: {error}"))?;
        let entry = lane_load.entry(lane.lane_id().as_str().to_string()).or_default();
        entry.running = entry.running.saturating_add(saturate(load.running));
        entry.queued = entry.queued.saturating_add(saturate(load.queued));
    }
    let registry = registry.lock().registry().clone();
    Ok(AdmissionInputs {
        registry,
        config,
        lane_load,
        shard_writer_width,
    })
}

/// The inputs of one served `AdmitWorkerTask` call.
pub struct AdmitWorkerTaskInput<'a> {
    /// The request id echoed in the response.
    pub req_id: u64,
    /// The reserved worker asking for a task grant.
    pub worker_id: &'a str,
    /// The most slots the worker asks for.
    pub requested_max: u32,
    /// The tenant named in the request body.
    pub tenant: &'a str,
    /// The tenant of the verified carrier; must equal `tenant`.
    pub verified_tenant: &'a str,
    /// Engine time of the admission, in milliseconds.
    pub now_ms: u64,
    /// The live worker registry.
    pub registry: &'a WorkerRegistry,
    /// The scheduler configuration used for sizing.
    pub config: &'a SchedulerConfig,
    /// The current load per lane.
    pub lane_load: &'a BTreeMap<String, LaneLoad>,
    /// The shard-writer width (MEDULLA-DURABLE-KERNEL-R077).
    pub shard_writer_width: u32,
    /// The id given to the grant when admission succeeds.
    pub grant_id: String,
}

/// Served `AdmitWorkerTask` (MEDULLA-DURABLE-KERNEL-R075): the reserved-worker
/// admission — a `WorkerGrant` or a typed refusal — from the registry, the
/// scheduler config, the per-lane load and the R077 shard-writer width,
/// sized through `granted_slots` (MEDULLA-PLACEMENT-R002.3). Gated by
/// `worker:register` before this handler runs (MEDULLA-PLACEMENT-R002.4); here the body tenant must equal
/// the verified carrier tenant. A worker that is not live (unknown or expired)
/// is refused with `WORKER_UNKNOWN` before any sizing runs.
pub fn admit_worker_task(input: AdmitWorkerTaskInput<'_>) -> Response {
    let AdmitWorkerTaskInput {
        req_id,
        worker_id,
        requested_max,
        tenant,
        verified_tenant,
        now_ms,
        registry,
        config,
        lane_load,
        shard_writer_width,
        grant_id,
    } = input;
    if let Err(denied) =
        super::native_write_authority::require_carrier_tenant(tenant, verified_tenant)
    {
        medulla_kernel::metrics::process().access_denied();
        return Response::err(req_id, denied);
    }
    let decision = match registry.get_live(worker_id, now_ms) {
        Some(record) => admit_reserved_worker_task(
            record,
            config,
            lane_load,
            WorkerSizingInput {
                request_ceiling: Some(requested_max),
                available_memory_bytes: record.memory_bytes,
                // The engine classifies the selected lane from the lane floors.
                is_write_lane: false,
            },
            shard_writer_width,
            grant_id,
        ),
        None => WorkerAdmission::Refused(WorkerAdmissionRefusal::WorkerUnknown),
    };
    let payload = medulla_protocol_types::protocol::ResultPayload::of::<
        medulla_protocol_types::result_contract::coordination::AdmitWorkerTask,
    >(decision);
    match payload {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}
