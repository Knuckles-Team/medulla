//! The graph-scoped route of `LineageIngest` (MEDULLA-UNIFIED-DATA-PLANE-R702.3).
//!
//! It arrives after graph ACL, tenant binding, lazy-open and placement lookup,
//! lowers one drained batch with `medulla_h_ingest::lineage` and commits it
//! through the existing ChangeEnvelope authority, exactly like
//! `EnrichDocument`.

use super::repository_index::commit_context;
use super::*;

/// Drain, lower and commit one served batch of lineage events.
pub async fn route_lineage_ingest(
    ctx: GraphOpRouting<'_>,
    events: Vec<medulla_h_ingest::lineage::LineageEvent>,
) -> Response {
    use medulla_h_ingest::lineage;
    let core = ctx.core;
    let known_edge = |source: &str, target: &str| core.has_edge(source, target);
    let (write_set, receipt) = match lineage::handle_lineage_ingest(events, &known_edge) {
        Ok(lowered) => lowered,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    if write_set.methods.is_empty() {
        return respond(ctx.req_id, receipt);
    }
    let (placement_epoch, fencing_token) = match source_ingestion_route_fence(&ctx).await {
        Ok(fence) => fence,
        Err(response) => return response,
    };
    let commit_ctx = commit_context(&ctx, placement_epoch, fencing_token);
    let envelope = match medulla_h_ingest::repository_index::build_labelled_envelope(
        &commit_ctx,
        write_set,
        &lineage::LABELS,
    ) {
        Ok(envelope) => envelope,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    let committed =
        crate::dispatch::change_envelope::apply_one_change_envelope(ctx, envelope).await;
    match committed.refusal_text() {
        Some(refusal) => Response::err(committed.id, refusal),
        None => respond(committed.id, receipt),
    }
}

fn respond(req_id: u64, receipt: medulla_h_ingest::lineage::LineageIngestReceipt) -> Response {
    Response::ok(
        req_id,
        ResultPayload::of::<medulla_protocol_types::result_contract::ingestion::LineageIngest>(
            receipt,
        ),
    )
}
