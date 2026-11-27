//! Bookmark semantic search (MEDULLA-MEDIA-VOCAB-R001.102.2).
//!
//! The search ranks over the `bookmark` semantic index with the one live-generation probe of
//! [`crate::semantic_index::search`], filters hits by the caller's row visibility before counting,
//! and returns the [`BookmarkBinding`] of the index revision and graph snapshot it read. The
//! router handler reads the caller's projected graph; this module never does.

use std::collections::{BTreeMap, BTreeSet};

use medulla_ann_codes::semantic_index_service::SemanticIndexService;
use medulla_types::bookmark_query::{
    BookmarkBinding, BookmarkHit, BookmarkSearchResult, BookmarkSemanticSearchRequest,
    BOOKMARK_INDEX_STALE, BOOKMARK_LIMIT_EXCEEDED, BOOKMARK_RESPONSE_TOO_LARGE,
};
use medulla_types::contract::Digest256;

use crate::semantic_index::search::probe_vector;
use crate::semantic_index_runtime::semantic_cursor_secret;

/// The binding id of the bookmark semantic index.
pub const BOOKMARK_INDEX_BINDING_ID: &str = "bookmark";

/// The metadata keys a bookmark node may carry; a hit reports the absent ones as unknown.
const METADATA_KEYS: [&str; 5] = ["hashtags", "mentions", "tools", "urls", "domains"];

/// The most candidates one probe asks the index for, so a page never scans the whole index.
const MAX_CANDIDATES: usize = 4096;

/// What the router handler read of the caller's projected graph, once, at the start of the call.
pub struct BookmarkReadView<'a> {
    /// The revision of the graph snapshot the call reads.
    pub graph_snapshot_revision: u64,
    /// The authorization revision the call was admitted under.
    pub authorization_revision: u64,
    /// The verified tenant scope the cursor is bound to.
    pub tenant: &'a str,
    /// The node id a hit's source entity id names. The index is graph sourced, so its members
    /// are `semantic-graph-source:<digest>` entity ids, deterministic from the node id; the
    /// router computes this map over the caller's `Bookmark` nodes.
    pub node_of_entity: &'a dyn Fn(&str) -> Option<String>,
    /// Whether the caller may see a node.
    pub visible: &'a dyn Fn(&str) -> bool,
    /// A node's string properties, when it exists.
    pub properties: &'a dyn Fn(&str) -> Option<BTreeMap<String, String>>,
}

/// The graph text property the tenant's `bookmark` index is built from. An index with no
/// binding, or one bound to anything but a graph property, is `BOOKMARK_INDEX_STALE`.
pub fn bookmark_graph_selector(
    service: &SemanticIndexService,
) -> Result<medulla_index_types::semantic_index::GraphTextPropertyRef, String> {
    use medulla_index_types::semantic_index::SemanticSourceSelector;
    let stale = |reason: &str| refusal(BOOKMARK_INDEX_STALE, reason);
    let binding = service
        .binding()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| stale("the bookmark index has no binding"))?;
    match binding.source_selector {
        SemanticSourceSelector::GraphTextPropertyRef(selector) => Ok(selector),
        _ => Err(stale(
            "the bookmark index is not built from a graph property",
        )),
    }
}

/// The entity-id to node-id map of the given node ids, the `node_of_entity` of a view.
pub fn entity_node_map(
    selector: &medulla_index_types::semantic_index::GraphTextPropertyRef,
    tenant_scope: &str,
    node_ids: impl IntoIterator<Item = String>,
) -> BTreeMap<String, String> {
    node_ids
        .into_iter()
        .map(|node_id| {
            (
                crate::semantic_index_runtime::graph_source::graph_source_entity_id(
                    selector,
                    tenant_scope,
                    &node_id,
                ),
                node_id,
            )
        })
        .collect()
}

/// The refusal text `<CODE>: <reason>`.
pub fn refusal(code: medulla_types::refusal_code::RefusalCode, reason: &str) -> String {
    format!("{}: {reason}", code.as_str())
}

/// The SHA-256 of a value's named MessagePack form.
pub fn digest_of<T: serde::Serialize>(value: &T) -> Result<Digest256, String> {
    rmp_serde::to_vec_named(value)
        .map(|bytes| Digest256::sha256(&bytes))
        .map_err(|error| format!("bookmark digest encoding failed: {error}"))
}

/// The cursor domain of a bookmark search.
pub const SEARCH_CURSOR_DOMAIN: &[u8] = b"eg/bookmark-cursor/v1";
/// The cursor domain of a bookmark relations read; a search cursor never opens a relations page.
pub const RELATIONS_CURSOR_DOMAIN: &[u8] = b"eg/bookmark-relations-cursor/v1";

/// What a cursor MAC binds besides its offset: the tenant, the query, the index revision and the
/// graph snapshot revision of the call.
pub struct CursorBinding<'a> {
    /// The cursor domain, search or relations.
    pub domain: &'static [u8],
    /// The verified tenant scope.
    pub tenant: &'a str,
    /// The digest of the query the cursor pages.
    pub query_digest: &'a Digest256,
    /// The index revision the page was read at.
    pub index_revision: u64,
    /// The graph snapshot revision of the call.
    pub graph_snapshot_revision: u64,
}

fn cursor_mac(binding: &CursorBinding<'_>, offset: usize) -> Result<String, String> {
    let secret = semantic_cursor_secret();
    Ok(Digest256::framed(
        binding.domain,
        &[
            &secret[..],
            binding.tenant.as_bytes(),
            &binding.query_digest.as_bytes()[..],
            &binding.index_revision.to_be_bytes()[..],
            &binding.graph_snapshot_revision.to_be_bytes()[..],
            &(offset as u64).to_be_bytes()[..],
        ],
    )?
    .to_hex())
}

/// The MAC-bound cursor of the page that starts at `offset`.
pub fn encode_cursor(binding: &CursorBinding<'_>, offset: usize) -> Result<String, String> {
    Ok(format!("{offset}.{}", cursor_mac(binding, offset)?))
}

/// The page offset of a cursor whose MAC binds this call's tenant, query, index revision and
/// snapshot revision; any other cursor is `BOOKMARK_INDEX_STALE`.
pub fn decode_cursor(binding: &CursorBinding<'_>, cursor: &str) -> Result<usize, String> {
    let stale = || {
        refusal(
            BOOKMARK_INDEX_STALE,
            "the cursor names another index or snapshot",
        )
    };
    let (offset, mac) = cursor.split_once('.').ok_or_else(stale)?;
    let offset: usize = offset.parse().map_err(|_| stale())?;
    if cursor_mac(binding, offset)? != mac {
        return Err(stale());
    }
    Ok(offset)
}

/// The live generation of the tenant's `bookmark` semantic index, the `index_revision` of every
/// bookmark answer. No live generation is `BOOKMARK_INDEX_STALE`, never an empty success.
pub fn bookmark_index_revision(persist_dir: &std::path::Path, tenant: &str) -> Result<u64, String> {
    let service = crate::semantic_index_runtime::open_semantic_service(
        persist_dir,
        tenant,
        BOOKMARK_INDEX_BINDING_ID,
    )
    .map_err(|_| refusal(BOOKMARK_INDEX_STALE, "the bookmark index is unavailable"))?;
    service
        .live_generation()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            refusal(
                BOOKMARK_INDEX_STALE,
                "the bookmark index has no live generation",
            )
        })
}

/// The five metadata keys the node does not carry: absent, `[]`, or (for `domains`) a pair whose
/// label is null.
fn unknown_keys(properties: &BTreeMap<String, String>) -> Vec<String> {
    let mut unknown: Vec<String> = METADATA_KEYS
        .iter()
        .filter(|key| match properties.get(**key).map(String::as_str) {
            None | Some("[]") => true,
            Some(text) if **key == "domains" => {
                serde_json::from_str::<Vec<(String, Option<String>)>>(text)
                    .map_or(true, |pairs| pairs.iter().any(|(_, label)| label.is_none()))
            }
            Some(_) => false,
        })
        .map(|key| (*key).to_string())
        .collect();
    unknown.sort();
    unknown
}

fn provenance(properties: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    METADATA_KEYS
        .iter()
        .copied()
        .chain(["file_sha256", "source_revision"])
        .filter_map(|key| {
            properties
                .get(key)
                .map(|value| (key.to_string(), value.clone()))
        })
        .collect()
}

/// Search the bookmark index. Errors are `CODE: reason` refusals.
pub fn search(
    service: &SemanticIndexService,
    request: &BookmarkSemanticSearchRequest,
    view: &BookmarkReadView<'_>,
) -> Result<BookmarkSearchResult, String> {
    request
        .validate()
        .map_err(|code| refusal(code, "the request is invalid"))?;
    // Read once at the start of the call: every later read uses this one revision.
    let index_revision = service
        .live_generation()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            refusal(
                BOOKMARK_INDEX_STALE,
                "the bookmark index has no live generation",
            )
        })?;
    let mut without_cursor = request.clone();
    without_cursor.cursor = None;
    let query_digest = digest_of(&without_cursor)?;
    let limit_policy_digest = digest_of(&request.limit_policy)?;
    let cursor_binding = CursorBinding {
        domain: SEARCH_CURSOR_DOMAIN,
        tenant: view.tenant,
        query_digest: &query_digest,
        index_revision,
        graph_snapshot_revision: view.graph_snapshot_revision,
    };
    let offset = match &request.cursor {
        Some(cursor) => decode_cursor(&cursor_binding, cursor)?,
        None => 0,
    };
    let binding = service
        .binding()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| refusal(BOOKMARK_INDEX_STALE, "the bookmark index has no binding"))?;
    let max_hits = request.limit_policy.max_hits as usize;
    let wanted = offset.saturating_add(max_hits).saturating_add(1);
    let candidates = wanted.saturating_mul(4).min(MAX_CANDIDATES);
    if wanted > MAX_CANDIDATES {
        return Err(refusal(
            BOOKMARK_LIMIT_EXCEEDED,
            "the page lies beyond the candidate cap",
        ));
    }
    let probe = probe_vector(service, &binding, &request.query_embedding, candidates)?;
    if probe.generation != index_revision {
        return Err(refusal(
            BOOKMARK_INDEX_STALE,
            "the live generation changed during the call",
        ));
    }
    let sources: BTreeSet<&str> = request.source_ids.iter().map(String::as_str).collect();
    // Visibility filters before anything is counted.
    let mut visible: Vec<(String, f32, BTreeMap<String, String>)> = Vec::new();
    for (entity, distance) in probe.hits {
        let Some(id) = (view.node_of_entity)(&entity) else {
            continue;
        };
        if !(view.visible)(&id) {
            continue;
        }
        let Some(properties) = (view.properties)(&id) else {
            continue;
        };
        let source = properties
            .get("source_id")
            .map(String::as_str)
            .unwrap_or("");
        if !sources.is_empty() && !sources.contains(source) {
            continue;
        }
        visible.push((id, 1.0 - distance, properties));
    }
    let page: Vec<_> = visible.iter().skip(offset).take(max_hits + 1).collect();
    let truncated = page.len() > max_hits;
    let page = &page[..page.len().min(max_hits)];
    let mut hits: Vec<BookmarkHit> = page
        .iter()
        .map(|(id, score, properties)| BookmarkHit {
            id: id.clone(),
            score: *score,
            source_id: properties.get("source_id").cloned().unwrap_or_default(),
            provenance: provenance(properties),
            unknown_keys: unknown_keys(properties),
            cursor: None,
        })
        .collect();
    if truncated {
        let next = encode_cursor(&cursor_binding, offset + max_hits)?;
        if let Some(last) = hits.last_mut() {
            last.cursor = Some(next);
        }
    }
    let mut source_ids: Vec<String> = hits
        .iter()
        .map(|hit| hit.source_id.clone())
        .filter(|id| !id.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if hits.is_empty() {
        source_ids = request.source_ids.clone();
        source_ids.sort();
    }
    // A hit set with differing receipts reports the receipt of the newest node by `source_offset`.
    let newest = page
        .iter()
        .max_by_key(|(_, _, properties)| {
            properties
                .get("source_offset")
                .and_then(|offset| offset.parse::<u64>().ok())
                .unwrap_or(0)
        })
        .map(|(_, _, properties)| properties);
    let read = |key: &str| {
        newest
            .and_then(|properties| properties.get(key))
            .cloned()
            .unwrap_or_default()
    };
    let result = BookmarkSearchResult {
        hits,
        binding: BookmarkBinding {
            source_ids,
            ingestion_receipt_id: read("ingestion_receipt_id"),
            ingestion_receipt_digest: read("ingestion_receipt_digest"),
            source_revision: read("source_revision"),
            index_revision,
            query_digest,
            graph_snapshot_revision: view.graph_snapshot_revision,
            authorization_revision: view.authorization_revision,
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
