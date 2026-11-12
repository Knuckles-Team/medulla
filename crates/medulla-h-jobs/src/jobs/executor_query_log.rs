//! Private implementation module for the analytics job handler: the `SourceQueryLogIngest`
//! claim (MEDULLA-UNIFIED-DATA-PLANE-R764.7).
//!
//! The claim reads one attached source's query log through `SourceQueryLog::read_since` and
//! records the usage it shows (`medulla_server_ops::api::source_usage`), then stages one
//! summary row and publishes it like every other analytics result. A refusal
//! (`QUERY_LOG_UNSUPPORTED`, `QUERY_LOG_UNAVAILABLE`) fails the attempt with that code.

use super::prelude_jobs::*;
use super::prelude_std::*;
use super::*;
use medulla_server_api::state::ServerState;
use medulla_server_ops::api::source_usage::{run_query_log_ingest, QueryLogRun, QueryLogSummary};

/// The verified target of one query log claim.
pub(super) struct QueryLogTarget {
    pub source: String,
    pub owner_scope: String,
    pub tenant_scope: String,
}

pub(super) async fn execute_query_log_claim(
    state: &Arc<RwLock<ServerState>>,
    store: &Arc<JobStore>,
    job: &medulla_jobs::AnalyticsJob,
    worker_ref: &str,
    epoch: u64,
    target: QueryLogTarget,
) -> Result<(), String> {
    store
        .checkpoint_fenced(
            &job.job_id,
            worker_ref,
            epoch,
            Checkpoint {
                progress: 0.1,
                stage: "reading".to_string(),
                state_blob: None,
                updated_at_ms: unix_ms(),
            },
            unix_ms(),
        )
        .map_err(|error| error.to_string())?;
    let (graph, _, _) = resolve_core_ref(state, &job.input_snapshot.graph)
        .await
        .ok_or_else(|| "analytics target graph is unavailable".to_string())?;
    let run = QueryLogRun {
        owner_scope: target.owner_scope,
        tenant: target.tenant_scope,
        graph,
        source: target.source,
    };
    let summary = match run_query_log_ingest(state, &run).await {
        Ok(summary) => summary,
        Err(refusal) => {
            let code = refusal.split(':').next().unwrap_or("QUERY_LOG_UNAVAILABLE");
            let _ = store.fail_attempt_fenced(&job.job_id, worker_ref, epoch, code, unix_ms());
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
                stage: "computed".to_string(),
                state_blob: None,
                updated_at_ms: now,
            },
            now,
        )
        .map_err(|error| error.to_string())?;
    let result = typed_query_log_result(job, &summary)?;
    let staged = store
        .stage_result_fenced(&job.job_id, worker_ref, epoch, result, unix_ms())
        .map_err(|error| error.to_string())?;
    publish_staged_result(state, store, staged, worker_ref, epoch).await
}

/// The one summary row a query log run publishes.
fn typed_query_log_result(
    job: &medulla_jobs::AnalyticsJob,
    summary: &QueryLogSummary,
) -> Result<TypedJobResult, String> {
    let dataset = job.input_snapshot.dataset_ref.clone();
    let row = BTreeMap::from([
        (
            "id".to_string(),
            serde_json::json!(format!("query_log_run:{}", job.job_id)),
        ),
        (
            "kind".to_string(),
            serde_json::json!("source_query_log_usage"),
        ),
        ("confidence".to_string(), serde_json::json!(1.0)),
        ("evidence_refs".to_string(), serde_json::json!([dataset])),
        ("source_refs".to_string(), serde_json::json!([dataset])),
        ("proof_ids".to_string(), serde_json::json!([])),
        ("contradiction_ids".to_string(), serde_json::json!([])),
        (
            "statements".to_string(),
            serde_json::json!(summary.statements),
        ),
        ("reads".to_string(), serde_json::json!(summary.reads)),
        ("joins".to_string(), serde_json::json!(summary.joins)),
        ("filters".to_string(), serde_json::json!(summary.filters)),
        ("unparsed".to_string(), serde_json::json!(summary.unparsed)),
        (
            "watermark_ms".to_string(),
            serde_json::json!(summary.watermark_ms),
        ),
    ]);
    TypedJobResult::new(
        [
            ("id", "string", false),
            ("kind", "string", false),
            ("confidence", "float64", false),
            ("evidence_refs", "list<string>", false),
            ("source_refs", "list<string>", false),
            ("proof_ids", "list<string>", false),
            ("contradiction_ids", "list<string>", false),
            ("statements", "int64", false),
            ("reads", "int64", false),
            ("joins", "int64", false),
            ("filters", "int64", false),
            ("unparsed", "int64", false),
            ("watermark_ms", "int64", false),
        ]
        .into_iter()
        .map(|(name, logical_type, nullable)| ResultColumn {
            name: name.to_string(),
            logical_type: logical_type.to_string(),
            nullable,
        })
        .collect(),
        vec![row],
        vec![dataset],
        Vec::new(),
        None,
        None,
        reproducibility_manifest(&job.input_snapshot, &job.algo, &job.policy),
    )
}
