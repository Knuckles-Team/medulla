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
        medulla_kernel::metrics::access_denied();
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
