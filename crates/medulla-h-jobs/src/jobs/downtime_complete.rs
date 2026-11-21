//! Private implementation module for the analytics job handler: completing a downtime job the
//! engine ran in process (MEDULLA-FEDERATED-QUERY-R091.3).
//!
//! The engine claims the one job it has just run as `engine:downtime`, stages its output as a
//! one-row typed result and completes the publication. The result is the job's own record; it
//! writes nothing to a graph.

use super::prelude_jobs::*;
use super::*;

/// Claim `job_id` for the engine's downtime worker, stage `output` and finish the job.
pub(super) fn complete_downtime_job(
    store: &JobStore,
    job_id: &str,
    output: serde_json::Value,
) -> Result<(), String> {
    let now = unix_ms();
    let worker_ref = native_opaque_ref("downtime_worker", job_id);
    let quota = TenantJobQuota {
        max_active: usize::MAX,
        max_reserved_cpu_ms: u64::MAX,
    };
    let claim = store
        .claim_job(
            job_id,
            &worker_ref,
            &[opaque_worker_capability(DOWNTIME_CAPABILITY)],
            now,
            bounded_lease_ms(60_000),
            quota,
        )
        .map_err(|error| job_refusal(&error))?
        .ok_or_else(|| format!("CONFLICT: downtime job {job_id} is not ready to be claimed"))?;
    let result = typed_downtime_result(&claim.job, &output)?;
    let staged = store
        .stage_result_fenced(job_id, &worker_ref, claim.lease.epoch, result, unix_ms())
        .map_err(|error| job_refusal(&error))?;
    store
        .complete_publication_fenced(&staged.job_id, &worker_ref, claim.lease.epoch, unix_ms())
        .map(|_| ())
        .map_err(|error| job_refusal(&error))
}

/// The one row a downtime job publishes: its output as JSON text.
fn typed_downtime_result(
    job: &medulla_jobs::AnalyticsJob,
    output: &serde_json::Value,
) -> Result<TypedJobResult, String> {
    let dataset = job.input_snapshot.dataset_ref.clone();
    let text = serde_json::to_string(output).map_err(|error| error.to_string())?;
    let row = BTreeMap::from([
        (
            "id".to_string(),
            serde_json::json!(format!("downtime_run:{}", job.job_id)),
        ),
        ("kind".to_string(), serde_json::json!("downtime_output")),
        ("confidence".to_string(), serde_json::json!(1.0)),
        ("evidence_refs".to_string(), serde_json::json!([dataset])),
        ("source_refs".to_string(), serde_json::json!([dataset])),
        ("proof_ids".to_string(), serde_json::json!([])),
        ("contradiction_ids".to_string(), serde_json::json!([])),
        ("output".to_string(), serde_json::json!(text)),
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
            ("output", "string", false),
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
