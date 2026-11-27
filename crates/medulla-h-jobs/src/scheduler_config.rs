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

/// The commons graph that holds the downtime selections.
const COMMONS: &str = "__commons__";

/// Page the `dw:Selection` nodes of the commons graph, newest first (their ids sort that way),
/// with the entry nodes of each. With no commons graph or no selection the page is empty.
async fn downtime_selections(
    state: &Arc<RwLock<ServerState>>,
    after: Option<String>,
    limit: u32,
) -> Result<ResultPayload, String> {
    if !(1..=100).contains(&limit) {
        return Err(format!(
            "INVALID_PARAMS: the downtime selection page limit {limit} is outside [1, 100]"
        ));
    }
    let (core, persist_dir) = {
        let current = state.read().await;
        (
            current.registry.get(COMMONS).map(|entry| entry.core.clone()),
            current.persist_dir.clone(),
        )
    };
    let mut page = core
        .map(|core| selection_page(&core, after.as_deref(), limit as usize))
        .unwrap_or_default();
    // The job observation is read now and never stored on the selection node.
    for selection in &mut page.selections {
        for chosen in &mut selection.chosen {
            chosen.job = crate::jobs::observe_downtime_job(persist_dir.as_deref(), &chosen.job_id);
        }
    }
    ResultPayload::of::<contract::DowntimeSelections>(page)
}

fn node_json(bytes: &[u8]) -> serde_json::Value {
    rmp_serde::from_slice(bytes).unwrap_or(serde_json::Value::Null)
}

/// One page of the stored downtime selections, after `after`, at most `limit` of them; the page names where the next one starts when more remain.
pub fn selection_page(
    core: &medulla_graph_core::graph::GraphCore,
    after: Option<&str>,
    limit: usize,
) -> medulla_work_types::downtime::DowntimeSelections {
    use medulla_work_types::downtime::{
        DowntimeSelected, DowntimeSelectionRecord, DowntimeSkipReason, DowntimeSkipped,
        DowntimeSelections,
    };
    let mut rows = core.get_nodes_by_label_page("dw:Selection", after, limit + 1);
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next_after = if more {
        rows.last().map(|(id, _)| id.clone())
    } else {
        None
    };
    let selections = rows
        .into_iter()
        .map(|(selection_id, bytes)| {
            let props = node_json(&bytes);
            let count = props["dw:entryCount"].as_u64().unwrap_or(0);
            let mut record = DowntimeSelectionRecord {
                at_ms: props["dw:atMs"].as_i64().unwrap_or(0),
                selection_id: selection_id.clone(),
                chosen: Vec::new(),
                skipped: Vec::new(),
            };
            for index in 0..count {
                let Some(entry) = core.get_node_properties(&format!("{selection_id}/{index}"))
                else {
                    continue;
                };
                let entry = node_json(&entry);
                let text = |key: &str| entry[key].as_str().unwrap_or_default().to_string();
                if entry["dw:outcome"] == "chosen" {
                    record.chosen.push(DowntimeSelected {
                        iri: text("dw:jobType"),
                        intent: text("dw:intent"),
                        job_id: text("dw:jobId"),
                        job: None,
                    });
                } else if let Some(reason) = DowntimeSkipReason::from_name(&text("dw:skipReason"))
                {
                    record.skipped.push(DowntimeSkipped {
                        iri: text("dw:jobType"),
                        reason,
                    });
                }
            }
            record
        })
        .collect();
    DowntimeSelections {
        selections,
        next_after,
    }
}

/// Route one scheduler-settings method, or hand the method back when it is not
/// one of the two.
pub async fn try_handle(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    method: Method,
) -> Result<Response, Method> {
    let method = match method {
        // MEDULLA-DURABLE-KERNEL-R101.5: the recorded downtime selections, newest first.
        Method::DowntimeSelections { after, limit } => {
            return Ok(answer(req_id, downtime_selections(state, after, limit).await));
        }
        other => other,
    };
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
