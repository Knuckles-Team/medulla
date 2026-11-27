//! Private implementation module for the analytics job handler: the `AnalyticalQuery` claim
//! (MEDULLA-FEDERATED-QUERY-R731).
//!
//! The claim runs one `UnifiedQuery` plan or `Sparql` query with no node budget over the
//! snapshot of the job's graph, reading rows only through the job actor's row-level view
//! (`executor_quality::ActorRows`), and publishes the result rows through the same durable
//! publication path as every analytics result. The query is the job's input: it is read only
//! here and never logged.

use super::executor_quality::ActorRows;
use super::prelude_jobs::*;
use super::prelude_std::*;
use super::*;
use medulla_server_api::state::ServerState;
use medulla_work_types::jobs::AnalyticalQuery;

/// The verified target of one analytical query claim.
pub(super) struct AnalyticalTarget {
    pub query: AnalyticalQuery,
    pub tenant_scope: String,
    pub agent: String,
}

/// The result of one analytical run: the extra columns and one map per row.
struct AnalyticalRows {
    columns: Vec<(String, &'static str, bool)>,
    rows: Vec<BTreeMap<String, serde_json::Value>>,
}

pub(super) async fn execute_analytical_claim(
    state: &Arc<RwLock<ServerState>>,
    store: &Arc<JobStore>,
    job: &medulla_jobs::AnalyticsJob,
    worker_ref: &str,
    epoch: u64,
    target: AnalyticalTarget,
) -> Result<(), String> {
    store
        .checkpoint_fenced(
            &job.job_id,
            worker_ref,
            epoch,
            Checkpoint {
                progress: 0.1,
                label: "running".to_string(),
                state_blob: None,
                updated_at_ms: unix_ms(),
            },
            unix_ms(),
        )
        .map_err(|error| error.to_string())?;
    let (_, _, core) = resolve_core_ref(state, &job.input_snapshot.graph)
        .await
        .ok_or_else(|| "analytics target graph is unavailable".to_string())?;
    let rows_of_actor = ActorRows::of(state, &target.agent).await;
    let AnalyticalTarget { query, .. } = target;
    let ran = tokio::task::spawn_blocking(move || run_analytical(&core, &rows_of_actor, query))
        .await
        .map_err(|_| "analytical query task failed".to_string())?;
    let found = match ran {
        Ok(found) => found,
        Err(_) => {
            let _ = fail_attempt(
                store,
                &job.job_id,
                worker_ref,
                epoch,
                "kernel_failure",
                unix_ms(),
            );
            return Ok(());
        }
    };
    let now = unix_ms();
    store
        .checkpoint_fenced(
            &job.job_id,
            worker_ref,
            epoch,
            Checkpoint {
                progress: 0.9,
                label: "computed".to_string(),
                state_blob: None,
                updated_at_ms: now,
            },
            now,
        )
        .map_err(|error| error.to_string())?;
    let result = typed_analytical_result(job, found)?;
    let staged = store
        .stage_result_fenced(&job.job_id, worker_ref, epoch, result, unix_ms())
        .map_err(|error| error.to_string())?;
    publish_staged_result(state, store, staged, worker_ref, epoch).await
}

/// Run the query over the actor's rows with no node budget.
#[cfg(feature = "query")]
fn run_analytical(
    core: &medulla_graph_core::graph::GraphCore,
    rows_of_actor: &ActorRows,
    query: AnalyticalQuery,
) -> Result<AnalyticalRows, String> {
    let view = rows_of_actor.view(core);
    match query {
        AnalyticalQuery::Unified { plan_msgpack } => {
            let plan: medulla_query_types::wire::Plan = rmp_serde::from_slice(&plan_msgpack)
                .map_err(|_| "analytical plan decoding failed".to_string())?;
            let semantic = core.semantic_store.read();
            let ctx = medulla_exec::exec::PlanCtx::new(&view, &semantic);
            let unified =
                medulla_server_ops::api::query_txn::query_support::execute_rows(&plan, &ctx)?;
            let mut texts = std::collections::BTreeSet::new();
            let mut values = std::collections::BTreeSet::new();
            for row in &unified {
                texts.extend(row.texts.keys().cloned());
                values.extend(row.values.keys().cloned());
            }
            let rows = unified
                .iter()
                .map(|row| {
                    let mut out = base_row(&row.id, row.score.map(f64::from));
                    for name in &texts {
                        out.insert(
                            name.clone(),
                            row.texts
                                .get(name)
                                .map_or(serde_json::Value::Null, |term| serde_json::json!(term)),
                        );
                    }
                    for name in &values {
                        out.insert(
                            name.clone(),
                            row.values
                                .get(name)
                                .map_or(serde_json::Value::Null, |value| serde_json::json!(value)),
                        );
                    }
                    out
                })
                .collect();
            let columns = texts
                .into_iter()
                .map(|name| (name, "string", true))
                .chain(values.into_iter().map(|name| (name, "float64", true)))
                .collect();
            Ok(AnalyticalRows { columns, rows })
        }
        AnalyticalQuery::Sparql {
            query,
            base_iri,
            type_convention,
        } => run_analytical_sparql(&view, &query, &base_iri, &type_convention),
    }
}

#[cfg(not(feature = "query"))]
fn run_analytical(
    _core: &medulla_graph_core::graph::GraphCore,
    _rows_of_actor: &ActorRows,
    _query: AnalyticalQuery,
) -> Result<AnalyticalRows, String> {
    Err("analytical query jobs are not built into this worker".to_string())
}

#[cfg(all(feature = "query", feature = "shacl"))]
fn run_analytical_sparql(
    view: &medulla_graph_core::graph::GraphView,
    query: &str,
    base_iri: &str,
    type_convention: &str,
) -> Result<AnalyticalRows, String> {
    let projection = medulla_rdf::sparql::Projection::from_wire(base_iri, type_convention);
    let dataset = medulla_rdf::sparql::Dataset::new(view, Vec::new());
    let (vars, table) = medulla_rdf::sparql::execute(&dataset, query, &projection, None)?
        .into_table()
        .to_rows();
    let rows = table
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let mut out = base_row(&format!("row:{index}"), None);
            for (name, cell) in vars.iter().zip(row) {
                out.insert(
                    name.clone(),
                    cell.as_ref()
                        .map_or(serde_json::Value::Null, |text| serde_json::json!(text)),
                );
            }
            out
        })
        .collect();
    Ok(AnalyticalRows {
        columns: vars
            .into_iter()
            .map(|name| (name, "string", true))
            .collect(),
        rows,
    })
}

#[cfg(all(feature = "query", not(feature = "shacl")))]
fn run_analytical_sparql(
    _view: &medulla_graph_core::graph::GraphView,
    _query: &str,
    _base_iri: &str,
    _type_convention: &str,
) -> Result<AnalyticalRows, String> {
    Err("analytical SPARQL jobs are not built into this worker".to_string())
}

/// The KnowledgeBatch fields every row carries, with its id and score.
fn base_row(id: &str, score: Option<f64>) -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        ("id".to_string(), serde_json::json!(id)),
        (
            "kind".to_string(),
            serde_json::json!("analytical_query_row"),
        ),
        ("confidence".to_string(), serde_json::json!(1.0)),
        ("evidence_refs".to_string(), serde_json::json!([])),
        ("source_refs".to_string(), serde_json::json!([])),
        ("proof_ids".to_string(), serde_json::json!([])),
        ("contradiction_ids".to_string(), serde_json::json!([])),
        (
            "score".to_string(),
            score.map_or(serde_json::Value::Null, |score| serde_json::json!(score)),
        ),
    ])
}

fn typed_analytical_result(
    job: &medulla_jobs::AnalyticsJob,
    found: AnalyticalRows,
) -> Result<TypedJobResult, String> {
    let dataset = job.input_snapshot.dataset_ref.clone();
    let AnalyticalRows { columns, mut rows } = found;
    for row in &mut rows {
        for (name, _, _) in &columns {
            row.entry(name.clone()).or_insert(serde_json::Value::Null);
        }
    }
    let schema = [
        ("id", "string", false),
        ("kind", "string", false),
        ("confidence", "float64", false),
        ("evidence_refs", "list<string>", false),
        ("source_refs", "list<string>", false),
        ("proof_ids", "list<string>", false),
        ("contradiction_ids", "list<string>", false),
        ("score", "float64", true),
    ]
    .into_iter()
    .map(|(name, logical_type, nullable)| (name.to_string(), logical_type, nullable))
    .chain(columns)
    .map(|(name, logical_type, nullable)| ResultColumn {
        name,
        logical_type: logical_type.to_string(),
        nullable,
    })
    .collect();
    TypedJobResult::new(
        schema,
        rows,
        vec![dataset],
        Vec::new(),
        None,
        None,
        reproducibility_manifest(&job.input_snapshot, &job.algo, &job.policy),
    )
}
