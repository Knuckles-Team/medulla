//! Engine-held records reach the metadata catalog through the one emitter
//! (MEDULLA-TYPED-PACKS-R108.3).
//!
//! An Agent Library publish, a workflow registration and a graph creation each hold a
//! record the engine owns. After the record is durable, `emit_asset` turns it into the
//! catalog triples that describe it and the triples are committed as one served
//! `AddTriples` on the graph the request was issued on, so the catalog write guard of
//! `R108.4` and the ranking index of `R108.16` see them like any other asset. Connector
//! asset nodes stay with the arbora batch; no family keeps its own index.

use super::*;

/// Emit one record's catalog nodes into `graph`. A refusal text means the record is durable
/// but its catalog nodes are not; replaying the same request emits them again (the triples
/// are idempotent).
#[cfg(feature = "rdf")]
pub(super) async fn commit_asset(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified_context: &VerifiedRequestContext,
    graph: &str,
    record: medulla_catalog::emit::AssetRecord<'_>,
) -> Result<(), String> {
    let triples = medulla_catalog::emit::emit_asset(record, &medulla_catalog::emit::AssetLinks::default())
        .map_err(|error| error.to_string())?;
    let response = dispatch_graph_op(
        state,
        graph,
        req.id,
        req.agent_id.as_deref(),
        verified_context,
        Method::AddTriples {
            turtle: String::new(),
            ntriples: medulla_catalog::emit::to_ntriples(&triples),
        },
    )
    .await;
    match response.refusal_text() {
        Some(refusal) => Err(format!("CATALOG_EMIT_FAILED: {refusal}")),
        None => Ok(()),
    }
}

/// Answer `response` unchanged when the catalog nodes committed, else the emit refusal.
#[cfg(feature = "rdf")]
pub(super) fn after_emit(response: Response, req_id: u64, emitted: Result<(), String>) -> Response {
    match emitted {
        Ok(()) => response,
        Err(refusal) => Response::err(req_id, refusal),
    }
}
