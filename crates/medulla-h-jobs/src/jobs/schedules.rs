//! Owned schedule records served as `AnalyticsJob` ops (MEDULLA-PLACEMENT-R751.754).
//!
//! A schedule is a `Schedule` node in the tenant graph, written through the
//! ordinary `BatchUpdate` request boundary (so graph write access applies) and
//! read straight from the graph core. The jobs store stays the executor's
//! queue only. The owner check runs before any write or any validation of the
//! new body: a foreign principal is refused with `SCHEDULE_NOT_OWNER`.

use medulla_jobs::schedule::{
    ScheduleError, ScheduleRecord, ScheduleSpec, INVALID_SCHEDULE, SCHEDULE_EXISTS,
    SCHEDULE_NOT_FOUND,
};
use medulla_jobs::schedule_run::ScheduleBudget;
use medulla_protocol_types::protocol::Request;
use medulla_protocol_types::result_contract::coordination::{
    JobScheduleCreate, JobScheduleDelete, JobScheduleGet, JobScheduleList, JobSchedulePause,
    JobScheduleCalendar, JobScheduleResume, JobScheduleTicks, JobScheduleUpdate, ScheduleCalendar,
    ScheduleCalendarEntry, ScheduleCalendarLog, ScheduleRemoved, ScheduleTickList,
    ScheduleTickView, ScheduleView, ScheduleViewList,
};
use medulla_protocol_types::result_contract::MethodResult;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::CarrierAuthority;
use medulla_types::schedule_machine::{ScheduleEvent, ScheduleState};
use serde_json::{json, Map, Value};

use super::prelude_server::*;
use super::prelude_std::*;
use super::submit::resolve_submit_graph_core;
use super::unix_ms;

const NODE_TYPE: &str = "Schedule";
const BODY: &str = "schedule_body";

/// The caller, graph and request a schedule op runs under.
pub(super) struct ScheduleCall<'a> {
    pub state: &'a Arc<RwLock<ServerState>>,
    pub req_id: u64,
    pub authority: &'a CarrierAuthority,
    pub verified_context: &'a VerifiedRequestContext,
}

fn refuse(error: ScheduleError) -> String {
    error.to_string()
}

fn typed<M: MethodResult>(req_id: u64, result: Result<M::Body, String>) -> Response {
    match result.and_then(ResultPayload::of::<M>) {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

fn view(record: ScheduleRecord) -> Result<ScheduleView, String> {
    Ok(ScheduleView {
        spec: serde_json::to_value(&record.spec).map_err(|e| e.to_string())?,
        name: record.name,
        owner: record.owner,
        tenant: record.tenant,
        task_kind: record.task_kind,
        state: record.state,
        resumed_ms: record.resumed_ms,
        max_run_s: record.budget.max_run_s,
        max_tokens: record.budget.max_tokens,
        period_s: record.budget.period_s,
        exclusion_group: record.exclusion_group,
        min_spacing_s: record.min_spacing_s,
    })
}

/// The budget and exclusion group fields a create or update op carries.
#[derive(Default)]
pub(super) struct PolicyInput {
    pub max_run_s: Option<u64>,
    pub max_tokens: Option<u64>,
    pub period_s: Option<u64>,
    pub exclusion_group: Option<String>,
    pub min_spacing_s: Option<u64>,
}

impl PolicyInput {
    /// The budget: a given limit, else the record's current one, else the knob default.
    fn budget(&self, current: Option<ScheduleBudget>) -> ScheduleBudget {
        let knobs = medulla_server_api::knob_config::knobs();
        ScheduleBudget {
            max_run_s: self.max_run_s.unwrap_or_else(|| {
                current.map_or(knobs.jobs_schedule_budget_run_s, |b| b.max_run_s)
            }),
            max_tokens: self.max_tokens.unwrap_or_else(|| {
                current.map_or(knobs.jobs_schedule_budget_tokens, |b| b.max_tokens)
            }),
            period_s: self.period_s.unwrap_or_else(|| {
                current.map_or(knobs.jobs_schedule_budget_period_s, |b| b.period_s)
            }),
        }
    }
}

fn parse_spec(spec: Value) -> Result<ScheduleSpec, String> {
    serde_json::from_value(spec)
        .map_err(|e| refuse(ScheduleError::new(INVALID_SCHEDULE, e.to_string())))
}

fn load(
    core: &medulla_graph_core::graph::GraphCore,
    tenant: &str,
    name: &str,
) -> Result<ScheduleRecord, String> {
    let not_found = || format!("{SCHEDULE_NOT_FOUND}: {name}");
    let raw = core
        .get_node_properties(&ScheduleRecord::node_id(tenant, name))
        .ok_or_else(not_found)?;
    decode(&raw)
}

fn decode(raw: &[u8]) -> Result<ScheduleRecord, String> {
    let node = medulla_types::msgpack::decode_property_value(raw)
        .map_err(|e| format!("INTERNAL: schedule node unreadable: {e:?}"))?;
    let body = node
        .get(BODY)
        .and_then(Value::as_str)
        .ok_or_else(|| "INTERNAL: schedule node has no body".to_string())?;
    serde_json::from_str(body).map_err(|e| format!("INTERNAL: schedule body invalid: {e}"))
}

fn upsert_op(call: &ScheduleCall<'_>, record: &ScheduleRecord) -> Result<Value, String> {
    schedule_node_operation(record, call.verified_context.tenant())
}

/// The `upsert_node` operation that stores `record` as a `Schedule` node of `tenant`: the one
/// shape the schedule ops and the boot registration (MEDULLA-REPO-INGEST-R006.6) both write.
pub fn schedule_node_operation(record: &ScheduleRecord, tenant: &str) -> Result<Value, String> {
    let body = serde_json::to_string(record).map_err(|e| e.to_string())?;
    let mut properties = Map::new();
    properties.insert("type".into(), NODE_TYPE.into());
    properties.insert("name".into(), record.name.clone().into());
    properties.insert(BODY.into(), body.into());
    properties.insert("tenant_id".into(), tenant.into());
    properties.insert("_owner".into(), record.owner.clone().into());
    Ok(json!({
        "op": "upsert_node",
        "id": ScheduleRecord::node_id(&record.tenant, &record.name),
        "properties": properties,
    }))
}

async fn commit(call: &ScheduleCall<'_>, graph: &str, op: Value) -> Result<(), String> {
    let operations_msgpack = rmp_serde::to_vec_named(&vec![op])
        .map_err(|_| "schedule batch encoding failed".to_string())?;
    let response = medulla_server_api::api::reentry::dispatch_verified_request(
        call.state,
        Request {
            id: call.req_id,
            graph: graph.to_string(),
            auth_token: String::new(),
            agent_id: Some(call.authority.agent_id().to_string()),
            method: Method::BatchUpdate { operations_msgpack },
            trace_id: None,
        },
        call.verified_context.clone(),
    )
    .await;
    response.refusal_text().map_or(Ok(()), Err)
}

async fn core_of(
    call: &ScheduleCall<'_>,
    graph: &str,
) -> Result<Arc<medulla_graph_core::graph::GraphCore>, String> {
    resolve_submit_graph_core(call.state, call.req_id, call.authority, graph)
        .await
        .map_err(|response| response.refusal_text().unwrap_or_default())
}

pub(super) async fn create(
    call: ScheduleCall<'_>,
    graph: String,
    name: String,
    spec: Value,
    task_kind: String,
    policy: PolicyInput,
) -> Response {
    let result = async {
        let core = core_of(&call, &graph).await?;
        let tenant = call.verified_context.tenant().to_string();
        if core
            .get_node_properties(&ScheduleRecord::node_id(&tenant, &name))
            .is_some()
        {
            return Err(format!("{SCHEDULE_EXISTS}: {name}"));
        }
        let record = ScheduleRecord {
            name,
            // The owner's hashed principal scope: the key its fired jobs are owned by.
            owner: call.authority.actor_scope().to_string(),
            tenant,
            spec: parse_spec(spec)?,
            task_kind,
            state: ScheduleState::default(),
            resumed_ms: 0,
            budget: policy.budget(None),
            exclusion_group: policy.exclusion_group,
            min_spacing_s: policy.min_spacing_s.unwrap_or(0),
        };
        record.validate().map_err(refuse)?;
        commit(&call, &graph, upsert_op(&call, &record)?).await?;
        view(record)
    }
    .await;
    typed::<JobScheduleCreate>(call.req_id, result)
}

pub(super) async fn get(call: ScheduleCall<'_>, graph: String, name: String) -> Response {
    let result = async {
        let core = core_of(&call, &graph).await?;
        view(load(&core, call.verified_context.tenant(), &name)?)
    }
    .await;
    typed::<JobScheduleGet>(call.req_id, result)
}

pub(super) async fn update(
    call: ScheduleCall<'_>,
    graph: String,
    name: String,
    spec: Value,
    task_kind: String,
    policy: PolicyInput,
) -> Response {
    let result = async {
        let core = core_of(&call, &graph).await?;
        let existing = load(&core, call.verified_context.tenant(), &name)?;
        existing
            .authorize_write(call.authority.actor_scope())
            .map_err(refuse)?;
        let record = ScheduleRecord {
            spec: parse_spec(spec)?,
            task_kind,
            budget: policy.budget(Some(existing.budget)),
            exclusion_group: policy.exclusion_group.or(existing.exclusion_group.clone()),
            min_spacing_s: policy.min_spacing_s.unwrap_or(existing.min_spacing_s),
            ..existing
        };
        record.validate().map_err(refuse)?;
        commit(&call, &graph, upsert_op(&call, &record)?).await?;
        view(record)
    }
    .await;
    typed::<JobScheduleUpdate>(call.req_id, result)
}

pub(super) async fn delete(call: ScheduleCall<'_>, graph: String, name: String) -> Response {
    let result = async {
        let core = core_of(&call, &graph).await?;
        let existing = load(&core, call.verified_context.tenant(), &name)?;
        existing
            .authorize_write(call.authority.actor_scope())
            .map_err(refuse)?;
        let id = ScheduleRecord::node_id(&existing.tenant, &existing.name);
        commit(&call, &graph, json!({"op": "remove_node", "id": id})).await?;
        Ok(ScheduleRemoved {
            name: existing.name,
        })
    }
    .await;
    typed::<JobScheduleDelete>(call.req_id, result)
}

pub(super) async fn list(call: ScheduleCall<'_>, graph: String) -> Response {
    let result = async {
        let core = core_of(&call, &graph).await?;
        let prefix = ScheduleRecord::node_id(call.verified_context.tenant(), "");
        let ids = core
            .nodes_by_property("type", NODE_TYPE)
            .unwrap_or_else(|| core.node_ids());
        let mut records = Vec::new();
        for id in ids.iter().filter(|id| id.starts_with(&prefix)) {
            if let Some(raw) = core.get_node_properties(id) {
                records.push(decode(&raw)?);
            }
        }
        records.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(ScheduleViewList {
            schedules: records.into_iter().map(view).collect::<Result<_, _>>()?,
        })
    }
    .await;
    typed::<JobScheduleList>(call.req_id, result)
}

/// Pause or resume a schedule. The owner only; a repeated pause or resume answers with the
/// unchanged record and writes nothing, so both are idempotent. The state is written only
/// through `ScheduleState::apply`.
pub(super) async fn set_state(
    call: ScheduleCall<'_>,
    graph: String,
    name: String,
    event: ScheduleEvent,
) -> Response {
    let result = async {
        let core = core_of(&call, &graph).await?;
        let existing = load(&core, call.verified_context.tenant(), &name)?;
        existing
            .authorize_write(call.authority.actor_scope())
            .map_err(refuse)?;
        if existing.state == ScheduleState::target(event) {
            return view(existing);
        }
        let mut record = existing;
        record.transition(event).map_err(|e| e.to_string())?;
        if event == ScheduleEvent::Resume {
            record.resumed_ms = unix_ms() as u64;
        }
        commit(&call, &graph, upsert_op(&call, &record)?).await?;
        view(record)
    }
    .await;
    match event {
        ScheduleEvent::Pause => typed::<JobSchedulePause>(call.req_id, result),
        ScheduleEvent::Resume => typed::<JobScheduleResume>(call.req_id, result),
    }
}

/// Ticks one schedule read returns when the op names no limit.
const DEFAULT_TICK_LIMIT: u32 = 20;
/// The most ticks one read returns.
const MAX_TICK_LIMIT: u32 = 200;

fn tick_view(fields: &Value) -> Option<ScheduleTickView> {
    let text = |key: &str| {
        fields
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let number = |key: &str| fields.get(key).and_then(Value::as_u64).unwrap_or(0);
    let job = text("job_id");
    Some(ScheduleTickView {
        window_start_ms: fields.get("window_start_ms").and_then(Value::as_u64)?,
        outcome: text("outcome"),
        reason: text("reason"),
        host: text("host"),
        work_item_id: (!job.is_empty()).then_some(job),
        used_run_s: number("used_run_s"),
        used_tokens: number("used_tokens"),
    })
}

/// The last ticks of one schedule of the caller's tenant, oldest first. The schedule must
/// exist; a tick node belongs to it by its `schedule` and `tenant` fields.
pub(super) async fn ticks(
    call: ScheduleCall<'_>,
    graph: String,
    name: String,
    limit: Option<u32>,
) -> Response {
    let result = async {
        let core = core_of(&call, &graph).await?;
        let tenant = call.verified_context.tenant().to_string();
        load(&core, &tenant, &name)?;
        let prefix = format!("schedule-tick:{tenant}:{name}:");
        let mut found = Vec::new();
        for id in core.node_ids().iter().filter(|id| id.starts_with(&prefix)) {
            let Some(raw) = core.get_node_properties(id) else {
                continue;
            };
            let Ok(fields) = medulla_types::msgpack::decode_property_value(&raw) else {
                continue;
            };
            let ours = fields.get("schedule").and_then(Value::as_str) == Some(name.as_str())
                && fields.get("tenant").and_then(Value::as_str) == Some(tenant.as_str());
            if let Some(tick) = ours.then(|| tick_view(&fields)).flatten() {
                found.push(tick);
            }
        }
        found.sort_by_key(|tick| tick.window_start_ms);
        let keep = limit.unwrap_or(DEFAULT_TICK_LIMIT).clamp(1, MAX_TICK_LIMIT) as usize;
        let skip = found.len().saturating_sub(keep);
        Ok(ScheduleTickList {
            schedule: name,
            ticks: found.split_off(skip),
        })
    }
    .await;
    typed::<JobScheduleTicks>(call.req_id, result)
}

/// The longest range one calendar read covers.
const MAX_CALENDAR_RANGE_MS: u64 = 31 * 24 * 60 * 60 * 1_000;
/// Most entries (and, separately, logs) one calendar read returns.
const MAX_CALENDAR_ROWS: usize = 2_048;

/// The calendar of the caller's tenant over `[from_ms, to_ms)`: the windows each enabled
/// schedule fires in (a paused schedule fires none; its due windows record skipped ticks, which
/// the logs show) and the ticks already recorded in the range. Reads only the graph core; a
/// range over 31 days or an empty one refuses `INVALID_ARGUMENT`.
pub(super) async fn calendar(
    call: ScheduleCall<'_>,
    graph: String,
    from_ms: u64,
    to_ms: u64,
) -> Response {
    let result = async {
        if to_ms <= from_ms || to_ms - from_ms > MAX_CALENDAR_RANGE_MS {
            return Err("INVALID_ARGUMENT: a schedule calendar range is 1 ms to 31 days".to_string());
        }
        let core = core_of(&call, &graph).await?;
        let tenant = call.verified_context.tenant().to_string();
        let prefix = ScheduleRecord::node_id(&tenant, "");
        let ids = core
            .nodes_by_property("type", NODE_TYPE)
            .unwrap_or_else(|| core.node_ids());
        let mut records = Vec::new();
        for id in ids.iter().filter(|id| id.starts_with(&prefix)) {
            if let Some(raw) = core.get_node_properties(id) {
                records.push(decode(&raw)?);
            }
        }
        records.sort_by(|a, b| a.name.cmp(&b.name));
        let mut entries = Vec::new();
        let mut logs = Vec::new();
        for record in &records {
            if record.state == ScheduleState::Enabled {
                for at_ms in record.spec.due_windows(from_ms, to_ms).map_err(refuse)? {
                    entries.push(ScheduleCalendarEntry {
                        schedule_id: record.name.clone(),
                        at_ms,
                        agent_id: record.owner.clone(),
                    });
                }
            }
            let tick_prefix = format!("schedule-tick:{tenant}:{}:", record.name);
            for id in core.node_ids().iter().filter(|id| id.starts_with(&tick_prefix)) {
                let Some(raw) = core.get_node_properties(id) else {
                    continue;
                };
                let Ok(fields) = medulla_types::msgpack::decode_property_value(&raw) else {
                    continue;
                };
                let ours = fields.get("schedule").and_then(Value::as_str)
                    == Some(record.name.as_str())
                    && fields.get("tenant").and_then(Value::as_str) == Some(tenant.as_str());
                let Some(tick) = ours.then(|| tick_view(&fields)).flatten() else {
                    continue;
                };
                if (from_ms..to_ms).contains(&tick.window_start_ms) {
                    logs.push(ScheduleCalendarLog {
                        schedule_id: record.name.clone(),
                        at_ms: tick.window_start_ms,
                        outcome: tick.outcome,
                    });
                }
            }
        }
        entries.sort_by(|a, b| (a.at_ms, &a.schedule_id).cmp(&(b.at_ms, &b.schedule_id)));
        logs.sort_by(|a, b| (a.at_ms, &a.schedule_id).cmp(&(b.at_ms, &b.schedule_id)));
        entries.truncate(MAX_CALENDAR_ROWS);
        logs.truncate(MAX_CALENDAR_ROWS);
        Ok(ScheduleCalendar { entries, logs })
    }
    .await;
    typed::<JobScheduleCalendar>(call.req_id, result)
}
