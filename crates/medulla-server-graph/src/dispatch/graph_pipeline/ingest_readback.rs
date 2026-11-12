//! The graph-scoped route of `VerifyIngestReadback` (MEDULLA-REPO-INGEST-R002.100).
//!
//! Arrives after graph ACL, tenant binding, lazy-open and placement lookup. The
//! reads run under the CALLER's row authority, never the writer's. Only a
//! `Matched` comparison commits (one `:ReadbackReceipt` node as one
//! ChangeEnvelope); every refusal writes nothing.

use super::repository_index::commit_context;
use super::*;
use medulla_h_ingest::ingest_readback as readback;

/// Route `VerifyIngestReadback`; hand any other method back.
pub async fn route_ingest_readback(
    ctx: GraphOpRouting<'_>,
    method: Method,
) -> Result<Response, Method> {
    match method {
        Method::VerifyIngestReadback { request } => {
            Ok(handle_verify_ingest_readback(ctx, *request).await)
        }
        other => Err(other),
    }
}

/// The caller's RLS-filtered view of the graph core.
struct CallerView<'a, 'b> {
    ctx: &'a GraphOpRouting<'b>,
}

impl readback::ReadbackView for CallerView<'_, '_> {
    fn graph_revision(&self) -> u64 {
        self.ctx.core.version()
    }

    fn read(&self, id: &str) -> readback::NodeRead {
        let Some(blob) = self.ctx.core.get_node_properties(id) else {
            return readback::NodeRead::Absent;
        };
        let visible = self
            .ctx
            .read_authority
            .as_ref()
            .is_none_or(|authority| authority.node_visible(self.ctx.core, id));
        if !visible {
            return readback::NodeRead::Denied;
        }
        let (content_digest, revision) = readback::observe(&blob);
        readback::NodeRead::Visible {
            content_digest,
            revision,
        }
    }
}

/// Compare the receipt with the caller's reads, then commit the readback node.
pub async fn handle_verify_ingest_readback(
    ctx: GraphOpRouting<'_>,
    request: medulla_types::ingest_readback::VerifyIngestReadbackRequest,
) -> Response {
    let req_id = ctx.req_id;
    let receipt = {
        let view = CallerView { ctx: &ctx };
        match readback::verify(&request, ctx.verified_actor, &view) {
            Ok(receipt) => receipt,
            Err(refusal) => return Response::err(req_id, refusal.to_string()),
        }
    };
    let write_set = match readback::lower(&receipt) {
        Ok(write_set) => write_set,
        Err(error) => return Response::err(req_id, error),
    };
    let (placement_epoch, fencing_token) = match source_ingestion_route_fence(&ctx).await {
        Ok(fence) => fence,
        Err(response) => return response,
    };
    let commit_ctx = commit_context(&ctx, placement_epoch, fencing_token);
    let envelope = match medulla_h_ingest::repository_index::build_labelled_envelope(
        &commit_ctx,
        write_set,
        &readback::LABELS,
    ) {
        Ok(envelope) => envelope,
        Err(error) => return Response::err(req_id, error),
    };
    let committed =
        crate::dispatch::change_envelope::apply_one_change_envelope(ctx, envelope).await;
    match committed.refusal_text() {
        Some(refusal) => Response::err(committed.id, refusal),
        None => Response::ok(
            committed.id,
            ResultPayload::of::<
                medulla_protocol_types::result_contract::ingestion::VerifyIngestReadback,
            >(receipt),
        ),
    }
}
