//! Private implementation module for the analytics job handler: the `UsageCluster` claim
//! (MEDULLA-TYPED-PACKS-R805.7).
//!
//! The claim clusters the recorded asset usage of the caller's tenant and graph into data
//! product proposals (`medulla_server_ops::api::usage_cluster`) and publishes one typed row per
//! proposal. Each row carries the proposal and the `open` change request that holds it for review,
//! so both reach the graph through the same durable publication path as every analytics result.

use super::prelude_jobs::*;
use super::prelude_std::*;
use super::*;
use medulla_server_api::state::ServerState;
use medulla_server_ops::api::usage_cluster::{
    run_usage_cluster, ProposalWithRequest, UsageClusterRun, PRODUCT_PROPOSAL_KIND,
};

/// The verified target of one usage cluster claim.
pub(super) struct UsageClusterTarget {
    pub term_bindings: Vec<(String, String)>,
    pub tenant_scope: String,
}

pub(super) async fn execute_usage_cluster_claim(
    state: &Arc<RwLock<ServerState>>,
    store: &Arc<JobStore>,
    job: &medulla_jobs::AnalyticsJob,
    worker_ref: &str,
    epoch: u64,
    target: UsageClusterTarget,
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
    let run = UsageClusterRun {
        tenant: target.tenant_scope,
        graph,
        term_bindings: target.term_bindings,
    };
    let proposals = match run_usage_cluster(state, &run).await {
        Ok(proposals) => proposals,
        Err(refusal) => {
            let code = refusal.split(':').next().unwrap_or("USAGE_UNAVAILABLE");
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
    let result = typed_usage_cluster_result(job, &proposals)?;
    let staged = store
        .stage_result_fenced(&job.job_id, worker_ref, epoch, result, unix_ms())
        .map_err(|error| error.to_string())?;
    publish_staged_result(state, store, staged, worker_ref, epoch).await
}

/// One row per proposal: the proposal fields and its change request.
fn typed_usage_cluster_result(
    job: &medulla_jobs::AnalyticsJob,
    proposals: &[ProposalWithRequest],
) -> Result<TypedJobResult, String> {
    let dataset = job.input_snapshot.dataset_ref.clone();
    let rows: Vec<BTreeMap<String, serde_json::Value>> = proposals
        .iter()
        .map(|found| {
            let state = serde_json::to_value(found.request.state)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default();
            BTreeMap::from([
                (
                    "id".to_string(),
                    serde_json::json!(format!(
                        "{PRODUCT_PROPOSAL_KIND}:{}",
                        found.request.proposed_digest
                    )),
                ),
                ("kind".to_string(), serde_json::json!(PRODUCT_PROPOSAL_KIND)),
                ("confidence".to_string(), serde_json::json!(1.0)),
                ("evidence_refs".to_string(), serde_json::json!([dataset])),
                ("source_refs".to_string(), serde_json::json!([dataset])),
                ("proof_ids".to_string(), serde_json::json!([])),
                ("contradiction_ids".to_string(), serde_json::json!([])),
                (
                    "datasets".to_string(),
                    serde_json::json!(found.proposal.datasets),
                ),
                ("terms".to_string(), serde_json::json!(found.proposal.terms)),
                (
                    "owner_candidate".to_string(),
                    serde_json::json!(found.proposal.owner_candidate),
                ),
                (
                    "principals".to_string(),
                    serde_json::json!(found.proposal.principals),
                ),
                (
                    "period".to_string(),
                    serde_json::json!(found.proposal.period),
                ),
                (
                    "change_request_target".to_string(),
                    serde_json::json!(found.request.target),
                ),
                ("change_request_state".to_string(), serde_json::json!(state)),
                (
                    "change_request_reviewers".to_string(),
                    serde_json::json!(found.request.reviewers),
                ),
                (
                    "proposed_digest".to_string(),
                    serde_json::json!(found.request.proposed_digest),
                ),
            ])
        })
        .collect();
    TypedJobResult::new(
        [
            ("id", "string", false),
            ("kind", "string", false),
            ("confidence", "float64", false),
            ("evidence_refs", "list<string>", false),
            ("source_refs", "list<string>", false),
            ("proof_ids", "list<string>", false),
            ("contradiction_ids", "list<string>", false),
            ("datasets", "list<string>", false),
            ("terms", "list<string>", false),
            ("owner_candidate", "string", false),
            ("principals", "list<string>", false),
            ("period", "string", false),
            ("change_request_target", "string", false),
            ("change_request_state", "string", false),
            ("change_request_reviewers", "list<string>", false),
            ("proposed_digest", "string", false),
        ]
        .into_iter()
        .map(|(name, logical_type, nullable)| ResultColumn {
            name: name.to_string(),
            logical_type: logical_type.to_string(),
            nullable,
        })
        .collect(),
        rows,
        vec![dataset],
        Vec::new(),
        None,
        None,
        reproducibility_manifest(&job.input_snapshot, &job.algo, &job.policy),
    )
}
