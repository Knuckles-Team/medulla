//! Private implementation module for the analytics job handler.

use super::prelude_jobs::*;
use super::prelude_server::*;
use super::prelude_std::*;
use super::*;

/// Maximum distinct items supported by the rule-enumeration mask width.
const MAX_ASSOCIATION_DISTINCT_ITEMS: usize = (u32::BITS - 1) as usize;

pub(super) struct SubmitBuild {
    pub(super) spec: SubmitJobSpec,
    pub(super) input_snapshot: InputSnapshotHandle,
    pub(super) algorithm_family: String,
    pub(super) algorithm: String,
    pub(super) params_digest: String,
    pub(super) input_payload: Vec<u8>,
    pub(super) policy_fingerprint: String,
}

fn build_submit_policy(
    authority: &CarrierAuthority,
    spec: &SubmitJobSpec,
    policy_fingerprint: &str,
) -> JobPolicy {
    let (tenant, actor) = job_owner(authority, spec);
    let purpose = medulla_server_api::batch_compile::opaque_coordinator_key(
        "job-purpose",
        "native",
        &spec.purpose,
    );
    let worker_pool = opaque_placement_value("job_pool", &spec.worker_pool);
    let worker_region = opaque_placement_value("job_region", &spec.worker_region);
    let required_capabilities = spec
        .required_capabilities
        .iter()
        .map(|value| opaque_placement_value("job_capability", value))
        .collect();
    JobPolicy {
        tenant,
        actor,
        purpose,
        priority: spec.priority,
        quota_cpu_ms: spec.quota_cpu_ms,
        deadline_unix_ms: spec.deadline_unix_ms,
        policy_fingerprint: policy_fingerprint.to_string(),
        resources: medulla_jobs::ResourceBudget {
            cpu_ms: spec.quota_cpu_ms,
            memory_bytes: spec.memory_bytes,
            io_bytes: spec.io_bytes,
            output_bytes: spec.output_bytes,
        },
        placement: medulla_jobs::JobPlacement {
            pool: worker_pool,
            region: worker_region,
            required_capabilities,
        },
    }
}

/// The verified actor of the engine's own scheduler service.
pub(super) const SCHEDULER_SERVICE_ACTOR: &str = "service:scheduler";

/// The tenant and actor scope that own a submitted job. A caller owns what it
/// submits, except that the scheduler service submits a fired window on behalf
/// of the schedule's owner: it signs the submission,
/// but the owner named by the spec's `tenant` and `actor` is who may read and
/// cancel the work. Any other caller's `tenant` and `actor` are ignored.
fn job_owner(authority: &CarrierAuthority, spec: &SubmitJobSpec) -> (String, String) {
    if authority.agent_id() == SCHEDULER_SERVICE_ACTOR && !spec.actor.is_empty() {
        return (
            medulla_server_api::carrier::tenant_scope_of(&spec.tenant),
            spec.actor.clone(),
        );
    }
    (
        authority.tenant_scope().to_string(),
        authority.actor_scope().to_string(),
    )
}

pub(super) fn build_submit_spec(authority: &CarrierAuthority, build: SubmitBuild) -> SubmitSpec {
    let SubmitBuild {
        spec,
        input_snapshot,
        algorithm_family,
        algorithm,
        params_digest,
        input_payload,
        policy_fingerprint,
    } = build;
    SubmitSpec {
        input_snapshot,
        policy: build_submit_policy(authority, &spec, &policy_fingerprint),
        algo: AlgoVersion {
            family: algorithm_family,
            algorithm,
            params_digest,
            code_version: CODE_VERSION.to_string(),
            env_version: ENV_VERSION.to_string(),
        },
        input_payload: Some(input_payload),
        max_attempts: spec.max_attempts,
        backoff_ms: spec.backoff_ms,
    }
}

#[cfg(feature = "program-optimization")]
pub(super) fn verified_program_policy(
    authority: &CarrierAuthority,
    policy_fingerprint: &str,
    purpose: &str,
) -> Result<PolicyEnvelope, String> {
    let opaque = |namespace: &str, value: &str| {
        OpaqueRef::new(native_opaque_ref(namespace, value)).map_err(|error| error.to_string())
    };
    let purpose_refs = (!purpose.trim().is_empty())
        .then(|| opaque("purpose", purpose))
        .transpose()?
        .into_iter()
        .collect();
    Ok(PolicyEnvelope {
        tenant_ref: opaque("tenant", authority.tenant_scope())?,
        access_policy_ref: opaque("policy", policy_fingerprint)?,
        classification: Classification::Internal,
        retention_policy_ref: opaque("retention", "engine-governed")?,
        deletion_policy_ref: opaque("deletion", "engine-governed")?,
        legal_hold_ref: None,
        purpose_refs,
    })
}
/// The `JobOp::Submit` arm of [`handle`]: compile the durable batch for the
/// submit op, then run [`handle_submit`].
pub(super) async fn handle_submit_op(
    state: &Arc<RwLock<ServerState>>,
    store: &Arc<JobStore>,
    req_id: u64,
    caller: (&CarrierAuthority, &VerifiedRequestContext),
    attempt_nonce: Option<Nonce>,
    spec: SubmitJobSpec,
) -> Response {
    let authority = caller.0;
    // A required heavy capability no host declares is refused before anything is
    // compiled or queued (MEDULLA-PLACEMENT-R004.4).
    if let Err(error) =
        super::host_capabilities::refuse_uncovered(state, &spec.required_capabilities).await
    {
        return Response::err(req_id, error);
    }
    let method = Method::AnalyticsJob {
        op: JobOp::Submit(spec.clone()),
    };
    let (batch, now) = match compile_job_batch(store, req_id, authority, attempt_nonce, &method) {
        Ok(value) => value,
        Err(error) => return Response::err(req_id, error),
    };
    handle_submit(state, store, req_id, caller, spec, batch, now).await
}

/// The `JobOp::Cancel` arm of [`handle`]: verify ownership, compile the
/// durable batch for the cancel op, then apply it.
pub(super) fn handle_cancel_op(
    store: &Arc<JobStore>,
    req_id: u64,
    authority: &CarrierAuthority,
    attempt_nonce: Option<Nonce>,
    job_id: String,
) -> Response {
    if let Err(error) = owned_job(store, authority, &job_id) {
        return Response::err(req_id, error);
    }
    let method = Method::AnalyticsJob {
        op: JobOp::Cancel {
            job_id: job_id.clone(),
        },
    };
    // Another job transition between the version read and the versioned write
    // is a benign race: the store re-reads the version and re-evaluates (see
    // `JobStore::request_cancel_retrying`).
    let mut compile_error = None;
    let outcome = store.request_cancel_retrying(&job_id, || {
        compile_job_batch(store, req_id, authority, attempt_nonce, &method).map_err(|error| {
            compile_error = Some(error);
            medulla_jobs::JobError::Codec("cancel batch compile failed".into())
        })
    });
    if let Some(error) = compile_error {
        return Response::err(req_id, error);
    }
    match outcome {
        Ok((job, _)) => job_response::<
            medulla_protocol_types::result_contract::coordination::JobCancel,
        >(req_id, &job),
        Err(e) => Response::err(req_id, job_refusal(&e)),
    }
}

/// The `JobOp::Resume` arm of [`handle`]: verify ownership, compile the
/// durable batch for the resume op, then run [`handle_resume`].
pub(super) async fn handle_resume_op(
    state: &Arc<RwLock<ServerState>>,
    store: &Arc<JobStore>,
    req_id: u64,
    authority: &CarrierAuthority,
    attempt_nonce: Option<Nonce>,
    job_id: String,
) -> Response {
    if let Err(error) = owned_job(store, authority, &job_id) {
        return Response::err(req_id, error);
    }
    let method = Method::AnalyticsJob {
        op: JobOp::Resume {
            job_id: job_id.clone(),
        },
    };
    let (batch, now) = match compile_job_batch(store, req_id, authority, attempt_nonce, &method) {
        Ok(value) => value,
        Err(error) => return Response::err(req_id, error),
    };
    handle_resume(state, store, req_id, &job_id, batch, now).await
}

/// Resolve this process's `JobStore` off `state`'s configured persistence
/// root, and (outside clustered/Raft mode) ensure the colocated worker pool
/// is running. Clustered mode disables automatic per-replica workers: every
/// scheduler transition there must arrive as an authenticated Worker*
/// command and cross Raft.
pub(super) async fn resolve_job_store(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
) -> Result<Arc<JobStore>, Response> {
    let (persist_dir, clustered) = {
        let current = state.read().await;
        (current.persist_dir.clone(), {
            #[cfg(feature = "raft")]
            {
                current.multi_raft.is_some()
            }
            #[cfg(not(feature = "raft"))]
            {
                false
            }
        })
    };
    let store = job_store(persist_dir.as_deref()).map_err(|error| Response::err(req_id, error))?;
    if !clustered && !job_workers_started() {
        let config = analytics_pool_config(state)
            .await
            .map_err(|error| Response::err(req_id, error))?;
        ensure_job_workers(state.clone(), store.clone(), &config);
    }
    Ok(store)
}
/// Resolve the target graph's `GraphCore` for a `Submit`, checking Read
/// access (the RLS boundary for anything the job might later read).
pub(super) async fn resolve_submit_graph_core(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    authority: &CarrierAuthority,
    graph: &str,
) -> Result<Arc<medulla_graph_core::graph::GraphCore>, Response> {
    let s = state.read().await;
    let Some(entry) = s.registry.get(graph) else {
        return Err(Response::err(
            req_id,
            format!("INVALID_ARGUMENT: unknown graph '{graph}'"),
        ));
    };
    if let Err(error) = check_graph_access(
        &s.isolation,
        Some(authority.agent_id()),
        graph,
        entry.graph_type,
        entry.owner.as_deref(),
        entry.importing.as_deref(),
        AccessLevel::Read,
    ) {
        return Err(Response::err(req_id, error));
    }
    Ok(entry.core.clone())
}

/// Whether a submit's worker-pool/region/capability placement constraints
/// are outside native bounds.
pub(super) fn submit_placement_invalid(
    worker_pool: &str,
    worker_region: &str,
    required_capabilities: &[String],
) -> bool {
    let invalid_placement_value =
        |value: &str| value.len() > 128 || value.chars().any(char::is_control);
    invalid_placement_value(worker_pool)
        || invalid_placement_value(worker_region)
        || required_capabilities.len() > 64
        || required_capabilities
            .iter()
            .any(|value| value.is_empty() || invalid_placement_value(value))
}

/// Reject invalid thresholds before traversing or pseudonymizing input.
fn validate_association_thresholds(
    req_id: u64,
    min_support: f64,
    min_confidence: f64,
) -> Result<(), Response> {
    // Reject invalid parameters before traversing or pseudonymizing the input.
    // A governed-payload refusal is caller input, not an internal engine fault.
    if !min_support.is_finite()
        || !(0.0..=1.0).contains(&min_support)
        || !min_confidence.is_finite()
        || !(0.0..=1.0).contains(&min_confidence)
    {
        return Err(Response::err(
            req_id,
            "INVALID_ARGUMENT: association job thresholds are invalid",
        ));
    }
    Ok(())
}

/// Count the opaque item references produced by Submit's hashing pass.
fn validate_association_cardinality(
    req_id: u64,
    transactions: &[Vec<String>],
) -> Result<(), Response> {
    let distinct_items = transactions
        .iter()
        .flatten()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    if distinct_items > MAX_ASSOCIATION_DISTINCT_ITEMS {
        return Err(Response::err(
            req_id,
            "INVALID_ARGUMENT: association job exceeds the distinct-item limit",
        ));
    }
    Ok(())
}

/// The `JobKind::MineAssociate` arm of [`handle_submit`]'s kind-governance
/// match: pseudonymize transaction items, validate thresholds/cardinality,
/// and validate the algorithm name.
pub(super) fn submit_job_mine_associate(
    authority: &CarrierAuthority,
    req_id: u64,
    transactions: Vec<Vec<String>>,
    min_support: f64,
    min_confidence: f64,
    algorithm: String,
) -> Result<(JobKind, String, String, serde_json::Value), Response> {
    validate_association_thresholds(req_id, min_support, min_confidence)?;
    if parse_algorithm(&algorithm).is_err() {
        return Err(Response::err(
            req_id,
            "INVALID_ARGUMENT: unsupported association job algorithm",
        ));
    }
    // Durable worker input contains opaque item references only. Source
    // labels and personal identifiers never enter jobs.redb.
    let transactions: Vec<Vec<String>> = transactions
        .into_iter()
        .map(|transaction| {
            transaction
                .into_iter()
                .map(|item| {
                    native_opaque_ref(
                        "analytics_item",
                        &format!("{}\0{item}", authority.owner_scope()),
                    )
                })
                .collect()
        })
        .collect();
    validate_association_cardinality(req_id, &transactions)?;
    let params = serde_json::json!({
        "min_support": min_support,
        "min_confidence": min_confidence,
        "algorithm": algorithm.clone(),
    });
    Ok((
        JobKind::MineAssociate {
            transactions,
            min_support,
            min_confidence,
            algorithm: algorithm.clone(),
        },
        "mining.association".to_string(),
        algorithm,
        params,
    ))
}

/// The `JobKind::SourceQueryLogIngest` arm of [`handle_submit`]'s kind-governance match: the
/// client names the source; the owner and tenant scopes are the verified caller's, replacing
/// anything the request carried.
pub(super) fn submit_job_source_query_log(
    authority: &CarrierAuthority,
    req_id: u64,
    source: String,
) -> Result<(JobKind, String, String, serde_json::Value), Response> {
    if source.trim().is_empty() || source.len() > 256 {
        return Err(Response::err(
            req_id,
            "INVALID_ARGUMENT: the source name must be 1 to 256 characters",
        ));
    }
    let params = serde_json::json!({ "source": source });
    Ok((
        JobKind::SourceQueryLogIngest {
            source,
            owner_scope: authority.owner_scope().to_string(),
            tenant_scope: authority.tenant_scope().to_string(),
        },
        "usage.query_log".to_string(),
        "source_query_log".to_string(),
        params,
    ))
}

/// A dataset, column or target name a data-plane job accepts: 1 to 256 characters,
/// letters, digits, `_`, `.`, `:` and `-` only, the character set `compile_rule`
/// accepts for a rule target.
pub(super) fn plain_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
}

/// The `JobKind::DatasetProfile` arm of [`handle_submit`]'s kind-governance match
/// (MEDULLA-UNIFIED-DATA-PLANE-R761.3): the client names the dataset and columns; the
/// owner scope, tenant scope and tenant are the verified caller's, replacing anything
/// the request carried. At most `knob profile.max_columns` columns per job.
pub(super) fn submit_job_profile(
    authority: &CarrierAuthority,
    req_id: u64,
    dataset: String,
    columns: Vec<String>,
) -> Result<(JobKind, String, String, serde_json::Value), Response> {
    if !plain_identifier(&dataset) {
        return Err(Response::err(
            req_id,
            "INVALID_ARGUMENT: the dataset must be a plain name of 1 to 256 characters",
        ));
    }
    let max_columns = medulla_server_api::knob_config::knobs().profile_max_columns as usize;
    if columns.len() > max_columns || !columns.iter().all(|column| plain_identifier(column)) {
        return Err(Response::err(
            req_id,
            "INVALID_ARGUMENT: the columns must be plain names, at most profile.max_columns of them",
        ));
    }
    let params = serde_json::json!({ "dataset": dataset, "columns": columns });
    Ok((
        JobKind::DatasetProfile {
            dataset,
            columns,
            owner_scope: authority.owner_scope().to_string(),
            tenant_scope: authority.tenant_scope().to_string(),
            tenant: authority.log_owner().0.to_string(),
            agent: authority.agent_id().to_string(),
        },
        "catalog.profile".to_string(),
        "dataset_profile".to_string(),
        params,
    ))
}

/// The `JobKind::QualityEvaluate` arm of [`handle_submit`]'s kind-governance match
/// (MEDULLA-UNIFIED-DATA-PLANE-R762.3): the client names the target; the owner scope,
/// tenant scope and tenant are the verified caller's, replacing anything the request
/// carried.
pub(super) fn submit_job_quality(
    authority: &CarrierAuthority,
    req_id: u64,
    target: String,
) -> Result<(JobKind, String, String, serde_json::Value), Response> {
    if !plain_identifier(&target) {
        return Err(Response::err(
            req_id,
            "INVALID_ARGUMENT: the target must be a plain name of 1 to 256 characters",
        ));
    }
    let params = serde_json::json!({ "target": target });
    Ok((
        JobKind::QualityEvaluate {
            target,
            owner_scope: authority.owner_scope().to_string(),
            tenant_scope: authority.tenant_scope().to_string(),
            tenant: authority.log_owner().0.to_string(),
            agent: authority.agent_id().to_string(),
        },
        "catalog.quality".to_string(),
        "quality_evaluate".to_string(),
        params,
    ))
}

/// The most bytes of one analytical query a job carries.
const MAX_ANALYTICAL_QUERY_BYTES: usize = 1 << 20;

/// The `JobKind::AnalyticalQuery` arm of [`handle_submit`]'s kind-governance match
/// (MEDULLA-FEDERATED-QUERY-R731): the client supplies the query; the tenant scope and agent
/// are the verified caller's, replacing anything the request carried. The query is the job's
/// input and is never part of the lineage params, so it is not logged.
pub(super) fn submit_job_analytical(
    authority: &CarrierAuthority,
    req_id: u64,
    query: medulla_work_types::jobs::AnalyticalQuery,
) -> Result<(JobKind, String, String, serde_json::Value), Response> {
    use medulla_work_types::jobs::AnalyticalQuery;
    let (size, shape) = match &query {
        AnalyticalQuery::Unified { plan_msgpack } => (plan_msgpack.len(), "unified"),
        AnalyticalQuery::Sparql { query, .. } => (query.len(), "sparql"),
    };
    if size == 0 || size > MAX_ANALYTICAL_QUERY_BYTES {
        return Err(Response::err(
            req_id,
            "INVALID_ARGUMENT: an analytical query is 1 byte to 1 MiB",
        ));
    }
    Ok((
        JobKind::AnalyticalQuery {
            query,
            tenant_scope: authority.tenant_scope().to_string(),
            agent: authority.agent_id().to_string(),
        },
        "query.analytical".to_string(),
        "analytical_query".to_string(),
        serde_json::json!({ "shape": shape }),
    ))
}

/// Bounded-decode one program optimization request.
#[cfg(feature = "program-optimization")]
pub(super) fn decode_optimization_request(request_msgpack: &[u8]) -> Option<OptimizationRequest> {
    medulla_types::msgpack::decode_bounded(
        request_msgpack,
        medulla_types::msgpack::MsgpackLimits::new(
            MAX_JOB_INPUT_BYTES,
            MAX_JOB_INPUT_ITEMS,
            medulla_types::msgpack::DEFAULT_MAX_DEPTH,
        ),
    )
    .ok()
}

/// MEDULLA-DECISION-ENGINE-R813.2/R813.3: when the request names a score source,
/// its corpus is built from the score rows the server read for the caller, through
/// `corpus_from_scores`. The client carries the corpus header only (reference
/// and privacy attestation); examples it supplies are refused, and the snapshot
/// version is the one the rows were read at.
#[cfg(feature = "program-optimization")]
fn fill_corpus_from_scores(
    request: &mut OptimizationRequest,
    score_rows: Option<&medulla_program::corpus::StoredScoreSet>,
    envelope: &PolicyEnvelope,
) -> Result<(), String> {
    if request.score_source.is_none() {
        return Ok(());
    }
    let Some(set) = score_rows else {
        return Err("INTERNAL: a score source was named but no score rows were read".to_string());
    };
    if !request.corpus.examples.is_empty() {
        return Err(
            "INVALID_ARGUMENT: a request that names a score source carries no examples".to_string(),
        );
    }
    let rows = super::score_source::corpus_rows(set, envelope)
        .map_err(|error| format!("INVALID_ARGUMENT: {error}"))?;
    let built = medulla_program::corpus::corpus_from_scores(
        &rows,
        &medulla_program::corpus::CorpusPolicy {
            corpus_ref: request.corpus.corpus_ref.clone(),
            snapshot_version: set.snapshot_version,
            privacy: request.corpus.privacy.clone(),
            envelope: envelope.clone(),
        },
    )
    .map_err(|refusal| refusal.to_string())?;
    request.corpus = built.corpus;
    if let Some(source) = request.score_source.as_mut() {
        source.skipped_without_run = built.skipped_without_run as u64;
    }
    Ok(())
}

/// The most term bindings one `UsageCluster` job carries.
const MAX_USAGE_TERM_BINDINGS: usize = 10_000;

/// The `JobKind::UsageCluster` arm of [`handle_submit`]'s kind-governance match: the caller
/// supplies the glossary bindings it can read; the owner and tenant scopes are the verified
/// caller's, replacing anything the request carried.
pub(super) fn submit_job_usage_cluster(
    authority: &CarrierAuthority,
    req_id: u64,
    term_bindings: Vec<medulla_work_types::jobs::UsageTermBinding>,
) -> Result<(JobKind, String, String, serde_json::Value), Response> {
    if term_bindings.len() > MAX_USAGE_TERM_BINDINGS
        || term_bindings.iter().any(|b| {
            b.asset.is_empty() || b.asset.len() > 256 || b.term.is_empty() || b.term.len() > 256
        })
    {
        return Err(Response::err(
            req_id,
            "INVALID_ARGUMENT: term bindings must name an asset and a term of 1 to 256 characters, at most 10000 of them",
        ));
    }
    let params = serde_json::json!({ "term_bindings": term_bindings.len() });
    Ok((
        JobKind::UsageCluster {
            term_bindings,
            owner_scope: authority.owner_scope().to_string(),
            tenant_scope: authority.tenant_scope().to_string(),
        },
        "usage.cluster".to_string(),
        "usage_cluster".to_string(),
        params,
    ))
}
/// The `JobKind::ProgramOptimize` arm of [`handle_submit`]'s kind-governance
/// match: bounded-decode the request, rebind its policy to the verified
/// caller, re-encode it, and ensure the native capability is required.
#[cfg(feature = "program-optimization")]
pub(super) fn submit_job_program_optimize(
    authority: &CarrierAuthority,
    req_id: u64,
    policy_fingerprint: &str,
    purpose: &str,
    request_msgpack: Vec<u8>,
    score_rows: Option<&medulla_program::corpus::StoredScoreSet>,
    required_capabilities: &mut Vec<String>,
) -> Result<(JobKind, String, String, serde_json::Value), Response> {
    if let Err(error) = check_program_optimizer_name(&request_msgpack) {
        return Err(Response::err(req_id, error));
    }
    let Some(mut request) = decode_optimization_request(&request_msgpack) else {
        return Err(Response::err(
            req_id,
            "program optimization request failed bounded decoding",
        ));
    };
    let verified_policy = match verified_program_policy(authority, policy_fingerprint, purpose) {
        Ok(policy) => policy,
        Err(error) => return Err(Response::err(req_id, error)),
    };
    if let Err(error) = fill_corpus_from_scores(&mut request, score_rows, &verified_policy) {
        return Err(Response::err(req_id, error));
    }
    if let Err(error) = check_program_request(&request) {
        return Err(Response::err(req_id, error));
    }
    let request = match request.rebind_program_policy(verified_policy) {
        Ok(request) => request,
        Err(error) => {
            return Err(Response::err(
                req_id,
                format!("program optimization request is invalid: {error}"),
            ));
        }
    };
    let optimizer = request.optimizer.as_str().to_string();
    let execution = request.optimizer.execution().as_str().to_string();
    let request_msgpack = match rmp_serde::to_vec_named(&request) {
        Ok(payload) => payload,
        Err(error) => {
            return Err(Response::err(
                req_id,
                format!("program optimization encoding failed: {error}"),
            ));
        }
    };
    ensure_program_optimization_capability(req_id, required_capabilities)?;
    let params = serde_json::json!({
        "optimizer": optimizer,
        "execution": execution,
        "request_ref": request.request_ref.as_str(),
        "corpus_ref": request.corpus.corpus_ref.as_str(),
        "snapshot_version": request.corpus.snapshot_version,
        "examples": request.corpus.examples.len(),
    });
    Ok((
        JobKind::ProgramOptimize { request_msgpack },
        "program.optimization".to_string(),
        optimizer,
        params,
    ))
}

/// Ensure `required_capabilities` names `program.optimization`, appending it
/// if there is room (native cap: 64 entries).
#[cfg(feature = "program-optimization")]
pub(super) fn ensure_program_optimization_capability(
    req_id: u64,
    required_capabilities: &mut Vec<String>,
) -> Result<(), Response> {
    if required_capabilities
        .iter()
        .any(|capability| capability == "program.optimization")
    {
        return Ok(());
    }
    if required_capabilities.len() == 64 {
        return Err(Response::err(
            req_id,
            "analytics job has no room for its required native capability",
        ));
    }
    required_capabilities.push("program.optimization".to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::MAX_ASSOCIATION_DISTINCT_ITEMS;

    // spec: MEDULLA-PLACEMENT-R003.1
    #[test]
    fn association_distinct_item_limit_matches_rule_mask_width() {
        assert_eq!(MAX_ASSOCIATION_DISTINCT_ITEMS, 31);
    }
}

#[cfg(test)]
mod association_refusal_tests {
    use super::{validate_association_cardinality, validate_association_thresholds};

    // spec: MEDULLA-PLACEMENT-R003.2
    #[test]
    fn cardinality_and_threshold_payloads_have_distinct_typed_refusals() {
        for (count, support, confidence, expected) in [
            (
                32,
                0.1,
                0.5,
                "association job exceeds the distinct-item limit",
            ),
            (2, -0.1, 0.5, "association job thresholds are invalid"),
            (2, 0.1, f64::NAN, "association job thresholds are invalid"),
            (
                32,
                f64::INFINITY,
                0.5,
                "association job thresholds are invalid",
            ),
        ] {
            let transactions = vec![(0..count).map(|i| format!("item:{i}")).collect()];
            let refusal = validate_association_thresholds(302, support, confidence)
                .and_then(|()| validate_association_cardinality(302, &transactions))
                .unwrap_err();
            assert_eq!(refusal.id, 302);
            assert_eq!(refusal.error.as_deref(), Some("INVALID_ARGUMENT"));
            assert_eq!(refusal.error_detail.as_deref(), Some(expected));
            assert!(refusal.result.is_none());
        }
        let at_limit: Vec<String> = (0..31).map(|i| format!("item:{i}")).collect();
        let duplicates = vec![at_limit.clone(), at_limit];
        assert!(validate_association_thresholds(302, 0.0, 1.0).is_ok());
        assert!(validate_association_cardinality(302, &duplicates).is_ok());
    }
}
