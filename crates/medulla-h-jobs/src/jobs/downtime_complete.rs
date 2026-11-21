//! Private implementation module for the analytics job handler: the typed result of a
//! downtime job (MEDULLA-FEDERATED-QUERY-R091.3, MEDULLA-DURABLE-KERNEL-R101.7).
//!
//! A downtime body that finishes answers its output as a one-row typed result; the downtime
//! worker stages it and completes the publication. The result is the job's own record; it
//! writes nothing to a graph.

use super::prelude_jobs::*;
use super::*;

/// The one row a downtime job publishes: its output as JSON text.
pub fn downtime_output_result(
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
