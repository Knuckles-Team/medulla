//! Graph-handler commit gateway shared by crates below `medulla-h-graph`.

use medulla_core::graph::GraphCore;
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use crate::mutation;
use crate::mutation::MutationCtx;
use crate::mutation::MutationPlan;

/// Keep the mutation kernel behind a heap indirection so this module's large
/// gateway match does not embed one copy of the kernel future in every arm.
/// Without this boundary the generated `try_handle_gateway` future exceeds
/// Tokio's default worker-thread stack on ordinary mutation requests.
pub async fn commit_gateway<F>(
    ctx: &MutationCtx<'_>,
    plan: &MutationPlan,
    method: &Method,
    apply: F,
) -> Response
where
    F: FnOnce(&GraphCore) -> Result<ResultPayload, String>,
{
    Box::pin(mutation::commit_mutation(ctx, plan, method, apply)).await
}
