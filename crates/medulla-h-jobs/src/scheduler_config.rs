//! Served scheduler settings (MEDULLA-DURABLE-KERNEL-R076.2).
//!
//! `GetSchedulerConfig` answers the policy in force: the stored setting, or the
//! policy built from the startup knobs when none was stored. `SetSchedulerConfig`
//! validates a whole policy with `SchedulerConfig::validate` and stores it in the
//! control-plane owner file `scheduler_config.redb`; a refused setting writes nothing and the policy in force is
//! unchanged. Both are gated by `scheduler:admin` before this handler runs.
//! The lane-registry membership check of MEDULLA-INGEST-WORKER-R604.1 joins
//! `validate` when that registry lands; until then a lane id is checked for
//! blankness and for fitting inside the worker pool.

use std::sync::Arc;

use medulla_persistence::scheduler_config_store::{
    shared_scheduler_config_store, SchedulerConfigStore,
};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_protocol_types::result_contract::coordination as contract;
use medulla_server_api::state::ServerState;
use medulla_types::contract::ServerErrorCode;
use medulla_work_types::scheduler_config::SchedulerConfig;
use tokio::sync::RwLock;

/// The policy in force: the stored one, else the policy of the startup knobs.
pub fn effective_config(store: &SchedulerConfigStore) -> Result<SchedulerConfig, String> {
    let stored = store.get().map_err(|error| format!("INTERNAL: {error}"))?;
    match stored {
        Some(json) => serde_json::from_str(&json)
            .map_err(|_| "INTERNAL: the stored scheduler policy is unreadable".to_string()),
        None => medulla_server_api::knob_config::scheduler_config_from(
            medulla_server_api::knob_config::knobs(),
        )
        .map_err(|error| {
            format!(
                "{}: {error}",
                ServerErrorCode::SchedulerConfigInvalid.as_str()
            )
        }),
    }
}

/// Validate and store `config`; a refused policy writes nothing.
pub fn apply_config(
    store: &SchedulerConfigStore,
    config: SchedulerConfig,
) -> Result<SchedulerConfig, String> {
    config.validate().map_err(|error| {
        format!(
            "{}: {error}",
            ServerErrorCode::SchedulerConfigInvalid.as_str()
        )
    })?;
    let json = serde_json::to_string(&config)
        .map_err(|_| "INTERNAL: the scheduler policy could not be encoded".to_string())?;
    store
        .put(&json)
        .map_err(|error| format!("INTERNAL: {error}"))?;
    Ok(config)
}

/// Serve `GetSchedulerConfig`.
pub fn get(store: &SchedulerConfigStore, req_id: u64) -> Response {
    answer(
        req_id,
        effective_config(store).and_then(ResultPayload::of::<contract::GetSchedulerConfig>),
    )
}

/// Serve `SetSchedulerConfig`.
pub fn set(store: &SchedulerConfigStore, req_id: u64, config: SchedulerConfig) -> Response {
    answer(
        req_id,
        apply_config(store, config).and_then(ResultPayload::of::<contract::SetSchedulerConfig>),
    )
}

fn answer(req_id: u64, payload: Result<ResultPayload, String>) -> Response {
    match payload {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

/// Route one scheduler-settings method, or hand the method back when it is not
/// one of the two.
pub async fn try_handle(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    method: Method,
) -> Result<Response, Method> {
    if matches!(method, Method::DowntimeSelections) {
        // MEDULLA-DURABLE-KERNEL-R101.3: the latest downtime tick's decision.
        return Ok(answer(
            req_id,
            ResultPayload::of::<contract::DowntimeSelections>(
                medulla_server_api::downtime::latest_selections(),
            ),
        ));
    }
    if !matches!(
        method,
        Method::GetSchedulerConfig | Method::SetSchedulerConfig { .. }
    ) {
        return Err(method);
    }
    let store = {
        let current = state.read().await;
        match shared_scheduler_config_store(&current) {
            Ok(store) => store,
            Err(error) => return Ok(Response::err(req_id, format!("INTERNAL: {error}"))),
        }
    };
    Ok(match method {
        Method::SetSchedulerConfig { config } => set(&store, req_id, config),
        _ => get(&store, req_id),
    })
}
