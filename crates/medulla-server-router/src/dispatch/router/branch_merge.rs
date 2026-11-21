//! `Method::BranchMerge` (MEDULLA-BRANCH-MERGE-R003, R007): plan, then apply.
//!
//! `h-graph` plans the merge and never calls the router: `publish_merge`
//! answers a `MergeApplyPlan` (the target-graph mutation batch, the history
//! rows and the head move). This arm applies it the way the decision plane
//! commits a `PlannedWrite`: the batch goes through the one graph mutation
//! compile path as one `BatchUpdate` under the caller's verified context, then
//! the registry transaction writes the history rows, the commit rows and the
//! head move. Every refusal that can be decided before the data graph changes
//! (a stale target head) is decided first.

use super::*;

/// The wire fields of `Method::BranchMerge`.
pub type BranchMergeFields = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<medulla_decision_types::decision::DecisionPolicyRef>,
);

/// Serve `Method::BranchMerge`.
#[cfg(feature = "redb")]
pub async fn handle_branch_merge(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    fields: BranchMergeFields,
) -> Response {
    use medulla_h_graph::graph_ops::branch as ops;
    use medulla_persistence::branch_registry_store::HeadMoveError;
    let (tenant, graph, source, target, merge_base, policy) = fields;
    let request = ops::BranchMergeRequest {
        tenant,
        graph,
        source,
        target,
        merge_base,
        policy,
    };
    let step = match ops::plan_branch_merge(state, req.id, verified, request).await {
        Ok(step) => step,
        Err(refusal) => return refusal,
    };
    let plan = match step {
        ops::publish::MergeStep::Done(receipt) => {
            return ops::merge_receipt_response(req.id, receipt)
        }
        ops::publish::MergeStep::Apply(plan) => *plan,
    };
    let stale = || {
        Response::err(
            req.id,
            format!(
                "{}: branch merge refused",
                medulla_types::contract::ServerErrorCode::MergeTargetStale.as_str()
            ),
        )
    };
    match plan
        .target
        .head_is(&plan.tenant, &plan.expected_target_head)
        .await
    {
        Ok(true) => {}
        Ok(false) => return stale(),
        Err(error) => return Response::err(req.id, error),
    }
    if !plan.operations.is_empty() {
        if let Err(refused) =
            super::branch_write::ensure_target_graph(state, req, verified, &plan.target.target_graph)
                .await
        {
            return refused;
        }
        let operations_msgpack = match rmp_serde::to_vec_named(&plan.operations) {
            Ok(bytes) => bytes,
            Err(_) => return Response::err(req.id, "merge batch encoding failed"),
        };
        let applied = dispatch_boxed(dispatch_graph_op(
            state,
            &plan.target.target_graph,
            req.id,
            req.agent_id.as_deref(),
            verified,
            Method::BatchUpdate { operations_msgpack },
        ))
        .await;
        if let Some(refusal) = applied.refusal_text() {
            return Response::err(req.id, refusal);
        }
    }
    let appended = plan
        .target
        .publish_merge(
            &plan.source,
            &plan.tenant,
            &plan.expected_target_head,
            plan.write.clone(),
        )
        .await;
    let appended = match appended {
        Ok(appended) => appended,
        Err(HeadMoveError::Branch(refusal)) => {
            return Response::err(
                req.id,
                format!("{}: branch merge refused", refusal.code().as_str()),
            )
        }
        Err(HeadMoveError::Stale) => return stale(),
        Err(HeadMoveError::Store(detail)) => return Response::err(req.id, detail),
    };
    match plan.finish(appended).await {
        Ok(receipt) => ops::merge_receipt_response(req.id, receipt),
        Err(error) => ops::publish_failure_response(req.id, error),
    }
}

/// `BranchMerge` needs the `redb` registry owner file.
#[cfg(not(feature = "redb"))]
pub async fn handle_branch_merge(
    _state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    _verified: &VerifiedRequestContext,
    _fields: BranchMergeFields,
) -> Response {
    Response::err(req.id, "BranchMerge requires the `redb` feature")
}
