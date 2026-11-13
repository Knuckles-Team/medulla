//! `Method::BranchWrite` (MEDULLA-BRANCH-MERGE-R002): one mutation on a branch.
//!
//! The mutation commits through the ordinary graph path to the branch's own
//! target graph, so `main` and sibling branches never see it. One
//! `BranchCommit` is then appended to the branch's single commit chain and the
//! head advances in the same registry transaction. Every refusal precedes the
//! graph change.

#[cfg(feature = "redb")]
use super::lifecycle::{create_graph, GraphLifecycleRequest};
use super::*;
#[cfg(feature = "redb")]
use medulla_server_graph::dispatch::shared::timed_read;

/// The wire fields of `Method::BranchWrite`.
pub struct BranchWriteFields {
    pub tenant: String,
    pub graph: String,
    pub branch: String,
    pub mutation: Method,
}

#[cfg(feature = "redb")]
/// Idempotency key under which every target graph is created, so a retry of a
/// first write reconciles instead of colliding.
const TARGET_CREATE_KEY: &str = "branch-target-graph";

#[cfg(feature = "redb")]
/// A mutation a branch accepts: a graph mutation the caller is allowed, that
/// is neither a lifecycle operation nor another branch operation.
fn check_mutation(
    mutation: &Method,
    verified: &VerifiedRequestContext,
) -> Result<(), &'static str> {
    if matches!(
        mutation,
        Method::BranchWrite { .. }
            | Method::CreateBranch { .. }
            | Method::CreateGraph { .. }
            | Method::DeleteGraph { .. }
            | Method::MultiGraphBatchUpdate { .. }
    ) {
        return Err("a branch write takes one graph mutation, not a lifecycle or branch operation");
    }
    let policy = medulla_capabilities::policy(mutation);
    if !policy.mutates {
        return Err("a branch write takes a mutation, not a read");
    }
    if !verified.allows_method(policy.authz_action, true) {
        return Err("the verified request context lacks the scope of the inner mutation");
    }
    Ok(())
}

#[cfg(feature = "redb")]
/// Create the branch's target graph on the first write.
async fn ensure_target_graph(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    target: &str,
) -> Result<(), Response> {
    if timed_read(state).await.registry.exists(target) {
        return Ok(());
    }
    let request = GraphLifecycleRequest::verified(
        req.id,
        req.agent_id.clone(),
        verified,
        TARGET_CREATE_KEY.to_string(),
    )
    .map_err(|denied| Response::err(req.id, denied))?;
    let created = create_graph(
        state,
        request,
        target.to_string(),
        medulla_protocol_types::protocol::GraphType::Agent,
    )
    .await;
    match created.refusal_text() {
        Some(_) => Err(created),
        None => Ok(()),
    }
}

/// Serve `Method::BranchWrite`.
#[cfg(feature = "redb")]
pub async fn handle_branch_write(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    fields: BranchWriteFields,
) -> Response {
    use medulla_h_graph::graph_ops::branch as ops;
    let BranchWriteFields {
        tenant,
        graph,
        branch,
        mutation,
    } = fields;
    if let Err(detail) = check_mutation(&mutation, verified) {
        return ops::invalid_write(req.id, detail);
    }
    let reference = medulla_types::branch::BranchRef {
        tenant: tenant.clone(),
        graph: graph.clone(),
        name: branch.clone(),
    };
    let target = match ops::resolve_target(state, verified, reference).await {
        Ok(target) => target,
        Err(error) => return ops::target_error_response(req.id, error),
    };
    let batch_id = medulla_server_api::batch_compile::branch_batch_id(
        verified.tenant(),
        &graph,
        &branch,
        Some(verified.principal()),
        verified.idempotency_key(),
    );
    match target.replayed(&batch_id).await {
        Ok(Some(commit)) => {
            let appended = medulla_persistence::branch_registry_store::AppendedCommit {
                commit,
                replayed: true,
            };
            return ops::receipt_response(req.id, target.receipt(appended));
        }
        Ok(None) => {}
        Err(error) => return Response::err(req.id, error),
    }
    let payload = match ops::mutation_payload(&mutation) {
        Ok(bytes) => bytes,
        Err(error) => return Response::err(req.id, error),
    };
    let manifest_digest = ops::payload_digest(&payload);
    if let Err(refused) = ensure_target_graph(state, req, verified, &target.target_graph).await {
        return refused;
    }
    let applied = dispatch_boxed(dispatch_graph_op(
        state,
        &target.target_graph,
        req.id,
        req.agent_id.as_deref(),
        verified,
        mutation,
    ))
    .await;
    if applied.refusal_text().is_some() {
        return applied;
    }
    let engine_version = timed_read(state)
        .await
        .registry
        .get(&target.target_graph)
        .map(|entry| entry.core.version())
        .unwrap_or(0);
    let input = medulla_persistence::branch_registry_store::BranchCommitInput {
        batch_id,
        engine_version,
        manifest_digest,
        actor: verified.principal().to_string(),
        created_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0),
        // The mutation is the commit's payload, so every version of the chain
        // reads and diffs (MEDULLA-BRANCH-MERGE-R006).
        payload: Some(payload),
    };
    match target.append(verified.tenant(), input).await {
        Ok(appended) => ops::receipt_response(req.id, target.receipt(appended)),
        Err(error) => ops::append_error_response(req.id, error),
    }
}

/// `BranchWrite` needs the `redb` registry owner file.
#[cfg(not(feature = "redb"))]
pub async fn handle_branch_write(
    _state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    _verified: &VerifiedRequestContext,
    _fields: BranchWriteFields,
) -> Response {
    Response::err(req.id, "BranchWrite requires the `redb` feature")
}
