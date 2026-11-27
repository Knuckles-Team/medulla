//! MEDULLA-DECISION-ENGINE-R100.100.4: the served context search through the real
//! `dispatch` shell. It reads the `Document` nodes the caller may see, returns spans
//! whose offsets slice the original bytes, caps a page and a span by the policy
//! knobs, pages with an opaque cursor, and answers a denied document exactly as it
//! answers an absent one.
#![cfg(feature = "query")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_agent_types::agent_component::{
    AgentComponentKind, AgentComponentPublishRequest, ComponentDependency, DecisionPolicyRef,
};
use medulla_decision_types::decision::digest::policy_digest;
use medulla_decision_types::decision::knob_families::context_search::KNOBS;
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_decision_types::decision::policy::DECISION_POLICY_ATTRIBUTE;
use medulla_decision_types::decision::statistical::context_search::{
    sha256_hex, RetrievalContextSearchRequest, RetrievalContextSearchResult,
    CONTEXT_SEARCH_SCHEMA_VERSION,
};
use medulla_decision_types::decision::DecisionPolicy;
use medulla_persistence_records::agent_component::test_component_draft;
use medulla_persistence_records::agent_fixtures::mutation_context;
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_server_api::state::ServerState;

const SECRET: &str = "context-search-served-secret";
const GRAPH: &str = "__commons__";
const COMPOSED: &str = "un caf\u{e9} noir, un th\u{e9} vert";
const DECOMPOSED: &str = "le cafe\u{301} du matin, puis le cafe\u{301} du soir";
const MULTIBYTE: &str = "\u{65e5}\u{672c}\u{8a9e} na\u{ef}ve caf\u{e9} \u{65e5}\u{672c}\u{8a9e}";

fn state() -> Arc<RwLock<ServerState>> {
    let (persist_dir, persistence) = common::tempdir_persistence();
    test_support::state_with(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
    )
}

async fn add(state: &Arc<RwLock<ServerState>>, id: u64, node: &str, props: serde_json::Value) {
    let response = test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            id,
            Method::AddNode {
                node_id: node.to_string(),
                properties_msgpack: test_support::json_bytes(props),
            },
        ),
    )
    .await;
    assert!(
        response.error.is_none(),
        "seed failed: {:?}",
        response.error
    );
}

fn document(text: &str) -> serde_json::Value {
    serde_json::json!({ "type": "Document", "text": text })
}

/// The documents every case reads. `extra` is the node that differs between the
/// two worlds of the denied-equals-absent case (a hidden Document or a plain Note).
async fn seed(state: &Arc<RwLock<ServerState>>, extra: serde_json::Value) {
    add(state, 1, "doc-composed", document(COMPOSED)).await;
    add(state, 2, "doc-decomposed", document(DECOMPOSED)).await;
    add(state, 3, "doc-multibyte", document(MULTIBYTE)).await;
    add(
        state,
        4,
        "note-cafe",
        serde_json::json!({ "type": "Note", "text": "caf\u{e9} in a note" }),
    )
    .await;
    add(state, 5, "doc-extra", extra).await;
}

fn request(query: &str, ngram_n: u32, max_spans: u32) -> RetrievalContextSearchRequest {
    RetrievalContextSearchRequest {
        schema_version: CONTEXT_SEARCH_SCHEMA_VERSION,
        tenant_id: common::TEST_TENANT.to_string(),
        graph: GRAPH.to_string(),
        query: query.to_string(),
        ngram_n,
        max_spans,
        cursor: None,
        policy: DecisionPolicyRef::Default,
    }
}

async fn search(
    state: &Arc<RwLock<ServerState>>,
    request: RetrievalContextSearchRequest,
) -> Response {
    test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            900,
            Method::RetrievalContextSearch {
                request: Box::new(request),
            },
        ),
    )
    .await
}

fn decode(response: Response) -> RetrievalContextSearchResult {
    assert!(
        response.error.is_none(),
        "search failed: {:?}",
        response.error
    );
    match response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("typed result"),
        other => panic!("unexpected payload {other:?}"),
    }
}

fn refused(response: Response) -> String {
    response.error.expect("the search must refuse")
}

fn slice(text: &str, start: u64, end: u64) -> &str {
    text.get(start as usize..end as usize)
        .expect("a character range")
}

/// Publish `policy` as a `DecisionPolicy` component of the test tenant, the way the
/// assembly lane serves it, and return its pin.
async fn publish_policy(
    state: &Arc<RwLock<ServerState>>,
    id: &str,
    policy: &DecisionPolicy,
) -> ComponentDependency {
    let store = state.write().await.ensure_agent_library().expect("library");
    let mut draft = test_component_draft(common::TEST_TENANT, id);
    draft.kind = AgentComponentKind::DecisionPolicy;
    draft.content_digest = policy_digest(policy);
    draft.attributes.insert(
        DECISION_POLICY_ATTRIBUTE.to_string(),
        serde_json::to_string(policy).expect("policy json"),
    );
    let request = AgentComponentPublishRequest {
        context: mutation_context(
            &store,
            common::TEST_TENANT,
            &format!("publish-{id}"),
            1,
            0,
            "agent-component:publish",
        ),
        component: draft,
        evaluation_receipt_digest: None,
        body: None,
    };
    let committed = store
        .publish_component(request, None)
        .expect("publish policy");
    ComponentDependency {
        component_id: id.to_string(),
        kind: AgentComponentKind::DecisionPolicy,
        definition_digest: committed.result.component.definition_digest,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R100.100.4, MEDULLA-DECISION-ENGINE-R100.100.3
#[tokio::test(flavor = "multi_thread")]
async fn context_search_is_authorized_and_bounded() {
    let state = state();
    seed(&state, document("late caf\u{e9}")).await;
    let texts = [
        ("doc-composed", COMPOSED),
        ("doc-decomposed", DECOMPOSED),
        ("doc-multibyte", MULTIBYTE),
        ("doc-extra", "late caf\u{e9}"),
    ];

    // Composed, decomposed and multibyte text: every offset slices the original bytes,
    // every span names the digest of the stored text, a Note is not searched.
    let all = decode(search(&state, request("caf\u{e9}", 1, 0)).await);
    assert_eq!(all.next_cursor, None);
    assert_eq!(
        all.spans.len(),
        5,
        "1 + 2 + 1 + 1 matches over the Documents"
    );
    let mut order: Vec<(String, u64)> = all
        .spans
        .iter()
        .map(|s| (s.document_id.clone(), s.match_start))
        .collect();
    let sorted = {
        let mut sorted = order.clone();
        sorted.sort();
        sorted
    };
    assert_eq!(
        order, sorted,
        "spans come in (document id, match start) order"
    );
    order.dedup_by_key(|(id, _)| id.clone());
    assert_eq!(
        order.into_iter().map(|(id, _)| id).collect::<Vec<_>>(),
        [
            "doc-composed",
            "doc-decomposed",
            "doc-extra",
            "doc-multibyte"
        ]
    );
    for span in &all.spans {
        let (_, original) = texts
            .iter()
            .find(|(id, _)| *id == span.document_id)
            .expect("only Documents are searched");
        assert_eq!(span.text_digest, sha256_hex(&[original.as_bytes()]));
        assert!(matches!(
            slice(original, span.match_start, span.match_end),
            "caf\u{e9}" | "cafe\u{301}"
        ));
        assert_eq!(
            slice(original, span.context_start, span.context_end),
            span.context_text
        );
    }
    assert_eq!(all.index_digest.len(), 64);
    assert!(all.query_revision > 0 && all.authorization_revision > 0);

    // A cursor pages the same spans, one per call, and a stale one is refused.
    let first = decode(search(&state, request("caf\u{e9}", 1, 1)).await);
    assert_eq!(first.spans.len(), 1);
    let mut seen = first.spans.clone();
    let mut cursor = first.next_cursor.clone();
    while let Some(next) = cursor {
        let mut paged = request("caf\u{e9}", 1, 1);
        paged.cursor = Some(next);
        let page = decode(search(&state, paged).await);
        seen.extend(page.spans);
        cursor = page.next_cursor;
    }
    assert_eq!(seen, all.spans);
    let mut forged = request("caf\u{e9}", 1, 1);
    forged.cursor = first.next_cursor.clone().map(|c| format!("{c}0"));
    assert!(refused(search(&state, forged).await).starts_with("CONTEXT_CURSOR_INVALID"));
    let mut other_query = request("th\u{e9}", 1, 1);
    other_query.cursor = first.next_cursor.clone();
    assert!(refused(search(&state, other_query).await).starts_with("CONTEXT_CURSOR_INVALID"));
    add(&state, 901, "doc-after", document("after caf\u{e9}")).await;
    let mut stale = request("caf\u{e9}", 1, 1);
    stale.cursor = first.next_cursor.clone();
    assert!(refused(search(&state, stale).await).starts_with("CONTEXT_CURSOR_STALE"));

    // The page cap is the policy knob, whatever the request asks.
    let ticks = "tick ".repeat(120);
    add(&state, 902, "doc-ticks", document(&ticks)).await;
    let capped = decode(search(&state, request("tick", 1, 1000)).await);
    assert_eq!(
        capped.spans.len(),
        100,
        "context_search.max_spans defaults to 100"
    );
    let mut rest = request("tick", 1, 1000);
    rest.cursor = capped.next_cursor.clone();
    let tail = decode(search(&state, rest).await);
    assert_eq!(tail.spans.len(), 20);
    assert_eq!(tail.next_cursor, None);

    // A policy pinned from the Agent Library supplies the knob values the search runs under.
    let mut policy = DecisionPolicy::engine_default();
    policy
        .knobs
        .insert(KNOBS[2].knob_name(), KnobValue::Integer(2));
    let pin = publish_policy(&state, "policy-context-search", &policy).await;
    let mut pinned = request("tick", 1, 50);
    pinned.policy = DecisionPolicyRef::Pinned {
        component: pin.clone(),
    };
    let pinned_page = decode(search(&state, pinned).await);
    assert_eq!(
        pinned_page.spans.len(),
        2,
        "the pinned max_spans knob caps the page"
    );
    assert!(pinned_page.next_cursor.is_some());
    let mut wrong_pin = request("tick", 1, 50);
    wrong_pin.policy = DecisionPolicyRef::Pinned {
        component: ComponentDependency {
            definition_digest: format!("sha256:{}", "0".repeat(64)),
            ..pin
        },
    };
    assert!(refused(search(&state, wrong_pin).await).starts_with("COMPONENT_PIN_MISMATCH"));

    // A span longer than the span-byte knob refuses instead of returning a truncated span.
    let words: Vec<String> = "abcdefgh"
        .chars()
        .map(|c| c.to_string().repeat(1200))
        .collect();
    let long = words.join(" ");
    add(&state, 903, "doc-long", document(&long)).await;
    let too_large = refused(search(&state, request(&long, 8, 0)).await);
    assert!(
        too_large.starts_with("CONTEXT_SPAN_TOO_LARGE"),
        "{too_large}"
    );

    // Refusals before any read: empty query, another tenant, another graph, bad width.
    assert!(refused(search(&state, request("   ", 1, 0)).await).starts_with("CONTEXT_QUERY_EMPTY"));
    let mut foreign = request("caf\u{e9}", 1, 0);
    foreign.tenant_id = "tenant-other".to_string();
    assert!(refused(search(&state, foreign).await).starts_with("ACCESS_DENIED"));
    let mut elsewhere = request("caf\u{e9}", 1, 0);
    elsewhere.graph = "another-graph".to_string();
    assert!(refused(search(&state, elsewhere).await).starts_with("INVALID_ARGUMENT"));
    assert!(
        refused(search(&state, request("caf\u{e9}", 9, 0)).await).starts_with("INVALID_ARGUMENT")
    );
}

// spec: MEDULLA-DECISION-ENGINE-R100.100.4
#[tokio::test(flavor = "multi_thread")]
async fn a_denied_document_answers_as_an_absent_one() {
    // World A holds a Document that belongs to another tenant's graph; world B holds a
    // plain Note in its place, so both worlds took the same writes.
    let denied = serde_json::json!({
        "type": "Document",
        "text": "secret caf\u{e9} caf\u{e9} caf\u{e9}",
        "_tenant": "tenant-other",
    });
    let absent = serde_json::json!({ "type": "Note", "text": "secret caf\u{e9}" });
    let (a, b) = (state(), state());
    seed(&a, denied).await;
    seed(&b, absent).await;
    for (query, max_spans) in [("caf\u{e9}", 0), ("caf\u{e9}", 1), ("secret", 0)] {
        let in_a = decode(search(&a, request(query, 1, max_spans)).await);
        let in_b = decode(search(&b, request(query, 1, max_spans)).await);
        assert_eq!(
            in_a, in_b,
            "a denied document leaves no trace in the answer"
        );
        assert!(
            in_a.spans.iter().all(|s| s.document_id != "doc-extra"),
            "no span, count, cursor offset or index digest reflects it"
        );
    }
    assert!(decode(search(&a, request("secret", 1, 0)).await)
        .spans
        .is_empty());

    // A cursor minted in one world is invalid in the other only through its snapshot,
    // never through a count of hidden rows: the paged answers match page for page.
    let page_a = decode(search(&a, request("caf\u{e9}", 1, 1)).await);
    let page_b = decode(search(&b, request("caf\u{e9}", 1, 1)).await);
    assert_eq!(page_a.next_cursor, page_b.next_cursor);
}
