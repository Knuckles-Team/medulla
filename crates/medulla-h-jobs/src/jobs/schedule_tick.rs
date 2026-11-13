//! The executor's schedule tick (MEDULLA-PLACEMENT-R751.753).
//!
//! One task beside the worker pool wakes every `jobs.schedule_poll_s` seconds. Per graph holding
//! `Schedule` nodes it contends for the leader lease record (compare-and-set on its revision,
//! `MEDULLA-PLACEMENT-R751.761`), and only the leader computes due windows. Each fresh window
//! is recorded in the `TickLedger`, checked under `DaemonLeadership::authorize`, submitted as the
//! job of the schedule's task kind (`MEDULLA-PLACEMENT-R751.762`) and acknowledged by a
//! `ScheduleTick` node. The node id carries the window, so a tick replayed by a later leader
//! finds its acknowledgement and submits nothing.
//!
//! Before it submits, each window is decided by `medulla_jobs::schedule_run::decide_window`
//! (R751.755 to R751.757, R751.760): a paused schedule, a window missed while no leader acted,
//! a spent budget, a still-active run of the schedule or of its exclusion group, or a window
//! inside the group spacing records a `skipped` tick with its reason and submits nothing. The
//! tick node and the schedule's `ScheduleRun` record (cursor, last fired window, active work
//! item, budget usage) are written in one batch, so two leaders cannot both fire a window.

use medulla_graph_core::graph::GraphCore;
use medulla_jobs::leader_lease::{contend, LeaseCell, LeaseRecord, LEASE_NODE_ID};
use medulla_jobs::schedule::{
    job_for_task_kind, ScheduleRecord, ScheduleTick, TickLedger, TickStatus,
};
use medulla_jobs::schedule_run::{
    decide_window, finished_usage, ScheduleRun, TickOutcome, WindowFacts,
};
use medulla_protocol_types::protocol::Request;
use serde_json::{json, Value};

use super::prelude_jobs::*;
use super::prelude_server::*;
use super::prelude_std::*;
use super::*;

const SERVICE: &str = "scheduler";
const TICK_NODE_TYPE: &str = "ScheduleTick";
/// Longest time a cached schedule list is kept without a re-read.
const SCHEDULE_REREAD_MS: u64 = 60_000;

/// The graph node id of one acknowledged tick.
pub(super) fn tick_node_id(tenant: &str, name: &str, window_ms: u64) -> String {
    format!("schedule-tick:{tenant}:{name}:{window_ms}")
}

fn decode_node(core: &GraphCore, id: &str) -> Option<Value> {
    let raw = core.get_node_properties(id)?;
    medulla_types::msgpack::decode_property_value(&raw).ok()
}

/// The lease record of one graph, compared and set on its revision.
struct GraphLeaseCell<'a> {
    core: &'a GraphCore,
}

impl LeaseCell for GraphLeaseCell<'_> {
    fn read(&self) -> LeaseRecord {
        decode_node(self.core, LEASE_NODE_ID)
            .and_then(|fields| LeaseRecord::from_fields(&fields))
            .unwrap_or_default()
    }

    fn compare_and_set(&self, expected_revision: u64, next: &LeaseRecord) -> bool {
        let mut conditions = serde_json::Map::new();
        conditions.insert("revision".into(), expected_revision.into());
        self.core
            .compare_and_set_fields(LEASE_NODE_ID, &conditions, &next.to_fields())
    }
}

#[derive(Default)]
struct ScheduleCache {
    by_graph: BTreeMap<String, (u64, u64, Vec<ScheduleRecord>)>,
    /// Graphs whose due windows this engine has followed without a gap. A graph not in the set
    /// (a new process, or a pass that was due while another engine led) is scanned from each
    /// schedule's cursor, so the windows nobody acted on are recorded as missed.
    followed: std::collections::BTreeSet<String>,
}

impl ScheduleCache {
    /// The schedules of `graph`, re-read when the graph changed or the list is a minute old.
    fn schedules(&mut self, graph: &str, core: &GraphCore, now_ms: u64) -> Vec<ScheduleRecord> {
        let version = core.version();
        if let Some((read_at, seen, list)) = self.by_graph.get(graph) {
            if *seen == version && now_ms < read_at + SCHEDULE_REREAD_MS {
                return list.clone();
            }
        }
        let mut list = Vec::new();
        for id in core
            .nodes_by_property("type", "Schedule")
            .unwrap_or_default()
        {
            if let Some(record) = decode_node(core, &id)
                .and_then(|node| node.get("schedule_body")?.as_str().map(str::to_string))
                .and_then(|body| serde_json::from_str::<ScheduleRecord>(&body).ok())
            {
                list.push(record);
            }
        }
        self.by_graph
            .insert(graph.to_string(), (now_ms, version, list.clone()));
        list
    }
}

/// Start the tick task once per process.
pub(super) fn ensure_schedule_tick(state: Arc<RwLock<ServerState>>) {
    tokio::spawn(async move {
        let host = format!("engine:{}:{}", std::process::id(), unix_ms());
        let mut cache = ScheduleCache::default();
        let mut last_ms = unix_ms() as u64;
        loop {
            let poll_s = medulla_server_api::knob_config::knobs().jobs_schedule_poll_s;
            tokio::time::sleep(Duration::from_secs(poll_s)).await;
            let now = unix_ms() as u64;
            let from = std::mem::replace(&mut last_ms, now + 1);
            if let Err(error) = tick_pass(&state, &host, &mut cache, from, now, poll_s).await {
                eprintln!("schedule tick pass failed: {error}");
            }
        }
    });
}

async fn tick_pass(
    state: &Arc<RwLock<ServerState>>,
    host: &str,
    cache: &mut ScheduleCache,
    from_ms: u64,
    now_ms: u64,
    poll_s: u64,
) -> Result<(), String> {
    let graphs: Vec<(String, Arc<GraphCore>)> = {
        let s = state.read().await;
        s.registry
            .all_entries()
            .into_iter()
            .map(|entry| (entry.name.clone(), entry.core.clone()))
            .collect()
    };
    let store = outbox_job_store(state).await?;
    let lookback_ms = medulla_server_api::knob_config::knobs()
        .jobs_schedule_missed_lookback_s
        .saturating_mul(1_000);
    for (graph, core) in graphs {
        let schedules = cache.schedules(&graph, &core, now_ms);
        let mut due = Vec::new();
        for record in &schedules {
            let run = load_run(&core, record);
            let start = match run.cursor_ms {
                Some(cursor) if !cache.followed.contains(&graph) => cursor + 1,
                _ => from_ms,
            }
            .max(from_ms.saturating_sub(lookback_ms));
            if let Ok(windows) = record.spec.due_windows(start, now_ms + 1) {
                due.extend(windows.into_iter().map(|window| (window, record.clone())));
            }
        }
        if due.is_empty() {
            cache.followed.insert(graph);
            continue;
        }
        due.sort_by(|a, b| (a.0, &a.1.name).cmp(&(b.0, &b.1.name)));
        if decode_node(&core, LEASE_NODE_ID).is_none() {
            let fields = LeaseRecord::default().to_fields();
            commit(state, &graph, vec![upsert(LEASE_NODE_ID, fields)]).await?;
        }
        let cell = GraphLeaseCell { core: &core };
        let ttl_ms = poll_s.max(1) * 3_000;
        let Some(fence) = contend(&cell, host, now_ms, ttl_ms).map_err(|e| format!("{e:?}"))?
        else {
            cache.followed.remove(&graph);
            continue;
        };
        cache.followed.insert(graph.clone());
        for (window, record) in due {
            let tick = ScheduleTick {
                schedule_id: ScheduleRecord::node_id(&record.tenant, &record.name),
                window_start_ms: window,
            };
            let tick_id = tick_node_id(&record.tenant, &record.name, window);
            let mut ledger = TickLedger::default();
            if core.get_node_properties(&tick_id).is_some() {
                ledger.record(&tick).map_err(|e| e.to_string())?;
                ledger.acknowledge(&tick);
            }
            if !matches!(ledger.record(&tick), Ok(TickStatus::Fresh)) {
                continue;
            }
            cell.read()
                .leadership()
                .authorize(host, fence, now_ms)
                .map_err(|e| e.to_string())?;
            let mut run = load_run(&core, &record);
            settle(&mut run, &store);
            run.roll_period(window, &record.budget);
            let (group_run_active, group_last_fired_ms) =
                group_facts(&core, &store, &record, &schedules);
            let facts = WindowFacts {
                window_ms: window,
                live_from_ms: from_ms,
                own_run_active: run
                    .active_job
                    .as_deref()
                    .is_some_and(|job| job_active(&store, job)),
                group_run_active,
                group_last_fired_ms,
            };
            let outcome = decide_window(&record, &run, &facts);
            let mut job_id = String::new();
            if outcome == TickOutcome::Fired {
                job_id = submit_scheduled_job(state, &graph, &record, window).await?;
                run.active_job = Some(job_id.clone());
                run.last_fired_ms = Some(window);
            }
            run.cursor_ms = Some(window);
            ledger.acknowledge(&tick);
            let mut fields = serde_json::Map::new();
            fields.insert("type".into(), TICK_NODE_TYPE.into());
            fields.insert("schedule".into(), record.name.clone().into());
            fields.insert("tenant".into(), record.tenant.clone().into());
            fields.insert("window_start_ms".into(), window.into());
            fields.insert("host".into(), host.into());
            fields.insert("outcome".into(), outcome.as_str().into());
            fields.insert("reason".into(), outcome.reason().into());
            fields.insert("job_id".into(), job_id.into());
            let run_fields = run.to_fields(&record.tenant, &record.name);
            let run_id = ScheduleRun::node_id(&record.tenant, &record.name);
            commit(
                state,
                &graph,
                vec![upsert(&tick_id, fields), upsert(&run_id, run_fields)],
            )
            .await?;
        }
    }
    Ok(())
}

/// The run record of a schedule, the initial one when none is stored.
fn load_run(core: &GraphCore, record: &ScheduleRecord) -> ScheduleRun {
    let id = ScheduleRun::node_id(&record.tenant, &record.name);
    ScheduleRun::from_fields(decode_node(core, &id).as_ref())
}

/// Whether the work item is still active. A work item the store no longer holds is finished.
fn job_active(store: &JobStore, job_id: &str) -> bool {
    store
        .get(job_id)
        .map(|job| !job.state.is_terminal())
        .unwrap_or(false)
}

/// Charge the schedule for its last work item once it has finished.
fn settle(run: &mut ScheduleRun, store: &JobStore) {
    let Some(job_id) = run.active_job.clone() else {
        return;
    };
    match store.get(&job_id) {
        Ok(job) => {
            if let Some(usage) = finished_usage(&job) {
                run.charge(usage);
            }
        }
        Err(_) => run.active_job = None,
    }
}

/// Whether another member of the schedule's exclusion group has an active work item, and the
/// newest fired window of any member. Group members are the schedules of the same tenant that
/// name the same group.
fn group_facts(
    core: &GraphCore,
    store: &JobStore,
    record: &ScheduleRecord,
    schedules: &[ScheduleRecord],
) -> (bool, Option<u64>) {
    let Some(group) = record.exclusion_group.as_deref() else {
        return (false, None);
    };
    let mut active = false;
    let mut last_fired = None;
    for member in schedules.iter().filter(|other| {
        other.tenant == record.tenant && other.exclusion_group.as_deref() == Some(group)
    }) {
        let run = load_run(core, member);
        last_fired = last_fired.max(run.last_fired_ms);
        if member.name != record.name {
            active |= run
                .active_job
                .as_deref()
                .is_some_and(|job| job_active(store, job));
        }
    }
    (active, last_fired)
}

fn upsert(id: &str, fields: serde_json::Map<String, Value>) -> Value {
    json!({"op": "upsert_node", "id": id, "properties": fields})
}

async fn service_context(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    session: String,
) -> Result<VerifiedRequestContext, String> {
    let (anonymous, server) = {
        let s = state.read().await;
        (s.local_anonymous, s.context.clone())
    };
    #[cfg(feature = "security")]
    if !anonymous {
        medulla_server_api::service_grant::ServiceGraphGrant {
            actor: "service:scheduler",
            role_prefix: SERVICE,
            unbootstrapped: "SCHEDULER_POLICY_UNBOOTSTRAPPED",
        }
        .ensure(&mut state.write().await.isolation, graph)?;
    }
    #[cfg(not(feature = "security"))]
    let _ = graph;
    if anonymous {
        return Ok(VerifiedRequestContext::anonymous_local(Some(session)));
    }
    let service = VerifiedRequestContext::authenticated_fixed_service_actor(
        &server,
        SERVICE,
        &["kg:write", "work:submit", "analytics:submit"],
    )?;
    Ok(VerifiedRequestContext::from_verified_claims_with_nonce(
        service.claims().clone(),
        session,
        Some(Nonce::minted()),
    ))
}

async fn dispatch(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    session: String,
    method: Method,
) -> Result<Response, String> {
    let context = service_context(state, graph, session).await?;
    let response = medulla_server_api::api::reentry::dispatch_verified_request(
        state,
        Request {
            id: 0,
            graph: graph.to_string(),
            auth_token: String::new(),
            agent_id: Some(context.agent_id().to_string()),
            method,
        },
        context,
    )
    .await;
    response.refusal_text().map_or(Ok(response), Err)
}

/// Write `ops` as one batch.
async fn commit(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    ops: Vec<Value>,
) -> Result<(), String> {
    let operations_msgpack = rmp_serde::to_vec_named(&ops)
        .map_err(|_| "schedule tick batch encoding failed".to_string())?;
    let session = format!("{SERVICE}:{graph}:{}", unix_ms());
    dispatch(
        state,
        graph,
        session,
        Method::BatchUpdate { operations_msgpack },
    )
    .await
    .map(|_| ())
}

/// Submit the job of the schedule's task kind; the idempotency session names the tick, so a
/// replay of the same window cannot create a second job.
async fn submit_scheduled_job(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    record: &ScheduleRecord,
    window_ms: u64,
) -> Result<String, String> {
    let job = job_for_task_kind(&record.task_kind).map_err(|e| e.to_string())?;
    let spec = SubmitJobSpec {
        graph: graph.to_string(),
        tenant: String::new(),
        actor: String::new(),
        purpose: format!("schedule:{}", record.name),
        priority: 0,
        deadline_unix_ms: None,
        quota_cpu_ms: None,
        memory_bytes: None,
        io_bytes: None,
        output_bytes: None,
        worker_pool: String::new(),
        worker_region: String::new(),
        required_capabilities: (!job.required_capability.is_empty())
            .then(|| job.required_capability.to_string())
            .into_iter()
            .collect(),
        max_attempts: 1,
        backoff_ms: 0,
        kind: JobKind::MineAssociate {
            transactions: Vec::new(),
            min_support: job.min_support,
            min_confidence: job.min_confidence,
            algorithm: job.algorithm.to_string(),
        },
    };
    let session = tick_node_id(&record.tenant, &record.name, window_ms);
    let response = dispatch(
        state,
        graph,
        session,
        Method::AnalyticsJob {
            op: JobOp::Submit(spec),
        },
    )
    .await?;
    submitted_job_id(&response).ok_or_else(|| "scheduled job submit returned no job id".to_string())
}

/// The work item id a submit response carries.
fn submitted_job_id(response: &Response) -> Option<String> {
    let record = match response.result.as_ref()? {
        ResultPayload::Json(value) => value.clone(),
        ResultPayload::Raw(bytes) => rmp_serde::from_slice::<Value>(bytes).ok()?,
        _ => return None,
    };
    record.get("job_id")?.as_str().map(str::to_string)
}
