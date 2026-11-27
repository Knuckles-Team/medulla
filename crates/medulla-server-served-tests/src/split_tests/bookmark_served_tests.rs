// The served bookmark proofs (MEDULLA-MEDIA-VOCAB-R001.102.2 to R001.102.4): bookmark envelopes are
// ingested through `SourceIngest` into a graph whose pack declares `Bookmark`, the pack import
// authors the tenant's `bookmark` semantic binding, S1 is completed for the graph-sourced
// binding, the live-generation helper drives S2 to S6, and search and relations are then called
// as a `kg:read` reader. The query embedding of a search is the helper's deterministic vector
// of the entity it should rank first.

use std::collections::BTreeSet;
use std::sync::Arc;

use medulla_exec::exec::HashEmbedder;
use medulla_index_types::semantic_index::SemanticSourceSelector;
use medulla_protocol_types::protocol::{Method, Response};
use medulla_server_api::knob_config;
use medulla_server_api::state::TextEmbedderOverride;
use medulla_server_ops::api::query_txn::query_support::{HASH_ENCODER_ID, TEXT_ENCODER_REVISION};
use medulla_types::bookmark_query::{
    BookmarkEdge, BookmarkQueryLimitPolicy, BookmarkRelationsRequest, BookmarkRelationsResult,
    BookmarkSearchResult, BookmarkSemanticSearchRequest,
};
use serde_json::{json, Value};

use super::connector_pack_harness::{ok, Served, OTHER_TENANT, TENANT};
use super::semantic_live_generation::{
    binding_of, complete_graph_source_s1, drive_generation_to_live, entity_vector,
};

const CONNECTOR: &str = "bookmark-source";
const STREAM: &str = "bookmarks";
const BINDING: &str = "bookmark";
/// The lowered read window of the snapshot journey, in edges (knob `bookmark.relations_window`).
const WINDOW: u32 = 8;

/// The policy of a plain call: room for every hit, edge and byte the journeys produce.
fn policy(max_hits: u32, max_relationships: u32) -> BookmarkQueryLimitPolicy {
    BookmarkQueryLimitPolicy {
        max_hits,
        max_relationships,
        max_response_bytes: 1 << 20,
    }
}

/// One bookmark envelope: the keys of the connector's bookmark payload.
fn envelope(
    key: &str,
    offset: u64,
    text: &str,
    hashtags: &[&str],
    mentions: &[&str],
    tools: &[&str],
    urls: &[&str],
    domains: Value,
) -> (String, Value) {
    (
        format!("record-{key}"),
        json!({
            "bookmark_id": format!("bm-{key}"),
            "record_key": key,
            "text": text,
            "source_offset": offset,
            "file_sha256": format!("{:0>64}", format!("{key}f11e")),
            "export_revision": "export-1",
            "vocabulary_revision": "vocabulary-1",
            "vocabulary_sha256": "0".repeat(64),
            "hashtags": hashtags,
            "mentions": mentions,
            "tools": tools,
            "urls": urls,
            "domains": domains,
        }),
    )
}

/// Two bookmarks of different metadata: the first names no account and the second carries a
/// domain with no label and no tool or url.
fn two_bookmarks() -> Vec<(String, Value)> {
    vec![
        envelope(
            "alpha",
            1,
            "a note on rust ownership",
            &["rust"],
            &[],
            &["cargo"],
            &["https://a.example/x"],
            json!([["a.example", "A Site"]]),
        ),
        envelope(
            "beta",
            2,
            "a talk on wasm components",
            &["rust", "wasm"],
            &["bob"],
            &[],
            &[],
            json!([["b.example", null]]),
        ),
    ]
}

/// A served state with the bookmark index live over `bookmarks`.
struct Journey {
    served: Served,
    live_generation: u64,
    dimension: usize,
    /// `(node id, source entity id)` in the order of `bookmarks`.
    nodes: Vec<(String, String)>,
    ingest_receipt_id: String,
}

fn node_id_of(record: &(String, Value)) -> String {
    format!("bookmark:{}", record.1["record_key"].as_str().unwrap())
}

impl Journey {
    /// Ingest `bookmarks` into `graph`, complete S1 for the graph-sourced binding the pack
    /// import authored and drive the generation to live.
    async fn live(graph: &str, bookmarks: &[(String, Value)]) -> Self {
        // The text embedder is injected into this state alone; no process variable is set.
        let embedder = TextEmbedderOverride::new(
            Arc::new(HashEmbedder::default()),
            HASH_ENCODER_ID,
            HASH_ENCODER_ID,
            TEXT_ENCODER_REVISION,
        )
        .expect("the hash embedder embeds text");
        let served = Served::provisioned_activating(
            graph,
            CONNECTOR,
            &[(STREAM, "Bookmark")],
            graph,
            Some(embedder),
        )
        .await;
        served.subscribe_source_dirty().await;
        let binding = binding_of(&served, BINDING).await;
        let receipt = served.ingest(STREAM, &as_records(bookmarks)).await;
        let dirty = served.source_dirty_records().await;
        assert_eq!(dirty.len(), 1, "one commit wakes the bound source once");
        complete_graph_source_s1(&served, BINDING, &dirty[0]).await;
        let live_generation = drive_generation_to_live(&served, BINDING).await;
        let SemanticSourceSelector::GraphTextPropertyRef(selector) = &binding.source_selector
        else {
            panic!("the bookmark binding is built from a graph property");
        };
        assert_eq!(selector.graph_id, graph);
        let node_ids: Vec<String> = bookmarks.iter().map(node_id_of).collect();
        let entities =
            medulla_h_retrieve::bookmark::entity_node_map(selector, TENANT, node_ids.clone());
        let nodes = node_ids
            .into_iter()
            .map(|node| {
                let entity = entities
                    .iter()
                    .find(|(_, candidate)| **candidate == node)
                    .map(|(entity, _)| entity.clone())
                    .expect("every bookmark node has a source entity");
                (node, entity)
            })
            .collect();
        Self {
            served,
            live_generation,
            dimension: binding.dimension as usize,
            nodes,
            ingest_receipt_id: receipt.receipt_id,
        }
    }

    /// The query embedding that ranks bookmark `at` first.
    fn query_for(&self, at: usize) -> Vec<f32> {
        entity_vector(self.dimension, &self.nodes[at].1)
    }

    async fn search_as(
        &self,
        tenant: &str,
        scopes: &[&str],
        request: BookmarkSemanticSearchRequest,
    ) -> Response {
        self.served
            .verified(
                tenant,
                scopes,
                Method::BookmarkSemanticSearch {
                    request: Box::new(request),
                },
            )
            .await
    }

    async fn search(&self, request: BookmarkSemanticSearchRequest) -> BookmarkSearchResult {
        ok(
            "BookmarkSemanticSearch",
            self.search_as(TENANT, &["kg:read"], request).await,
        )
    }

    async fn relations_as(
        &self,
        tenant: &str,
        scopes: &[&str],
        request: BookmarkRelationsRequest,
    ) -> Response {
        self.served
            .verified(
                tenant,
                scopes,
                Method::BookmarkRelations {
                    request: Box::new(request),
                },
            )
            .await
    }

    async fn relations_for(
        &self,
        principal: &str,
        request: BookmarkRelationsRequest,
    ) -> BookmarkRelationsResult {
        ok(
            "BookmarkRelations as a registered reader",
            self.served
                .verified_as(
                    principal,
                    TENANT,
                    &["kg:read"],
                    Method::BookmarkRelations {
                        request: Box::new(request),
                    },
                )
                .await,
        )
    }

    async fn relations(&self, request: BookmarkRelationsRequest) -> BookmarkRelationsResult {
        ok(
            "BookmarkRelations",
            self.relations_as(TENANT, &["kg:read"], request).await,
        )
    }
}

fn as_records(bookmarks: &[(String, Value)]) -> Vec<(&str, Value)> {
    bookmarks
        .iter()
        .map(|(record_id, payload)| (record_id.as_str(), payload.clone()))
        .collect()
}

fn search_request(
    query_embedding: Vec<f32>,
    max_hits: u32,
    cursor: Option<String>,
) -> BookmarkSemanticSearchRequest {
    BookmarkSemanticSearchRequest {
        query_embedding,
        source_ids: Vec::new(),
        limit_policy: policy(max_hits, 100),
        cursor,
    }
}

fn relations_request(
    node_ids: Vec<String>,
    max_relationships: u32,
    cursor: Option<String>,
) -> BookmarkRelationsRequest {
    BookmarkRelationsRequest {
        node_ids,
        limit_policy: policy(10, max_relationships),
        cursor,
    }
}

/// The refusal a response carries, `CODE: reason`.
fn refusal_of(label: &str, response: Response) -> String {
    assert!(
        response.result.is_none(),
        "{label} must not answer: {response:?}"
    );
    response
        .error_detail
        .unwrap_or_else(|| panic!("{label} was accepted but must be refused"))
}

/// `(source, target, relationship)` of each edge, sorted.
fn edge_set(edges: &[BookmarkEdge]) -> BTreeSet<(String, String, String)> {
    edges
        .iter()
        .map(|edge| {
            (
                edge.source_node_id.clone(),
                edge.target_node_id.clone(),
                edge.relationship.clone(),
            )
        })
        .collect()
}

/// The edges ingest writes for `two_bookmarks`.
fn expected_edges() -> BTreeSet<(String, String, String)> {
    [
        ("bookmark:alpha", "hashtag:rust", "taggedWith"),
        ("bookmark:alpha", "tool:cargo", "usesTool"),
        ("bookmark:alpha", "domain:a.example", "linksToDomain"),
        ("bookmark:beta", "hashtag:rust", "taggedWith"),
        ("bookmark:beta", "hashtag:wasm", "taggedWith"),
        ("bookmark:beta", "account:bob", "mentionsAccount"),
        ("bookmark:beta", "domain:b.example", "linksToDomain"),
    ]
    .into_iter()
    .map(|(source, target, relationship)| {
        (
            source.to_string(),
            target.to_string(),
            relationship.to_string(),
        )
    })
    .collect()
}

// spec: MEDULLA-MEDIA-VOCAB-R001.102.2
#[tokio::test]
async fn search_pages_with_bound_cursor_and_unknown_metadata() {
    let bookmarks = two_bookmarks();
    let journey = Journey::live("bookmark-proof-search", &bookmarks).await;
    assert!(journey.live_generation >= 1);

    // Page one holds the bookmark the query embedding names, and its last hit carries the cursor.
    let first = journey
        .search(search_request(journey.query_for(0), 1, None))
        .await;
    assert_eq!(first.hits.len(), 1);
    let alpha = &first.hits[0];
    assert_eq!(alpha.id, "bookmark:alpha");
    assert!(
        alpha.score > 0.99,
        "the query is the entity's own vector: {}",
        alpha.score
    );
    assert_eq!(alpha.unknown_keys, vec!["mentions".to_string()]);
    assert_eq!(alpha.provenance["hashtags"], r#"["rust"]"#);
    assert!(alpha.provenance.contains_key("file_sha256"));
    assert_eq!(
        alpha.provenance["source_revision"],
        alpha.provenance["file_sha256"]
    );
    assert!(
        !alpha.provenance.contains_key("mentions"),
        "no key is defaulted"
    );
    let cursor = alpha
        .cursor
        .clone()
        .expect("a truncated page carries a cursor");
    assert_eq!(first.binding.index_revision, journey.live_generation);
    assert_eq!(first.binding.source_ids.len(), 1);
    assert!(!first.binding.ingestion_receipt_id.is_empty());

    // The cursor opens the second page of the same query.
    let second = journey
        .search(search_request(
            journey.query_for(0),
            1,
            Some(cursor.clone()),
        ))
        .await;
    assert_eq!(second.hits.len(), 1);
    let beta = &second.hits[0];
    assert_eq!(beta.id, "bookmark:beta");
    assert_eq!(
        beta.unknown_keys,
        vec![
            "domains".to_string(),
            "tools".to_string(),
            "urls".to_string()
        ],
        "an absent, empty or unlabelled key is unknown, sorted"
    );
    assert!(beta.cursor.is_none(), "the last page carries no cursor");
    assert_eq!(second.binding.query_digest, first.binding.query_digest);

    // A cursor of another query fails its MAC.
    let other_query = journey
        .search_as(
            TENANT,
            &["kg:read"],
            search_request(journey.query_for(1), 1, Some(cursor.clone())),
        )
        .await;
    assert!(refusal_of("a cursor of another query", other_query).contains("BOOKMARK_INDEX_STALE"));

    // A commit changes the graph snapshot the cursor was bound to.
    let late = envelope(
        "gamma",
        3,
        "a post on sqlite internals",
        &["sqlite"],
        &[],
        &[],
        &[],
        json!([]),
    );
    journey.served.ingest(STREAM, &as_records(&[late])).await;
    let changed = journey
        .search_as(
            TENANT,
            &["kg:read"],
            search_request(journey.query_for(0), 1, Some(cursor)),
        )
        .await;
    assert!(refusal_of("a cursor of an older snapshot", changed).contains("BOOKMARK_INDEX_STALE"));
}

// spec: MEDULLA-MEDIA-VOCAB-R001.102.3
#[tokio::test]
async fn relations_hide_denied_edges_and_counts() {
    let bookmarks = two_bookmarks();
    let journey = Journey::live("bookmark-proof-relations", &bookmarks).await;
    let node_ids: Vec<String> = journey.nodes.iter().map(|(node, _)| node.clone()).collect();

    // The reader sees every edge leaving the two bookmarks, with no mindmap provenance.
    let all = journey
        .relations(relations_request(node_ids.clone(), 100, None))
        .await;
    assert_eq!(edge_set(&all.edges), expected_edges());
    assert_eq!(
        all.edges.len(),
        expected_edges().len(),
        "no edge is repeated"
    );
    assert!(all
        .edges
        .iter()
        .all(|edge| edge.provenance.is_none() && edge.cursor.is_none()));
    assert_eq!(all.binding.index_revision, journey.live_generation);

    // A page of three holds exactly three edges, and the cursors walk the same set.
    let mut paged = Vec::new();
    let mut cursor = None;
    loop {
        let page = journey
            .relations(relations_request(node_ids.clone(), 3, cursor.take()))
            .await;
        assert!(page.edges.len() <= 3);
        cursor = page.edges.last().and_then(|edge| edge.cursor.clone());
        paged.extend(page.edges);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(paged.len(), expected_edges().len());
    assert_eq!(edge_set(&paged), expected_edges());

    // A caller without `kg:read` gets a refusal, never an empty or partial edge list.
    let denied = journey
        .relations_as(TENANT, &[], relations_request(node_ids.clone(), 100, None))
        .await;
    assert!(
        denied.error.is_some() && denied.result.is_none(),
        "{denied:?}"
    );

    // Another tenant reads none of this tenant's bookmarks.
    let foreign = journey
        .relations_as(
            OTHER_TENANT,
            &["kg:read"],
            relations_request(node_ids.clone(), 100, None),
        )
        .await;
    assert!(
        foreign.error.is_some() && foreign.result.is_none(),
        "{foreign:?}"
    );

    // Equal scope, equal result: the administrator and the reader see the same edges.
    let as_admin: BookmarkRelationsResult = ok(
        "BookmarkRelations as the administrator",
        journey
            .served
            .call_on(
                journey.served.graph(),
                "bookmark-relations-admin",
                Method::BookmarkRelations {
                    request: Box::new(relations_request(node_ids.clone(), 100, None)),
                },
            )
            .await,
    );
    assert_eq!(as_admin.edges, all.edges);
    assert_eq!(as_admin.binding, all.binding);

    // A row-policy-denied edge: a registered reader sees every edge while the rows are public,
    // then none that leaves toward or from a row it may not read, and no count includes one.
    const READER: &str = "bookmark-reader";
    const DENIED: &str = "domain:b.example";
    journey.served.register_reader(READER).await;
    let before = journey
        .relations_for(READER, relations_request(node_ids.clone(), 100, None))
        .await;
    assert_eq!(edge_set(&before.edges), expected_edges());
    journey.served.deny_row(DENIED, "bookmark-owner").await;
    let mut visible = expected_edges();
    assert!(visible.remove(&(
        "bookmark:beta".to_string(),
        DENIED.to_string(),
        "linksToDomain".to_string(),
    )));
    let after = journey
        .relations_for(READER, relations_request(node_ids.clone(), 100, None))
        .await;
    assert_eq!(edge_set(&after.edges), visible);
    assert_eq!(
        after.edges.len(),
        visible.len(),
        "a denied edge is not counted"
    );
    // Pages of two hold the same visible edges, with no gap where the denied edge was.
    let mut paged = Vec::new();
    let mut cursor = None;
    loop {
        let page = journey
            .relations_for(
                READER,
                relations_request(node_ids.clone(), 2, cursor.take()),
            )
            .await;
        cursor = page.edges.last().and_then(|edge| edge.cursor.clone());
        paged.extend(page.edges);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(paged.len(), visible.len());
    assert_eq!(edge_set(&paged), visible);
    // The System administrator is bound by no row rule and still reads every edge.
    let admin_after = journey
        .relations(relations_request(node_ids.clone(), 100, None))
        .await;
    assert_eq!(edge_set(&admin_after.edges), expected_edges());
}

// spec: MEDULLA-MEDIA-VOCAB-R001.102.4
#[tokio::test]
async fn ingest_search_and_relations_share_one_snapshot() {
    // The read window is a knob read once per process, so it is lowered before the first call;
    // this test runs in its own process. Eight edges fit a window; the journey's reads are
    // pages of at most eight.
    let loaded =
        knob_config::load_knobs(&[format!("bookmark.relations_window={WINDOW}")], |_| None)
            .expect("the window knob loads");
    knob_config::install(loaded);
    assert_eq!(
        knob_config::knobs().bookmark_relations_window,
        WINDOW as u64
    );

    let bookmarks = two_bookmarks();
    let journey = Journey::live("bookmark-proof-snapshot", &bookmarks).await;

    let found = journey
        .search(search_request(journey.query_for(1), 2, None))
        .await;
    assert_eq!(found.hits.len(), 2);
    assert_eq!(found.hits[0].id, "bookmark:beta");
    let hit_ids: Vec<String> = found.hits.iter().map(|hit| hit.id.clone()).collect();
    let related = journey
        .relations(relations_request(hit_ids.clone(), WINDOW, None))
        .await;
    assert_eq!(edge_set(&related.edges), expected_edges());

    // Both answers read one index revision, one graph snapshot and one authorization revision,
    // and name the source receipt of the ingested nodes.
    let (search, relations) = (&found.binding, &related.binding);
    assert_eq!(search.index_revision, journey.live_generation);
    assert_eq!(search.index_revision, relations.index_revision);
    assert_eq!(
        search.graph_snapshot_revision,
        relations.graph_snapshot_revision
    );
    assert_eq!(
        search.authorization_revision,
        relations.authorization_revision
    );
    assert_eq!(search.source_ids, relations.source_ids);
    assert_eq!(search.ingestion_receipt_id, relations.ingestion_receipt_id);
    assert_eq!(
        search.ingestion_receipt_digest,
        relations.ingestion_receipt_digest
    );
    assert_eq!(search.source_revision, relations.source_revision);
    assert!(!search.ingestion_receipt_id.is_empty());
    assert!(!journey.ingest_receipt_id.is_empty());
    assert!(!search.source_ids.is_empty());
    assert_eq!(
        search.source_revision, found.hits[0].provenance["file_sha256"],
        "the receipt of the newest node by source offset is reported"
    );

    // Re-ingesting the same records writes the same nodes and edges: nothing is duplicated.
    journey.served.ingest(STREAM, &as_records(&bookmarks)).await;
    let again = journey
        .relations(relations_request(hit_ids.clone(), WINDOW, None))
        .await;
    assert_eq!(again.edges.len(), expected_edges().len());
    assert_eq!(edge_set(&again.edges), expected_edges());
    assert!(
        again.binding.graph_snapshot_revision > relations.graph_snapshot_revision,
        "the re-ingest is a new snapshot"
    );

    // Refusals: each is a named code, never an empty or truncated success.
    let mut invalid = search_request(journey.query_for(0), 1, None);
    invalid.limit_policy.max_hits = 0;
    let refused = refusal_of(
        "a zero limit",
        journey.search_as(TENANT, &["kg:read"], invalid).await,
    );
    assert!(refused.contains("BOOKMARK_POLICY_INVALID"), "{refused}");

    let mut small = search_request(journey.query_for(0), 2, None);
    small.limit_policy.max_response_bytes = 1;
    let refused = refusal_of(
        "a response above its byte limit",
        journey.search_as(TENANT, &["kg:read"], small).await,
    );
    assert!(refused.contains("BOOKMARK_RESPONSE_TOO_LARGE"), "{refused}");

    let mut oversized = relations_request(hit_ids, WINDOW, None);
    oversized.limit_policy.max_hits = 1;
    let refused = refusal_of(
        "more node ids than max_hits",
        journey.relations_as(TENANT, &["kg:read"], oversized).await,
    );
    assert!(refused.contains("BOOKMARK_LIMIT_EXCEEDED"), "{refused}");

    let first = journey
        .search(search_request(journey.query_for(0), 1, None))
        .await;
    let stale_cursor = first.hits[0].cursor.clone().expect("a truncated page");
    journey
        .served
        .ingest(
            STREAM,
            &as_records(&[envelope(
                "delta",
                4,
                "a thread on vector indexes",
                &["ann"],
                &[],
                &[],
                &[],
                json!([]),
            )]),
        )
        .await;
    let refused = refusal_of(
        "a cursor of an older snapshot",
        journey
            .search_as(
                TENANT,
                &["kg:read"],
                search_request(journey.query_for(0), 1, Some(stale_cursor)),
            )
            .await,
    );
    assert!(refused.contains("BOOKMARK_INDEX_STALE"), "{refused}");

    // Another tenant has no bookmark index: it is refused, and sees none of the hits.
    let foreign = journey
        .search_as(
            OTHER_TENANT,
            &["kg:read"],
            search_request(journey.query_for(0), 2, None),
        )
        .await;
    assert!(
        foreign.error.is_some() && foreign.result.is_none(),
        "{foreign:?}"
    );

    // The read window: a page that lies beyond it refuses, and so does an answer above it,
    // never a truncated success. Two more bookmarks bring the edges to eleven, over the eight.
    let over_page = journey
        .relations_as(
            TENANT,
            &["kg:read"],
            relations_request(vec!["bookmark:alpha".to_string()], WINDOW + 1, None),
        )
        .await;
    let refused = refusal_of("a page beyond the window", over_page);
    assert!(refused.contains("BOOKMARK_LIMIT_EXCEEDED"), "{refused}");

    journey
        .served
        .ingest(
            STREAM,
            &as_records(&[envelope(
                "epsilon",
                5,
                "a note on three tags",
                &["one", "two", "three"],
                &[],
                &[],
                &[],
                json!([]),
            )]),
        )
        .await;
    let all_ids = ["alpha", "beta", "delta", "epsilon"]
        .map(|key| format!("bookmark:{key}"))
        .to_vec();
    let over_answer = relations_request(all_ids, WINDOW, None);
    let refused = refusal_of(
        "an answer above the window",
        journey
            .relations_as(TENANT, &["kg:read"], over_answer)
            .await,
    );
    assert!(refused.contains("BOOKMARK_LIMIT_EXCEEDED"), "{refused}");
}
