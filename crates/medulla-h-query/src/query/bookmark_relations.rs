//! `Method::BookmarkRelations` (MEDULLA-MEDIA-VOCAB-R001.102.3): the authorized relations among
//! bookmark nodes, read from the caller's row-projected core at one pinned snapshot.
//!
//! Rows the caller cannot read are not in the projected core, so an edge whose source, target or
//! own row is hidden is never reached, and nothing is counted before that filter. The handler
//! stops at `max_relationships` and refuses `BOOKMARK_LIMIT_EXCEEDED` rather than answer a
//! truncated success. Nothing here writes.

use super::code_context::{out_edges, props_of, relationship, text};
use super::*;
use medulla_protocol_types::result_contract::query::BookmarkRelations;
use medulla_types::bookmark_query::{
    BookmarkBinding, BookmarkEdge, BookmarkRelationsRequest, BookmarkRelationsResult,
    BOOKMARK_LIMIT_EXCEEDED, BOOKMARK_RESPONSE_TOO_LARGE,
};
use medulla_types::contract::Digest256;
use medulla_types::refusal_code::RefusalCode;
use std::collections::BTreeSet;

/// The relationships a bookmark edge may carry: the ingest edges and the mindmap edge.
const RELATIONSHIPS: [&str; 5] = [
    "taggedWith",
    "mentionsAccount",
    "usesTool",
    "linksToDomain",
    "relatedTo",
];

fn refusal(code: RefusalCode, reason: &str) -> String {
    format!("{}: {reason}", code.as_str())
}

fn digest_of<T: serde::Serialize>(value: &T) -> Result<Digest256, String> {
    rmp_serde::to_vec_named(value)
        .map(|bytes| Digest256::sha256(&bytes))
        .map_err(|error| format!("bookmark digest encoding failed: {error}"))
}

/// Read the relations of `request.node_ids` at `snapshot` of `core`.
pub(crate) fn relations(
    core: &GraphCore,
    request: &BookmarkRelationsRequest,
    authorization_revision: u64,
) -> Result<BookmarkRelationsResult, String> {
    request
        .validate()
        .map_err(|code| refusal(code, "the request is invalid"))?;
    // One version for every read of the call.
    let graph_snapshot_revision = core.version();
    let cap = request.limit_policy.max_relationships as usize;
    let mut edges: Vec<BookmarkEdge> = Vec::new();
    let mut sources: BTreeSet<String> = BTreeSet::new();
    let mut newest: Option<(u64, String, String, String)> = None;
    for node_id in &request.node_ids {
        let Some(node) = props_of(core, node_id) else {
            continue;
        };
        let source_id = text(&node, "source_id");
        if !source_id.is_empty() {
            sources.insert(source_id);
        }
        let offset = text(&node, "source_offset").parse::<u64>().unwrap_or(0);
        if newest.as_ref().map_or(true, |(best, ..)| offset >= *best) {
            newest = Some((
                offset,
                text(&node, "ingestion_receipt_id"),
                text(&node, "ingestion_receipt_digest"),
                text(&node, "source_revision"),
            ));
        }
        for (target, edge) in out_edges(core, node_id) {
            let kind = relationship(&edge);
            if !RELATIONSHIPS.contains(&kind.as_str()) || props_of(core, &target).is_none() {
                continue;
            }
            if edges.len() == cap {
                return Err(refusal(
                    BOOKMARK_LIMIT_EXCEEDED,
                    "the relations are above max_relationships",
                ));
            }
            let mindmap = text(&edge, "mindmap_id");
            edges.push(BookmarkEdge {
                source_node_id: node_id.clone(),
                target_node_id: target,
                relationship: kind,
                provenance: (!mindmap.is_empty()).then_some(mindmap),
            });
        }
    }
    let (_, ingestion_receipt_id, ingestion_receipt_digest, source_revision) =
        newest.unwrap_or_default();
    let mut without_cursor = request.clone();
    without_cursor.cursor = None;
    let result = BookmarkRelationsResult {
        edges,
        binding: BookmarkBinding {
            source_ids: sources.into_iter().collect(),
            ingestion_receipt_id,
            ingestion_receipt_digest,
            source_revision,
            // The relations read no semantic index.
            index_revision: 0,
            query_digest: digest_of(&without_cursor)?,
            graph_snapshot_revision,
            authorization_revision,
            limit_policy_digest: digest_of(&request.limit_policy)?,
        },
    };
    let size = rmp_serde::to_vec_named(&result)
        .map_err(|error| format!("bookmark response encoding failed: {error}"))?
        .len();
    if size > request.limit_policy.max_response_bytes as usize {
        return Err(refusal(
            BOOKMARK_RESPONSE_TOO_LARGE,
            "the response is above max_response_bytes",
        ));
    }
    Ok(result)
}

/// Serve one `BookmarkRelations`.
pub(crate) fn handle_bookmark_relations(
    ctx: &QueryHandlerCtx<'_>,
    request: BookmarkRelationsRequest,
) -> Response {
    #[cfg(feature = "security")]
    let authorization_revision = ctx.rls.policy_revision();
    #[cfg(not(feature = "security"))]
    let authorization_revision = 0;
    match relations(ctx.core, &request, authorization_revision) {
        Err(error) => Response::err(ctx.req_id, error),
        Ok(answer) => served_response::<BookmarkRelations>(
            ctx.req_id,
            Ok(Ok(answer)),
            "BookmarkRelations",
            #[cfg(feature = "result-cache")]
            ctx.core,
            #[cfg(feature = "result-cache")]
            None,
        ),
    }
}
