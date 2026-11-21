//! `Method::BranchMerge` (MEDULLA-BRANCH-MERGE-R003, R007): plan, then apply.
//!
//! `h-graph` plans the merge and never calls the router: `publish_merge`
//! answers a `MergeApplyPlan` (the target-graph mutation batch, the history
//! rows and the head move). This arm commits it with `MergeApplyPlan::commit`:
//! a stale target head is refused first, the batch goes through the one graph
//! mutation compile path as one `BatchUpdate` under the caller's verified
//! context, then the registry transaction writes the history rows, the commit
//! rows and the head move under the merge's replay key. A merge whose registry
//! write failed is retried by running it again. A merge into `main` writes the
//! base graph and has no registry half.

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
    use medulla_h_graph::graph_ops::branch::publish::MergeCommitError;
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
    let effects = RouterMergeEffects {
        state,
        req,
        verified,
    };
    match plan.commit(&effects).await {
        Ok(receipt) => ops::merge_receipt_response(req.id, receipt),
        Err(MergeCommitError::Graph(refusal)) => Response::err(req.id, refusal),
        Err(MergeCommitError::Store(detail)) => Response::err(req.id, detail),
        Err(MergeCommitError::Head(HeadMoveError::Branch(refusal))) => Response::err(
            req.id,
            format!("{}: branch merge refused", refusal.code().as_str()),
        ),
        Err(MergeCommitError::Head(HeadMoveError::Stale)) => Response::err(
            req.id,
            format!(
                "{}: branch merge refused",
                medulla_types::contract::ServerErrorCode::MergeTargetStale.as_str()
            ),
        ),
        Err(MergeCommitError::Head(HeadMoveError::Store(detail))) => Response::err(req.id, detail),
        Err(MergeCommitError::Finish(error)) => ops::publish_failure_response(req.id, error),
    }
}

/// The graph half of a merge: one `BatchUpdate` under the caller's verified context through the
/// one graph mutation compile path. The registry half is the trait's default transaction.
#[cfg(feature = "redb")]
struct RouterMergeEffects<'a> {
    state: &'a Arc<RwLock<ServerState>>,
    req: &'a DispatchHeader,
    verified: &'a VerifiedRequestContext,
}

#[cfg(feature = "redb")]
impl medulla_h_graph::graph_ops::branch::publish::MergeEffects for RouterMergeEffects<'_> {
    async fn apply_graph(
        &self,
        graph: &str,
        operations: &[serde_json::Value],
        create_graph: bool,
    ) -> Result<(), String> {
        if create_graph {
            super::branch_write::ensure_target_graph(self.state, self.req, self.verified, graph)
                .await
                .map_err(|refused| {
                    refused.refusal_text().unwrap_or_else(|| {
                        "branch merge target graph could not be created".to_string()
                    })
                })?;
        }
        let operations_msgpack = rmp_serde::to_vec_named(operations)
            .map_err(|_| "merge batch encoding failed".to_string())?;
        let applied = dispatch_boxed(dispatch_graph_op(
            self.state,
            graph,
            self.req.id,
            self.req.agent_id.as_deref(),
            self.verified,
            Method::BatchUpdate { operations_msgpack },
        ))
        .await;
        match applied.refusal_text() {
            Some(refusal) => Err(refusal),
            None => Ok(()),
        }
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
