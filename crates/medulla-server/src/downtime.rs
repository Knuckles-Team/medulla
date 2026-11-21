//! The downtime scheduler tick (MEDULLA-DURABLE-KERNEL-R101.3).
//!
//! `server_startup::downtime_tick` calls [`run_tick`] every `downtime.tick_secs`
//! seconds. One tick reads the `dw:JobType` instances of the commons graph with a
//! SPARQL `SELECT`, evaluates each precondition with `medulla_rdf::sparql::execute`
//! (a precondition holds when it answers `QueryOutcome::Boolean(true)`), asks
//! `medulla_jobs::downtime::select_for_window` which types fit the open window,
//! takes one `BackgroundIngestion` capacity lease per chosen type through
//! `AcquireCapacity`, and only then submits the type's job through the jobs seam.
//! A type whose lease is not granted submits nothing. The decision is recorded for
//! the served `DowntimeSelections` method.
//!
//! The capacity cells `downtime:cpu` and `downtime:memory` are the operator's to
//! provision; a tick against an unprovisioned or full cell leases nothing and so
//! submits nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use medulla_jobs::downtime::{
    select_for_window, DowntimeJobType, DowntimeWindow, ForegroundActivity, PreconditionEval,
    WindowContext, WindowKind,
};
use medulla_jobs::ResourceBudget;
use medulla_mutation_types::capacity_lease::{CapacityResourceClass, LeasePriority};
use medulla_protocol_types::protocol::Method;
use medulla_rdf::sparql::{Binding, Dataset, Projection, QueryOutcome};
use medulla_server_api::downtime::{
    record_selections, DowntimeSelected, DowntimeSelections, FOREGROUND_ACTIVITY,
};
use medulla_server_api::state::ServerState;
use medulla_server_ops::api::ingest::DowntimeJobRequest;
use medulla_work_types::native_control::{
    CapacityAcquireRequest, CapacityAcquireResult, CapacityDecision, CapacityDemand,
    NativeControlSchemaVersion,
};
use tokio::sync::RwLock;

/// The graph that holds the `dw:JobType` instances.
const COMMONS: &str = "__commons__";
/// The downtime vocabulary namespace; also the projection base, so node types and
/// property names read as `dw:` terms.
const DW: &str = "https://knuckles-team.github.io/ontology/downtime-work#";
/// The capacity cell of the CPU budget, in milliseconds.
pub const CPU_CELL: &str = "downtime:cpu";
/// The capacity cell of the memory budget, in bytes.
pub const MEMORY_CELL: &str = "downtime:memory";
/// How long one background lease lasts, in milliseconds. A type holding an
/// unexpired lease is not selected again.
pub const LEASE_TTL_MS: u64 = 60_000;

const TYPES_QUERY: &str = "PREFIX dw: <https://knuckles-team.github.io/ontology/downtime-work#> \
SELECT ?t ?intent ?priority ?kind ?cpu ?mem ?start ?end WHERE { \
?t a dw:JobType ; dw:intent ?intent ; dw:priority ?priority ; dw:windowKind ?kind ; \
dw:hasBudget ?b . ?b dw:cpuMs ?cpu ; dw:memoryBytes ?mem . \
OPTIONAL { ?t dw:windowStartMs ?start } OPTIONAL { ?t dw:windowEndMs ?end } }";

const PRECONDITIONS_QUERY: &str =
    "PREFIX dw: <https://knuckles-team.github.io/ontology/downtime-work#> \
SELECT ?t ?query WHERE { ?t dw:hasPrecondition ?p . ?p dw:query ?query }";

/// What the tick remembers between runs: when each type last ran and until when
/// its lease holds. A restart forgets both; the job idempotency key keeps a
/// repeated selection inside one lease period from submitting twice.
#[derive(Debug, Default)]
pub struct TickMemory {
    last_run: BTreeMap<String, i64>,
    leased_until: BTreeMap<String, i64>,
    intents_registered: bool,
}

static MEMORY: Mutex<Option<TickMemory>> = Mutex::new(None);

/// A boxed future, so the effects stay object-safe.
pub type EffectFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The side effects of one tick: the capacity lease and the job submission.
pub trait TickEffects: Send + Sync {
    /// Take one background lease for `request`; `Ok(true)` when it was granted.
    fn lease<'a>(
        &'a self,
        request: CapacityAcquireRequest,
    ) -> EffectFuture<'a, Result<bool, String>>;
    /// Submit the job of one selected type and answer its job id.
    fn submit<'a>(
        &'a self,
        request: DowntimeJobRequest,
    ) -> EffectFuture<'a, Result<String, String>>;
}

/// The knob values one tick plans with.
#[derive(Clone, Copy, Debug)]
pub struct TickKnobs {
    pub idle_max_rps: u32,
    pub idle_quiet_secs: u32,
    pub window_cpu_permille: u32,
    pub window_memory_bytes: u64,
    pub tick_secs: u64,
}

impl TickKnobs {
    /// The knob values installed at startup.
    pub fn installed() -> Self {
        let knobs = medulla_server_api::knob_config::knobs();
        Self {
            idle_max_rps: knobs.downtime_idle_max_rps,
            idle_quiet_secs: knobs.downtime_idle_quiet_secs,
            window_cpu_permille: knobs.downtime_window_cpu_permille,
            window_memory_bytes: knobs.downtime_window_memory_bytes,
            tick_secs: knobs.downtime_tick_secs,
        }
    }
}

/// The types one tick chose, before any lease.
#[derive(Debug, Default, PartialEq)]
pub struct TickPlan {
    pub selected: Vec<DowntimeJobType>,
    pub skipped_precondition: Vec<String>,
    pub skipped_budget: Vec<String>,
}

fn lexical(binding: &Binding) -> String {
    let raw = binding.as_str();
    if let Some(rest) = raw.strip_prefix('"') {
        if let Some(end) = rest.rfind('"') {
            return rest[..end].to_string();
        }
    }
    raw.trim_matches(|c| c == '<' || c == '>').to_string()
}

type Row = std::collections::HashMap<String, Binding>;

fn cell(row: &Row, name: &str) -> Option<String> {
    row.get(name).map(lexical)
}

fn number<T: std::str::FromStr>(row: &Row, name: &str) -> Option<T> {
    cell(row, name)?.trim().parse().ok()
}

/// Build the job types from the two `SELECT` result tables. A row that misses a
/// required column, or names a window kind other than `idle` or `declared`, is
/// not a job type and is left out.
pub fn types_from_rows(type_rows: &[Row], precondition_rows: &[Row]) -> Vec<DowntimeJobType> {
    let mut preconditions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in precondition_rows {
        if let (Some(iri), Some(query)) = (cell(row, "t"), cell(row, "query")) {
            preconditions.entry(iri).or_default().push(query);
        }
    }
    let mut seen = BTreeSet::new();
    let mut types = Vec::new();
    for row in type_rows {
        let Some(iri) = cell(row, "t") else { continue };
        let window = match cell(row, "kind").as_deref() {
            Some("idle") => WindowKind::Idle,
            Some("declared") => WindowKind::Declared,
            _ => continue,
        };
        let (Some(intent), Some(priority), Some(cpu_ms), Some(memory_bytes)) = (
            cell(row, "intent"),
            number::<u16>(row, "priority"),
            number::<u64>(row, "cpu"),
            number::<u64>(row, "mem"),
        ) else {
            continue;
        };
        if !seen.insert(iri.clone()) {
            continue;
        }
        let mut found = preconditions.remove(&iri).unwrap_or_default();
        found.sort();
        types.push(DowntimeJobType {
            iri,
            intent,
            priority,
            budget: ResourceBudget {
                cpu_ms: Some(cpu_ms),
                memory_bytes: Some(memory_bytes),
                ..ResourceBudget::default()
            },
            preconditions: found,
            window,
        });
    }
    types
}

/// The declared window of one type: its own start and end, or `None` when it
/// names no complete window.
fn declared_windows(type_rows: &[Row]) -> BTreeMap<String, DowntimeWindow> {
    let mut windows = BTreeMap::new();
    for row in type_rows {
        if let (Some(iri), Some(start_ms), Some(end_ms)) = (
            cell(row, "t"),
            number::<i64>(row, "start"),
            number::<i64>(row, "end"),
        ) {
            windows.insert(
                iri,
                DowntimeWindow {
                    kind: WindowKind::Declared,
                    start_ms,
                    end_ms,
                },
            );
        }
    }
    windows
}

struct ViewEval<'a> {
    dataset: &'a Dataset<'a>,
    projection: &'a Projection,
}

impl PreconditionEval for ViewEval<'_> {
    fn holds(&self, precondition: &str) -> bool {
        matches!(
            medulla_rdf::sparql::execute(self.dataset, precondition, self.projection, None),
            Ok(QueryOutcome::Boolean(true))
        )
    }
}

/// Choose the types to run now: the idle window over every `idle` type, and each
/// distinct declared window over the `declared` types that name it. The leased
/// types are those whose lease has not yet lapsed.
pub fn plan_tick(
    types: &[DowntimeJobType],
    declared: &BTreeMap<String, DowntimeWindow>,
    memory: &TickMemory,
    eval: &dyn PreconditionEval,
    knobs: TickKnobs,
    now_ms: i64,
) -> TickPlan {
    let reading = FOREGROUND_ACTIVITY.reading(now_ms.max(0) as u64);
    let held: BTreeSet<String> = memory
        .leased_until
        .iter()
        .filter(|(_, until)| **until > now_ms)
        .map(|(iri, _)| iri.clone())
        .collect();
    let context = WindowContext {
        now_ms,
        activity: ForegroundActivity {
            queue_empty: true,
            peak_rps: reading.peak_rps,
            quiet_secs: reading.quiet_secs,
        },
        budget: ResourceBudget {
            cpu_ms: Some(u64::from(knobs.window_cpu_permille).saturating_mul(knobs.tick_secs)),
            memory_bytes: Some(knobs.window_memory_bytes),
            ..ResourceBudget::default()
        },
        held: held.clone(),
        leased: held,
        idle_max_rps: knobs.idle_max_rps,
        idle_quiet_secs: knobs.idle_quiet_secs,
    };
    let mut groups: Vec<(DowntimeWindow, Vec<DowntimeJobType>)> = vec![(
        DowntimeWindow {
            kind: WindowKind::Idle,
            start_ms: 0,
            end_ms: 0,
        },
        types
            .iter()
            .filter(|t| t.window == WindowKind::Idle)
            .cloned()
            .collect(),
    )];
    let mut by_window: BTreeMap<(i64, i64), Vec<DowntimeJobType>> = BTreeMap::new();
    for job_type in types.iter().filter(|t| t.window == WindowKind::Declared) {
        if let Some(window) = declared.get(&job_type.iri) {
            by_window
                .entry((window.start_ms, window.end_ms))
                .or_default()
                .push(job_type.clone());
        }
    }
    for ((start_ms, end_ms), members) in by_window {
        groups.push((
            DowntimeWindow {
                kind: WindowKind::Declared,
                start_ms,
                end_ms,
            },
            members,
        ));
    }
    let mut plan = TickPlan::default();
    for (window, members) in &groups {
        let selection = select_for_window(members, window, &memory.last_run, eval, &context);
        for iri in &selection.selected {
            if let Some(job_type) = members.iter().find(|t| &t.iri == iri) {
                plan.selected.push(job_type.clone());
            }
        }
        plan.skipped_precondition
            .extend(selection.skipped_precondition);
        plan.skipped_budget.extend(selection.skipped_budget);
    }
    plan
}

/// The lease request of one selected type: CPU and memory from its budget, at
/// `BackgroundIngestion` priority.
pub fn lease_request(job_type: &DowntimeJobType, now_ms: i64) -> CapacityAcquireRequest {
    let key = idempotency_key(&job_type.iri, now_ms);
    CapacityAcquireRequest {
        schema_version: NativeControlSchemaVersion::V1,
        tenant_ref: medulla_server_api::api::persistence::RaftMutationContext::internal_tenant_scope(),
        work_item_id: job_type.iri.clone(),
        owner_digest:
            medulla_server_api::api::persistence::RaftMutationContext::internal_principal_fingerprint(),
        idempotency_key: key,
        priority: LeasePriority::BackgroundIngestion,
        demands: vec![
            CapacityDemand {
                cell_id: CPU_CELL.to_string(),
                resource_class: CapacityResourceClass::Cpu,
                amount: job_type.budget.cpu_ms.unwrap_or(0),
            },
            CapacityDemand {
                cell_id: MEMORY_CELL.to_string(),
                resource_class: CapacityResourceClass::Memory,
                amount: job_type.budget.memory_bytes.unwrap_or(0),
            },
        ],
        lease_id: None,
        ttl_ms: LEASE_TTL_MS,
        now_ms: now_ms.max(0) as u64,
        cost_budget_micros: None,
        token_budget: None,
    }
}

/// Names one selection of one type inside one lease period.
fn idempotency_key(iri: &str, now_ms: i64) -> String {
    format!("{iri}:{}", now_ms.max(0) / LEASE_TTL_MS as i64)
}

/// Lease and submit every selected type, in run order, and answer the decision.
pub async fn apply_plan(
    effects: &dyn TickEffects,
    memory: &mut TickMemory,
    plan: TickPlan,
    now_ms: i64,
) -> DowntimeSelections {
    let mut decision = DowntimeSelections {
        at_ms: now_ms,
        selected: Vec::new(),
        skipped_precondition: plan.skipped_precondition,
        skipped_budget: plan.skipped_budget,
        skipped_lease: Vec::new(),
    };
    for job_type in plan.selected {
        let granted = effects.lease(lease_request(&job_type, now_ms)).await;
        if !matches!(granted, Ok(true)) {
            decision.skipped_lease.push(job_type.iri);
            continue;
        }
        let request = DowntimeJobRequest {
            iri: job_type.iri.clone(),
            intent: job_type.intent.clone(),
            priority: job_type.priority,
            cpu_ms: job_type.budget.cpu_ms.unwrap_or(0),
            memory_bytes: job_type.budget.memory_bytes.unwrap_or(0),
            idempotency_key: idempotency_key(&job_type.iri, now_ms),
        };
        memory
            .leased_until
            .insert(job_type.iri.clone(), now_ms + LEASE_TTL_MS as i64);
        match effects.submit(request).await {
            Ok(job_id) => {
                memory.last_run.insert(job_type.iri.clone(), now_ms);
                decision.selected.push(DowntimeSelected {
                    iri: job_type.iri,
                    intent: job_type.intent,
                    job_id,
                });
            }
            Err(error) => {
                tracing::warn!(iri = %job_type.iri, %error, "downtime job was not submitted");
            }
        }
    }
    decision
}

/// The effects of a served engine: the capacity ledger of the persistence
/// backend and the jobs seam.
struct StateEffects {
    state: Arc<RwLock<ServerState>>,
    persist_dir: Option<String>,
    backend: Arc<dyn medulla_server_api::api::persistence::PersistenceBackend>,
}

impl TickEffects for StateEffects {
    fn lease<'a>(
        &'a self,
        request: CapacityAcquireRequest,
    ) -> EffectFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            let fname = medulla_kernel::persist::sanitize(COMMONS);
            let receipt = self
                .backend
                .commit_capacity_lease(&fname, Method::AcquireCapacity { request })
                .await?;
            let result: CapacityAcquireResult = rmp_serde::from_slice(&receipt)
                .map_err(|error| format!("capacity receipt is unreadable: {error}"))?;
            Ok(matches!(
                result.decision,
                CapacityDecision::Accepted | CapacityDecision::Replayed
            ))
        })
    }

    fn submit<'a>(
        &'a self,
        request: DowntimeJobRequest,
    ) -> EffectFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let jobs = medulla_server_ops::api::ingest::jobs_seam(&self.state)
                .await
                .ok_or_else(|| "INTERNAL: jobs surface unavailable".to_string())?;
            jobs.submit_downtime_job(&self.persist_dir, &request)
        })
    }
}

/// Everything one tick reads from the commons graph, planned synchronously so no
/// graph view is held across an await.
fn plan_from_graph(
    core: &medulla_graph_core::graph::GraphCore,
    memory: &TickMemory,
    is_registered: &dyn Fn(&str) -> bool,
    now_ms: i64,
) -> Result<TickPlan, String> {
    let view = core.analysis_snapshot();
    let dataset = Dataset::new(&view, Vec::new());
    let projection = Projection::from_wire(DW, "");
    let table = |query: &str| -> Result<Vec<Row>, String> {
        match medulla_rdf::sparql::execute(&dataset, query, &projection, None)? {
            QueryOutcome::Solutions(result) => Ok(result.solutions),
            _ => Err("a downtime query answered something other than a table".to_string()),
        }
    };
    let type_rows = table(TYPES_QUERY)?;
    let precondition_rows = table(PRECONDITIONS_QUERY)?;
    let types: Vec<DowntimeJobType> = types_from_rows(&type_rows, &precondition_rows)
        .into_iter()
        .filter(|job_type| match job_type.validate(is_registered) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, "downtime job type refused");
                false
            }
        })
        .collect();
    let eval = ViewEval {
        dataset: &dataset,
        projection: &projection,
    };
    Ok(plan_tick(
        &types,
        &declared_windows(&type_rows),
        memory,
        &eval,
        TickKnobs::installed(),
        now_ms,
    ))
}

/// Run one downtime tick; answers how many jobs it submitted.
pub async fn run_tick(state: &Arc<RwLock<ServerState>>, now_ms: i64) -> usize {
    let (core, persist_dir, backend) = {
        let current = state.read().await;
        let Some(entry) = current.registry.get(COMMONS) else {
            return 0;
        };
        (
            entry.core.clone(),
            current.persist_dir.clone(),
            current.persistence.clone(),
        )
    };
    let (Some(backend), Some(jobs)) = (
        backend,
        medulla_server_ops::api::ingest::jobs_seam(state).await,
    ) else {
        return 0;
    };
    let mut memory = MEMORY
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
        .unwrap_or_default();
    if !memory.intents_registered {
        match jobs.register_downtime_intents(&persist_dir) {
            Ok(()) => memory.intents_registered = true,
            Err(error) => {
                tracing::warn!(%error, "downtime intents were not registered");
                restore(memory);
                return 0;
            }
        }
    }
    let is_registered = |intent: &str| {
        jobs.intent_registered(&persist_dir, intent)
            .unwrap_or(false)
    };
    let plan = match plan_from_graph(&core, &memory, &is_registered, now_ms) {
        Ok(plan) => plan,
        Err(error) => {
            tracing::warn!(%error, "downtime tick could not read the job types");
            restore(memory);
            return 0;
        }
    };
    let effects = StateEffects {
        state: state.clone(),
        persist_dir,
        backend,
    };
    let decision = apply_plan(&effects, &mut memory, plan, now_ms).await;
    let submitted = decision.selected.len();
    record_selections(decision);
    restore(memory);
    submitted
}

fn restore(memory: TickMemory) {
    if let Ok(mut slot) = MEMORY.lock() {
        *slot = Some(memory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Always(bool);
    impl PreconditionEval for Always {
        fn holds(&self, _precondition: &str) -> bool {
            self.0
        }
    }

    fn row(pairs: &[(&str, &str)]) -> Row {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), Binding::Literal(format!("\"{value}\""))))
            .collect()
    }

    fn type_row(iri: &str, intent: &str, priority: u16) -> Row {
        let priority = priority.to_string();
        row(&[
            ("t", iri),
            ("intent", intent),
            ("priority", &priority),
            ("kind", "idle"),
            ("cpu", "100"),
            ("mem", "1048576"),
        ])
    }

    fn knobs() -> TickKnobs {
        TickKnobs {
            idle_max_rps: 1,
            idle_quiet_secs: 10,
            window_cpu_permille: 250,
            window_memory_bytes: 1 << 30,
            tick_secs: 1,
        }
    }

    /// Records every effect in order; a lease is granted unless its type is listed.
    struct Recording {
        denied: Vec<String>,
        log: Mutex<Vec<String>>,
        leases: Mutex<Vec<CapacityAcquireRequest>>,
    }

    impl TickEffects for Recording {
        fn lease<'a>(
            &'a self,
            request: CapacityAcquireRequest,
        ) -> EffectFuture<'a, Result<bool, String>> {
            Box::pin(async move {
                self.log
                    .lock()
                    .unwrap()
                    .push(format!("lease:{}", request.work_item_id));
                let granted = !self.denied.contains(&request.work_item_id);
                self.leases.lock().unwrap().push(request);
                Ok(granted)
            })
        }

        fn submit<'a>(
            &'a self,
            request: DowntimeJobRequest,
        ) -> EffectFuture<'a, Result<String, String>> {
            Box::pin(async move {
                self.log
                    .lock()
                    .unwrap()
                    .push(format!("submit:{}", request.iri));
                Ok(format!("job-{}", request.intent))
            })
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R101.3
    #[tokio::test]
    async fn tick_enqueues_leased_jobs() {
        let type_rows = vec![
            type_row("dw:plan", "plan_evolution", 9),
            type_row("dw:refresh", "live_artifact_refresh", 5),
            type_row("dw:full", "plan_evolution", 1),
        ];
        let precondition_rows = vec![row(&[("t", "dw:plan"), ("query", "ASK { }")])];
        let types = types_from_rows(&type_rows, &precondition_rows);
        assert_eq!(types.len(), 3);
        assert_eq!(types[0].preconditions, ["ASK { }"]);

        let mut memory = TickMemory::default();
        // A quiet foreground leaves the idle window open at the epoch.
        let plan = plan_tick(
            &types,
            &BTreeMap::new(),
            &memory,
            &Always(true),
            knobs(),
            1_000_000,
        );
        let order: Vec<&str> = plan.selected.iter().map(|t| t.iri.as_str()).collect();
        assert_eq!(order, ["dw:plan", "dw:refresh", "dw:full"]);

        let effects = Recording {
            denied: vec!["dw:refresh".into()],
            log: Mutex::default(),
            leases: Mutex::default(),
        };
        let decision = apply_plan(&effects, &mut memory, plan, 1_000_000).await;
        // Every submitted job was preceded by its granted lease; the denied lease
        // submitted nothing.
        assert_eq!(
            *effects.log.lock().unwrap(),
            [
                "lease:dw:plan",
                "submit:dw:plan",
                "lease:dw:refresh",
                "lease:dw:full",
                "submit:dw:full"
            ]
        );
        assert_eq!(decision.skipped_lease, ["dw:refresh"]);
        let submitted: Vec<&str> = decision.selected.iter().map(|s| s.iri.as_str()).collect();
        assert_eq!(submitted, ["dw:plan", "dw:full"]);
        assert!(effects
            .leases
            .lock()
            .unwrap()
            .iter()
            .all(|lease| lease.priority == LeasePriority::BackgroundIngestion));

        // Inside the lease period the leased types are held and not selected again.
        let again = plan_tick(
            &types,
            &BTreeMap::new(),
            &memory,
            &Always(true),
            knobs(),
            1_000_500,
        );
        let order: Vec<&str> = again.selected.iter().map(|t| t.iri.as_str()).collect();
        assert_eq!(order, ["dw:refresh"]);
    }

    // spec: MEDULLA-DURABLE-KERNEL-R101.3
    #[test]
    fn rows_without_a_window_kind_or_a_number_are_not_job_types() {
        let mut bad_kind = type_row("dw:a", "plan_evolution", 1);
        bad_kind.insert("kind".into(), Binding::Literal("\"sometimes\"".into()));
        let mut no_cpu = type_row("dw:b", "plan_evolution", 1);
        no_cpu.remove("cpu");
        let ok = type_row("dw:c", "plan_evolution", 1);
        let types = types_from_rows(&[bad_kind, no_cpu, ok], &[]);
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].iri, "dw:c");
    }
}
