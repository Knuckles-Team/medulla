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

use medulla_types::scheduler_config::SchedulerConfig;
use medulla_work_types::worker_registry::{
    admit_reserved_worker_task, LaneLoad, WorkerAdmission, WorkerAdmissionRefusal, WorkerRegistry,
    WorkerSizingInput,
};

/// Served `AdmitWorkerTask` (MEDULLA-DURABLE-KERNEL-R075): the reserved-worker
/// admission — a `WorkerGrant` or a typed refusal — from the registry, the
/// scheduler config, the per-lane load and the R077 shard-writer width,
/// sized through `granted_slots` (MEDULLA-PLACEMENT-R002.3). Gated by
/// `scheduler:read` before this handler runs; here the body tenant must equal
/// the verified carrier tenant. A worker that is not live (unknown or expired)
/// is refused with `WORKER_UNKNOWN` before any sizing runs.
pub fn admit_worker_task(
    req_id: u64,
    worker_id: &str,
    requested_max: u32,
    tenant: &str,
    verified_tenant: &str,
    now_ms: u64,
    registry: &WorkerRegistry,
    config: &SchedulerConfig,
    lane_load: &BTreeMap<String, LaneLoad>,
    shard_writer_width: u32,
    grant_id: String,
) -> Response {
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
