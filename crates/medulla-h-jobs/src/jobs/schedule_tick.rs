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
pub(super) const TICK_NODE_TYPE: &str = "ScheduleTick";
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
    /// When each graph's stored queries were last re-linted (MEDULLA-FEDERATED-QUERY-R708.9).
    relinted_at: BTreeMap<String, u64>,
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
        purge_due_dashboards(state, &graph, &core, host, now_ms, poll_s).await?;
        if graph == medulla_server_api::knob_config::default_ingest_graph() {
            recover_drains(state, &graph, &core, &store, host, now_ms, poll_s).await?;
            // MEDULLA-DECISION-ENGINE-R073.6: a claim event whose publish was interrupted after
            // the claim store committed is published now. The publisher leases each row under one
            // consumer and its graph write replays by row, so concurrent or repeated passes
            // publish each row once.
            #[cfg(feature = "statechart")]
            if let Err(error) = crate::memory_event_publisher::publish_claim_events(state).await {
                eprintln!("MEMORY_EVENT_RECOVERY_FAILED: {error}");
            }
        }
        relint_stored_queries(state, &graph, &core, cache, host, now_ms, poll_s).await?;
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
        // The exclusion groups whose member fired earlier in this pass. The run
        // record that names that work item is committed through the graph and is
        // not yet guaranteed to be readable by the next member's check, so the
        // pass carries it itself: a later member of the group is skipped, never
        // fired beside it (MEDULLA-PLACEMENT-R751.760).
        let mut fired_groups: std::collections::BTreeMap<(String, String), String> =
            BTreeMap::new();
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
            let (mut group_run_active, group_last_fired_ms) =
                group_facts(&core, &store, &record, &schedules);
            if let Some(group) = record.exclusion_group.as_deref() {
                if let Some(fired) = fired_groups.get(&(record.tenant.clone(), group.to_string())) {
                    if fired != &record.name {
                        group_run_active = true;
                    }
                }
            }
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
                if let Some(group) = record.exclusion_group.clone() {
                    fired_groups.insert((record.tenant.clone(), group), record.name.clone());
                }
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
            fields.insert("used_run_s".into(), run.used_run_s.into());
            fields.insert("used_tokens".into(), run.used_tokens.into());
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

/// The page tasks the Running drains of `core` are waiting on that the queue does not hold: a
/// drain whose node advanced but whose next page was never enqueued (a crash between the task ack
/// and the enqueue). A drain updated within one poll interval is in flight and is left alone.
fn missing_drain_pages(
    core: &GraphCore,
    store: &JobStore,
    now_ms: u64,
    poll_s: u64,
) -> Vec<medulla_source_types::ingest_job::IngestJobEnvelope> {
    let grace_ms = i64::try_from(poll_s.saturating_mul(1_000)).unwrap_or(i64::MAX);
    let now = i64::try_from(now_ms).unwrap_or(i64::MAX);
    let mut missing = Vec::new();
    for id in core
        .nodes_by_property("type", "SourceDrain")
        .unwrap_or_default()
    {
        let Some(Value::Object(properties)) = decode_node(core, &id) else {
            continue;
        };
        let Some(body) = properties.get("drain_record").and_then(Value::as_str) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<medulla_ingestion_types::ingest_drain::DrainRecord>(body)
        else {
            continue;
        };
        if now.saturating_sub(record.updated_at_ms) < grace_ms {
            continue;
        }
        let Ok(Some(page)) = medulla_source_types::ingest_job::recovery_page(&record) else {
            continue;
        };
        if matches!(
            store.get(&medulla_source_types::ingest_job::task_id(&page)),
            Err(medulla_jobs::JobError::NotFound(_))
        ) {
            missing.push(page);
        }
    }
    missing
}

/// Enqueue the missing next page of every Running drain (MEDULLA-INGEST-WORKER-R605.2).
///
/// The drain ack writes the drain node first, then acks the page task, then enqueues the next
/// page; a crash after the node write leaves a Running drain with no task for its cursor page.
/// This pass, on the leader only, enqueues that page under its deterministic task id, so it
/// cannot duplicate a page the ack handler enqueues concurrently.
async fn recover_drains(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    core: &GraphCore,
    store: &JobStore,
    host: &str,
    now_ms: u64,
    poll_s: u64,
) -> Result<(), String> {
    let missing = missing_drain_pages(core, store, now_ms, poll_s);
    if missing.is_empty() {
        return Ok(());
    }
    if decode_node(core, LEASE_NODE_ID).is_none() {
        commit(
            state,
            graph,
            vec![upsert(LEASE_NODE_ID, LeaseRecord::default().to_fields())],
        )
        .await?;
    }
    let cell = GraphLeaseCell { core };
    let ttl_ms = poll_s.max(1) * 3_000;
    let Some(fence) = contend(&cell, host, now_ms, ttl_ms).map_err(|e| format!("{e:?}"))? else {
        return Ok(());
    };
    cell.read()
        .leadership()
        .authorize(host, fence, now_ms)
        .map_err(|e| e.to_string())?;
    let at = i64::try_from(now_ms).unwrap_or(i64::MAX);
    for page in missing {
        let task = medulla_source_types::ingest_job::task_id(&page);
        let payload = serde_json::to_string(&page).map_err(|error| error.to_string())?;
        let spec = medulla_jobs::TaskSubmit {
            tenant: page.tenant.clone(),
            actor: SERVICE.to_string(),
            tenant_capability: crate::task_queue::tenant_capability(&page.tenant),
            queue: page.lane().queue_name().to_string(),
            kind: page.kind.task_kind().to_string(),
            payload_ref: payload,
            idempotency_key: task.clone(),
            priority: medulla_work_types::task_queue::default_priority(),
            not_before_ms: 0,
            job_id: Some(task),
            max_attempts: Some(u32::MAX),
        };
        store.submit_task(spec, at).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Purge the dashboards of `graph` whose retention has ended (MEDULLA-TYPED-PACKS-R109.2).
///
/// Runs on the leader only: the graph's leader lease is contended as the schedule windows
/// contend it, and a pass that does not hold it purges nothing. It reads the `DashboardAsset`
/// nodes with `removed_at_ms` set, tests each with `medulla_core::dashboard::purge_due` against
/// the `catalog.dashboard_retention_days` knob and commits one batch that nulls the body on the
/// asset node and on every `DashboardRevision` node, keeping the revision nodes themselves.
async fn purge_due_dashboards(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    core: &GraphCore,
    host: &str,
    now_ms: u64,
    poll_s: u64,
) -> Result<(), String> {
    use medulla_core::dashboard::{purge_due, purge_operations, state_from_properties};
    let retention_days = medulla_types::dashboard::dashboard_retention_days();
    let mut operations = Vec::new();
    for id in core
        .nodes_by_property("type", medulla_types::dashboard::DASHBOARD_ASSET_LABEL)
        .unwrap_or_default()
    {
        let Some(Value::Object(properties)) = decode_node(core, &id) else {
            continue;
        };
        let Some(stored) = state_from_properties(&properties) else {
            continue;
        };
        if purge_due(&stored, now_ms, retention_days) {
            operations.extend(purge_operations(&stored.asset.id, stored.asset.revision));
        }
    }
    if operations.is_empty() {
        return Ok(());
    }
    if decode_node(core, LEASE_NODE_ID).is_none() {
        commit(
            state,
            graph,
            vec![upsert(LEASE_NODE_ID, LeaseRecord::default().to_fields())],
        )
        .await?;
    }
    let cell = GraphLeaseCell { core };
    let ttl_ms = poll_s.max(1) * 3_000;
    let Some(fence) = contend(&cell, host, now_ms, ttl_ms).map_err(|e| format!("{e:?}"))? else {
        return Ok(());
    };
    cell.read()
        .leadership()
        .authorize(host, fence, now_ms)
        .map_err(|e| e.to_string())?;
    commit(state, graph, operations).await
}

/// Re-lint the stored query versions of `graph` against the trust flags (MEDULLA-FEDERATED-QUERY-R708.9).
///
/// At most once per `stored_query.relint_period_s` per graph, on the leader only. Each stored
/// version names the assets it reads (recorded at its put); a version that reads an asset
/// flagged `deprecation` carries one `STORED_QUERY_DEPRECATED_ASSET` warning per asset on its
/// `StoredQueryLint` node. A node is rewritten only when its warnings change, and a version
/// whose warnings cleared has its node's list emptied. The lint warns; it never stops a run.
async fn relint_stored_queries(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    core: &GraphCore,
    cache: &mut ScheduleCache,
    host: &str,
    now_ms: u64,
    poll_s: u64,
) -> Result<(), String> {
    use medulla_catalog_types::stewardship::{deprecated_assets, TRUST_FLAG_LABEL};
    use medulla_source_types::stored_query::{lint_stored_query, StoredQueryWarning};
    let period_ms = medulla_server_api::knob_config::knobs()
        .stored_query_relint_period_s
        .saturating_mul(1_000);
    if let Some(last) = cache.relinted_at.get(graph) {
        if now_ms < last.saturating_add(period_ms) {
            return Ok(());
        }
    }
    let queries = core
        .nodes_by_property("type", "StoredQuery")
        .unwrap_or_default();
    if queries.is_empty() {
        cache.relinted_at.insert(graph.to_string(), now_ms);
        return Ok(());
    }
    let flags: Vec<serde_json::Map<String, Value>> = core
        .get_nodes_by_label(TRUST_FLAG_LABEL, usize::MAX)
        .into_iter()
        .filter_map(|(_, blob)| medulla_types::msgpack::decode_property_object(&blob).ok())
        .collect();
    let mut operations = Vec::new();
    for id in queries {
        let Some(Value::Object(node)) = decode_node(core, &id) else {
            continue;
        };
        let text = |key: &str| node.get(key).and_then(Value::as_str).unwrap_or_default();
        let version = node.get("version").and_then(Value::as_u64).unwrap_or(0);
        let references: Vec<String> = node
            .get("references")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        let warnings: Vec<StoredQueryWarning> =
            lint_stored_query(&references, &deprecated_assets(&flags, text("author_tenant")));
        let lint_id = format!("stored_query_lint:{}:{version}", text("name"));
        let held: Vec<StoredQueryWarning> = decode_node(core, &lint_id)
            .and_then(|lint| serde_json::from_value(lint.get("warnings")?.clone()).ok())
            .unwrap_or_default();
        if warnings == held {
            continue;
        }
        let mut fields = serde_json::Map::new();
        fields.insert("type".into(), "StoredQueryLint".into());
        fields.insert("name".into(), text("name").into());
        fields.insert("version".into(), version.into());
        fields.insert("warnings".into(), serde_json::to_value(&warnings).map_err(|e| e.to_string())?);
        fields.insert("linted_at_ms".into(), now_ms.into());
        operations.push(upsert(&lint_id, fields));
    }
    cache.relinted_at.insert(graph.to_string(), now_ms);
    if operations.is_empty() {
        return Ok(());
    }
    if decode_node(core, LEASE_NODE_ID).is_none() {
        commit(
            state,
            graph,
            vec![upsert(LEASE_NODE_ID, LeaseRecord::default().to_fields())],
        )
        .await?;
    }
    let cell = GraphLeaseCell { core };
    let ttl_ms = poll_s.max(1) * 3_000;
    let Some(fence) = contend(&cell, host, now_ms, ttl_ms).map_err(|e| format!("{e:?}"))? else {
        return Ok(());
    };
    cell.read()
        .leadership()
        .authorize(host, fence, now_ms)
        .map_err(|e| e.to_string())?;
    commit(state, graph, operations).await
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
            trace_id: None,
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
    if record.task_kind == medulla_jobs::schedule::PAPER_INTAKE_TASK_KIND {
        return enqueue_paper_intake(state, record, window_ms).await;
    }
    let job = job_for_task_kind(&record.task_kind).map_err(|e| e.to_string())?;
    let spec = SubmitJobSpec {
        graph: graph.to_string(),
        // The fired job belongs to the schedule's owner, not to this service.
        tenant: record.tenant.clone(),
        actor: record.owner.clone(),
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

/// The graph that holds the platform goals.
const COMMONS_GRAPH: &str = "__commons__";
/// The node label of a platform goal in the commons graph.
const PLATFORM_GOAL_LABEL: &str = "PlatformGoal";
/// The ingest queue a paper intake task is enqueued on.
const PAPER_INTAKE_QUEUE: &str = "ingest";

/// The run id of one paper intake firing: `sha256(schedule_id + boundary)`.
pub(crate) fn paper_intake_run_id(schedule_id: &str, boundary_ms: u64) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(schedule_id.as_bytes());
    hasher.update(boundary_ms.to_string().as_bytes());
    hex::encode(hasher.finalize())
}

/// The ids of the `:PlatformGoal` nodes of the commons graph, sorted. None when the commons graph
/// is not resident.
async fn platform_goal_ids(state: &Arc<RwLock<ServerState>>) -> Vec<String> {
    let core = {
        let current = state.read().await;
        current
            .registry
            .get(COMMONS_GRAPH)
            .map(|entry| entry.core.clone())
    };
    let mut ids: Vec<String> = core
        .map(|core| {
            core.get_nodes_by_label(PLATFORM_GOAL_LABEL, usize::MAX)
                .into_iter()
                .map(|(id, _)| id)
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// Enqueue the one `paper_intake` task of the window (MEDULLA-REPO-INGEST-R006.6): its payload
/// names the run id, the cycle boundary and the platform goal ids, and its idempotency key is the
/// run id, so a repeated firing of the same boundary writes nothing. The queue row is the only
/// write. A repeat answers the run id and reports `PAPER_INTAKE_DUPLICATE_RUN`.
async fn enqueue_paper_intake(
    state: &Arc<RwLock<ServerState>>,
    record: &ScheduleRecord,
    window_ms: u64,
) -> Result<String, String> {
    let schedule_id = ScheduleRecord::node_id(&record.tenant, &record.name);
    let run_id = paper_intake_run_id(&schedule_id, window_ms);
    let goal_ids = platform_goal_ids(state).await;
    let payload = serde_json::to_string(&json!({
        "kind": medulla_jobs::schedule::PAPER_INTAKE_TASK_KIND,
        "run_id": run_id,
        "cycle_boundary_ms": window_ms,
        "goal_ids": goal_ids,
    }))
    .map_err(|error| error.to_string())?;
    let store = outbox_job_store(state).await?;
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms() as i64;
    let spec = medulla_jobs::TaskSubmit {
        tenant: record.tenant.clone(),
        actor: record.owner.clone(),
        tenant_capability: crate::task_queue::tenant_capability(&record.tenant),
        queue: PAPER_INTAKE_QUEUE.to_string(),
        kind: medulla_jobs::schedule::PAPER_INTAKE_TASK_KIND.to_string(),
        payload_ref: payload,
        idempotency_key: run_id.clone(),
        priority: medulla_work_types::task_queue::default_priority(),
        not_before_ms: 0,
        job_id: Some(run_id.clone()),
        max_attempts: Some(u32::MAX),
    };
    match store.submit_task(spec, now_ms).map_err(|e| e.to_string())? {
        medulla_jobs::TaskSubmitOutcome::Enqueued(job) => Ok(job.job_id),
        medulla_jobs::TaskSubmitOutcome::Duplicate => {
            eprintln!(
                "{}: run {run_id} already holds a queue row",
                medulla_jobs::schedule::PAPER_INTAKE_DUPLICATE_RUN
            );
            Ok(run_id)
        }
    }
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

// MEDULLA-TYPED-PACKS-R109.4: the purge of a deleted dashboard runs through the real schedule
// tick pass (`tick_pass` -> `purge_due_dashboards`) on a served engine state, with the retention
// knob at its one-day floor and a controlled clock handed to the pass, never a faked null body.
#[cfg(test)]
mod dashboard_purge_tick_tests {
    use super::*;
    use medulla_persistence::redb_backend::RedbBackend;
    use medulla_protocol_types::protocol::{GraphType, Method};
    use medulla_server_auth::auth::{
        dispatch_test_on_heap, scoped_test_request_on, ScopedTestCaller,
    };
    use medulla_types::dashboard::{
        dashboard_node_id, dashboard_revision_node_id, DashboardAssetOp, DashboardBody,
        DashboardPanel, ENV_DASHBOARD_RETENTION_DAYS,
    };

    const SECRET: &str = "dashboard-purge-tick-test-secret";
    const ISSUER: &str = "dashboard-purge-tick-test-issuer";
    const TENANT: &str = "tenant-dashboard-purge-tick";
    const GRAPH: &str = "dashboard-purge-tick-test";
    const DAY_MS: u64 = 86_400_000;

    fn next_nonce() -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    async fn call(state: &Arc<RwLock<ServerState>>, method: Method) -> Response {
        let id = next_nonce() + 1;
        let request = scoped_test_request_on(
            SECRET,
            id,
            GRAPH,
            method,
            ScopedTestCaller {
                principal: ISSUER,
                tenant: TENANT,
                scopes: &["*"],
                nonce: &format!("dashboard-purge-nonce-{id}"),
                idempotency_key: &format!("dashboard-purge-key-{id}"),
            },
        );
        dispatch_test_on_heap(state, request).await
    }

    async fn dashboard(state: &Arc<RwLock<ServerState>>, op: DashboardAssetOp) -> Response {
        call(state, Method::DashboardAsset { op: Box::new(op) }).await
    }

    async fn core_of(state: &Arc<RwLock<ServerState>>) -> Arc<GraphCore> {
        let s = state.read().await;
        s.registry
            .all_entries()
            .into_iter()
            .find(|entry| entry.name == GRAPH)
            .expect("the dashboard graph")
            .core
            .clone()
    }

    #[tokio::test]
    // spec: MEDULLA-TYPED-PACKS-R109.4
    async fn the_schedule_tick_purges_a_deleted_dashboard_after_its_retention_and_a_read_refuses_purged(
    ) {
        std::env::set_var(ENV_DASHBOARD_RETENTION_DAYS, "1");
        let dir = tempfile::tempdir().unwrap();
        let mut server = medulla_server::seams_install::new_state_for_test(
            SECRET,
            ServerState::test_isolation(ISSUER),
        );
        server.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        let backend =
            RedbBackend::open(dir.path().join("graphs").to_string_lossy().into_owned(), 64)
                .unwrap();
        server.persistence = Some(Arc::new(backend));
        std::mem::forget(dir);
        let state = Arc::new(RwLock::new(server));
        let created = call(
            &state,
            Method::CreateGraph {
                graph_name: GRAPH.into(),
                graph_type: GraphType::Global,
            },
        )
        .await;
        assert!(created.error.is_none(), "{:?}", created.error);

        let body = DashboardBody {
            panels: vec![DashboardPanel {
                id: "p0".into(),
                source: "orders".into(),
                query: "SELECT * FROM orders".into(),
                terms: Vec::new(),
                concepts: Vec::new(),
            }],
            layout: Value::Null,
            parameters: Value::Null,
        };
        for revision in 0..2 {
            let saved = dashboard(
                &state,
                DashboardAssetOp::Save {
                    id: "purge-me".into(),
                    body: body.clone(),
                    revision,
                },
            )
            .await;
            assert!(saved.error.is_none(), "{:?}", saved.error);
        }
        let kept = dashboard(
            &state,
            DashboardAssetOp::Save {
                id: "kept".into(),
                body: body.clone(),
                revision: 0,
            },
        )
        .await;
        assert!(kept.error.is_none(), "{:?}", kept.error);
        let deleted = dashboard(
            &state,
            DashboardAssetOp::Delete {
                id: "purge-me".into(),
                expected_revision: None,
            },
        )
        .await;
        assert!(deleted.error.is_none(), "{:?}", deleted.error);

        let core = core_of(&state).await;
        let removed_ms = decode_node(&core, &dashboard_node_id("purge-me"))
            .and_then(|node| node.get("removed_at_ms").and_then(Value::as_u64))
            .expect("the delete stamped removed_at_ms");
        let body_of = |id: &str| {
            decode_node(&core, id).and_then(|node| node.get("body").cloned())
        };
        let mut cache = ScheduleCache::default();

        // One pass a moment before the retention ends purges nothing.
        tick_pass(&state, "engine:test", &mut cache, removed_ms, removed_ms + DAY_MS - 1, 1)
            .await
            .unwrap();
        assert!(body_of(&dashboard_node_id("purge-me")).is_some_and(|b| !b.is_null()));

        // One pass once the retention has ended nulls the body of the asset and of every
        // revision, keeps the revision nodes, and leaves the live dashboard alone.
        tick_pass(&state, "engine:test", &mut cache, removed_ms + DAY_MS, removed_ms + DAY_MS + 1, 1)
            .await
            .unwrap();
        for id in [
            dashboard_node_id("purge-me"),
            dashboard_revision_node_id("purge-me", 1),
            dashboard_revision_node_id("purge-me", 2),
        ] {
            assert!(decode_node(&core, &id).is_some(), "{id} stays");
            assert!(body_of(&id).is_none_or(|b| b.is_null()), "{id} body is purged");
        }
        assert!(body_of(&dashboard_node_id("kept")).is_some_and(|b| !b.is_null()));

        // The next read refuses as purged, and the restore is refused too.
        let read = dashboard(&state, DashboardAssetOp::Get { id: "purge-me".into() }).await;
        assert!(
            read.refusal_text().is_some_and(|e| e.contains("DASHBOARD_PURGED")),
            "{:?}",
            read.error
        );
    }
}
