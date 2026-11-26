//! `Method::BookmarkRelations` (MEDULLA-MEDIA-VOCAB-R001.102.3): the authorized relations among
//! bookmark nodes, read through the one UQL statement runner at one pinned snapshot.
//!
//! The read is an `EDGES` view over the caller's served snapshot, run by [`handle_uql`] exactly as
//! `Method::Uql` runs it, so the caller's row visibility narrows the edges before anything is
//! counted. An edge whose own row, source or target the caller cannot read is never returned and
//! never counted. A page holds at most `max_relationships` edges and, when more follow, carries a
//! MAC-bound cursor on its last edge; a request that reaches past the read window refuses
//! `BOOKMARK_LIMIT_EXCEEDED` rather than answer a truncated success. Nothing here writes.

use super::code_context::{out_edges, props_of, relationship, text};
use super::live_artifact::response_body;
use super::uql_statement::handle_uql;
use super::*;
use medulla_h_retrieve::bookmark::{
    bookmark_index_revision, decode_cursor, digest_of, encode_cursor, refusal, CursorBinding,
    RELATIONS_CURSOR_DOMAIN,
};
use medulla_protocol_types::result_contract::query::BookmarkRelations;
use medulla_query_types::wire::{PredLiteral, UqlParam, UqlResult};
use medulla_types::bookmark_query::{
    BookmarkBinding, BookmarkEdge, BookmarkRelationsRequest, BookmarkRelationsResult,
    BOOKMARK_LIMIT_EXCEEDED, BOOKMARK_REVISION_MISMATCH, BOOKMARK_RESPONSE_TOO_LARGE,
};
use std::collections::BTreeSet;

/// The relationships a bookmark edge may carry: the ingest edges and the mindmap edge.
const RELATIONSHIPS: [&str; 5] = [
    "taggedWith",
    "mentionsAccount",
    "usesTool",
    "linksToDomain",
    "relatedTo",
];

/// The most edges one read window holds, before paging, from the knob
/// `bookmark.relations_window` (5000 by default). A larger answer refuses.
fn window() -> usize {
    usize::try_from(medulla_server_api::knob_config::knobs().bookmark_relations_window)
        .unwrap_or(usize::MAX - 1)
}

/// The UQL statement of one read: the edges that leave the requested nodes, in id order.
fn statement(window: usize) -> String {
    let relationships = RELATIONSHIPS
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "EDGES WHERE source IN $ids AND rel IN ({relationships}) |> ORDER BY id |> LIMIT {}",
        window + 1
    )
}

/// The `(source node, target node, position)` an `EDGES` row id `source|target|ord` names.
fn parse_edge_id<'a>(id: &'a str, node_ids: &'a [String]) -> Option<(&'a str, &'a str, usize)> {
    let (pair, ord) = id.rsplit_once('|')?;
    let ord = ord.parse().ok()?;
    let source = node_ids
        .iter()
        .filter(|node| {
            pair.strip_prefix(node.as_str())
                .is_some_and(|rest| rest.len() > 1 && rest.starts_with('|'))
        })
        .max_by_key(|node| node.len())?;
    Some((source, &pair[source.len() + 1..], ord))
}

/// Read the relations of `request.node_ids` at the snapshot of `ctx.core`.
async fn relations(
    ctx: &QueryHandlerCtx<'_>,
    request: &BookmarkRelationsRequest,
    tenant: &str,
    index_revision: u64,
    authorization_revision: u64,
) -> Result<BookmarkRelationsResult, String> {
    request
        .validate()
        .map_err(|code| refusal(code, "the request is invalid"))?;
    let core = ctx.core;
    // One version for every read of the call.
    let graph_snapshot_revision = core.version();
    let mut without_cursor = request.clone();
    without_cursor.cursor = None;
    let query_digest = digest_of(&without_cursor)?;
    let limit_policy_digest = digest_of(&request.limit_policy)?;
    let cursor_binding = CursorBinding {
        domain: RELATIONS_CURSOR_DOMAIN,
        tenant,
        query_digest: &query_digest,
        index_revision,
        graph_snapshot_revision,
    };
    let offset = match &request.cursor {
        Some(cursor) => decode_cursor(&cursor_binding, cursor)?,
        None => 0,
    };
    let cap = request.limit_policy.max_relationships as usize;
    let window = window();
    if offset.saturating_add(cap) > window {
        return Err(refusal(
            BOOKMARK_LIMIT_EXCEEDED,
            "the page lies beyond the read window",
        ));
    }

    let mut params = UqlParams::new();
    params.insert(
        "ids".to_string(),
        UqlParam::List(
            request
                .node_ids
                .iter()
                .map(|id| PredLiteral::Str(id.clone()))
                .collect(),
        ),
    );
    let response = handle_uql(ctx, statement(window), params, false, Default::default(), None)
        .await
        .map_err(|_| "ENGINE_UNAVAILABLE: UQL is not served in this build".to_string())?;
    let rows = match response_body::<UqlResult>(response)? {
        UqlResult::Rows {
            rows,
            snapshot_revision,
            ..
        } => {
            if snapshot_revision != graph_snapshot_revision {
                return Err(refusal(
                    BOOKMARK_REVISION_MISMATCH,
                    "the graph moved during the call",
                ));
            }
            rows
        }
        _ => return Err("the bookmark relations read did not return rows".to_string()),
    };
    if rows.len() > window {
        return Err(refusal(
            BOOKMARK_LIMIT_EXCEEDED,
            "the relations are above the read window",
        ));
    }

    // Both endpoints must be rows the caller can read; this filters before anything is counted.
    let mut edges: Vec<BookmarkEdge> = Vec::new();
    for row in &rows {
        let Some((source, target, position)) = parse_edge_id(&row.id, &request.node_ids) else {
            continue;
        };
        if props_of(core, source).is_none() || props_of(core, target).is_none() {
            continue;
        }
        let Some((_, edge)) = out_edges(core, source)
            .into_iter()
            .filter(|(to, _)| to == target)
            .nth(position)
        else {
            continue;
        };
        let kind = relationship(&edge);
        if !RELATIONSHIPS.contains(&kind.as_str()) {
            continue;
        }
        let mindmap = text(&edge, "mindmap_id");
        edges.push(BookmarkEdge {
            source_node_id: source.to_string(),
            target_node_id: target.to_string(),
            relationship: kind,
            provenance: (!mindmap.is_empty()).then_some(mindmap),
            cursor: None,
        });
    }
    let page: Vec<BookmarkEdge> = edges.into_iter().skip(offset).take(cap + 1).collect();
    let truncated = page.len() > cap;
    let mut edges: Vec<BookmarkEdge> = page.into_iter().take(cap).collect();
    if truncated {
        let next = encode_cursor(&cursor_binding, offset + cap)?;
        if let Some(last) = edges.last_mut() {
            last.cursor = Some(next);
        }
    }

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
        let source_offset = text(&node, "source_offset").parse::<u64>().unwrap_or(0);
        if newest
            .as_ref()
            .map_or(true, |(best, ..)| source_offset >= *best)
        {
            newest = Some((
                source_offset,
                text(&node, "ingestion_receipt_id"),
                text(&node, "ingestion_receipt_digest"),
                text(&node, "source_revision"),
            ));
        }
    }
    let (_, ingestion_receipt_id, ingestion_receipt_digest, source_revision) =
        newest.unwrap_or_default();
    let result = BookmarkRelationsResult {
        edges,
        binding: BookmarkBinding {
            source_ids: sources.into_iter().collect(),
            ingestion_receipt_id,
            ingestion_receipt_digest,
            source_revision,
            index_revision,
            query_digest,
            graph_snapshot_revision,
            authorization_revision,
            limit_policy_digest,
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
pub(crate) async fn handle_bookmark_relations(
    ctx: &QueryHandlerCtx<'_>,
    request: BookmarkRelationsRequest,
) -> Response {
    #[cfg(feature = "security")]
    let authorization_revision = ctx.rls.policy_revision();
    #[cfg(not(feature = "security"))]
    let authorization_revision = 0;
    let Some(tenant) = ctx
        .read_authority
        .and_then(medulla_server_api::carrier::GraphReadAuthority::carrier)
        .map(|carrier| carrier.tenant_scope().to_string())
    else {
        return Response::err(
            ctx.req_id,
            "ACCESS_DENIED: bookmark relations require a verified tenant",
        );
    };
    let persist_dir = ctx.state.read().await.persist_dir.clone();
    let Some(persist_dir) = persist_dir.map(std::path::PathBuf::from) else {
        return Response::err(
            ctx.req_id,
            "ENGINE_UNAVAILABLE: semantic index storage is unavailable",
        );
    };
    // Read once at the start of the call: the revision every part of the answer reports.
    let index_revision = match bookmark_index_revision(&persist_dir, &tenant) {
        Ok(revision) => revision,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    match relations(ctx, &request, &tenant, index_revision, authorization_revision).await {
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
