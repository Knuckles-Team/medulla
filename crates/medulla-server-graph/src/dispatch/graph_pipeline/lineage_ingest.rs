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
    let lowered = lower(&ctx, events, &known_edge).await;
    let (write_set, receipt) = match lowered {
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

/// Lower the batch, resolving the column lineage of every event that carries SQL against the
/// caller's own held catalog of the event's `catalog_source` (MEDULLA-UNIFIED-DATA-PLANE-R763.3).
async fn lower(
    ctx: &GraphOpRouting<'_>,
    events: Vec<medulla_h_ingest::lineage::LineageEvent>,
    known_edge: &(dyn Fn(&str, &str) -> bool + Sync),
) -> Result<
    (
        medulla_h_ingest::repository_index::IndexWriteSet,
        medulla_h_ingest::lineage::LineageIngestReceipt,
    ),
    String,
> {
    use medulla_h_ingest::lineage::{self, ColumnDeriveFailure, ColumnLineageSeam};
    #[cfg(feature = "federation")]
    use medulla_server_ops::api::query_txn::ForeignStateExt;
    let core = ctx.core;
    let known_column_edge = |source: &str, target: &str, digest: &str| {
        core.get_edge_properties(&format!("col:{target}"), &format!("col:{source}"))
            .iter()
            .filter_map(|bytes| rmp_serde::from_slice::<serde_json::Value>(bytes).ok())
            .any(|props| props.get("query_digest").and_then(|d| d.as_str()) == Some(digest))
    };
    if !events.iter().any(|event| event.sql_text.is_some()) {
        let none = ColumnLineageSeam {
            derive: &|_, _, _, _| Ok(lineage::LineageResult::default()),
            known_column_edge: &known_column_edge,
        };
        return lineage::handle_lineage_ingest(events, known_edge, &none);
    }
    let seam = medulla_server_ops::api::query_txn::query_seam(ctx.state).await;
    let state = ctx.state.read().await;
    let carrier = ctx
        .read_authority
        .as_ref()
        .and_then(|authority| authority.carrier());
    let derive = |sql: &str, dialect, source: &str, target: &str| {
        let seam = seam.as_ref().ok_or_else(|| {
            ColumnDeriveFailure::Refused("ENGINE_UNAVAILABLE: no query seam installed".to_string())
        })?;
        #[cfg(feature = "federation")]
        let catalog = {
            let carrier = carrier.ok_or_else(|| {
                ColumnDeriveFailure::Refused(
                    "ACCESS_DENIED: column lineage needs a verified caller".to_string(),
                )
            })?;
            state
                .foreign_sources()
                .catalog_read(carrier, source)
                .map_err(|refusal| {
                    if refusal.starts_with("SCHEMA_DRIFT") {
                        ColumnDeriveFailure::Refused(refusal)
                    } else {
                        ColumnDeriveFailure::Refused(format!("LINEAGE_CATALOG_MISSING: {refusal}"))
                    }
                })?
        };
        #[cfg(not(feature = "federation"))]
        let catalog = {
            let _ = (&state, carrier, source);
            return Err(ColumnDeriveFailure::Refused(
                "LINEAGE_CATALOG_MISSING: this build holds no attached catalogs".to_string(),
            ));
        };
        #[cfg(feature = "federation")]
        seam.derive_column_lineage(sql, dialect, &catalog, target)
            .map_err(|error| match error {
                lineage::LineageError::Parse(ref detail)
                    if detail.starts_with("ENGINE_UNAVAILABLE") =>
                {
                    ColumnDeriveFailure::Refused(detail.clone())
                }
                other => ColumnDeriveFailure::Statement(other),
            })
    };
    let seam = ColumnLineageSeam {
        derive: &derive,
        known_column_edge: &known_column_edge,
    };
    lineage::handle_lineage_ingest(events, known_edge, &seam)
}
