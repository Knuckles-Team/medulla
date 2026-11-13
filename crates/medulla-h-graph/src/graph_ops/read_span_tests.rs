//! Served `ReadSpan` proofs (MEDULLA-DECISION-ENGINE-R153.3, R153.5).

use super::read_span::{serve_read_span, SpanBlobs, SpanRequest};
use medulla_document::toc::{build_and_store_toc, StagedToc, TocSink};
use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_types::source_toc::{ReadSpanResult, SourceToc};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;

const SOURCE: &str = "preamble\n# Title\nintro\n\n## Part A\na1\na2\n## Part B\nb1\nb2\nlast line";

/// An in-memory content-addressed blob store standing in for the blob
/// substrate `SourceIngest` writes to.
#[derive(Default)]
struct MemoryBlobs(Mutex<HashMap<String, Vec<u8>>>);

impl SpanBlobs for MemoryBlobs {
    fn get(&self, digest: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.lock().unwrap().get(digest).cloned())
    }
}

struct Sink<'a>(&'a MemoryBlobs);

impl TocSink for Sink<'_> {
    fn put(&mut self, bytes: &[u8]) -> Result<String, String> {
        let digest = format!("{:x}", Sha256::digest(bytes));
        self.0
             .0
            .lock()
            .unwrap()
            .insert(digest.clone(), bytes.to_vec());
        Ok(digest)
    }
}

fn properties(value: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&value).unwrap()
}

/// Ingest `SOURCE` the way `SourceIngest` does: build and store the table, then
/// name it on the document node of the graph.
fn ingest(blobs: &MemoryBlobs, core: &GraphCore, document_id: &str, revision: u64) -> StagedToc {
    let staged = build_and_store_toc(
        &mut Sink(blobs),
        document_id,
        revision,
        SOURCE.as_bytes(),
        16,
    )
    .unwrap();
    let mut node = serde_json::Map::new();
    node.insert("type".into(), "Document".into());
    for (key, value) in staged.node_properties() {
        node.insert(key.into(), value);
    }
    core.add_node(document_id.into(), properties(node.into()));
    staged
}

fn span(document_id: &str, revision: u64, line_start: u64, line_end: u64) -> SpanRequest {
    SpanRequest {
        document_id: document_id.into(),
        revision,
        line_start,
        line_end,
    }
}

fn result_of(response: &Response) -> ReadSpanResult {
    match response.result.as_ref() {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value.clone()).unwrap(),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("expected a span, got {other:?} / {:?}", response.error),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R153.3
#[test]
fn ingest_stores_a_toc_and_read_span_returns_its_digest() {
    let blobs = MemoryBlobs::default();
    // Tenant A's graph holds the document; tenant B's graph does not.
    let tenant_a = GraphCore::new();
    let tenant_b = GraphCore::new();
    let staged = ingest(&blobs, &tenant_a, "doc-a", 7);

    // The node names the stored table, text and revision.
    let node = medulla_types::msgpack::decode_property_value(
        &tenant_a.get_node_properties("doc-a").unwrap(),
    )
    .unwrap();
    assert_eq!(node["toc_digest"], json!(staged.toc_digest));
    assert_eq!(node["source_digest"], json!(staged.source_digest));
    assert_eq!(node["source_revision"], json!(7));
    // The stored table is the one the node names.
    let stored = SourceToc::from_json(&blobs.get(&staged.toc_digest).unwrap().unwrap()).unwrap();
    assert_eq!(stored, staged.toc);

    // ReadSpan returns the table digest and the requested lines.
    let response = serve_read_span(1, &tenant_a, &blobs, &span("doc-a", 7, 2, 4));
    let served = result_of(&response);
    assert_eq!(served.toc_digest, staged.toc_digest);
    assert_eq!(served.text, "# Title\nintro\n\n");

    // Another tenant's graph (or a caller that cannot see the node) gets no
    // document, and no blob byte is returned.
    let foreign = serve_read_span(2, &tenant_b, &blobs, &span("doc-a", 7, 2, 4));
    assert!(foreign.result.is_none());
    assert!(foreign
        .refusal_text()
        .unwrap()
        .starts_with("DOCUMENT_NOT_FOUND"));

    // A revision without a table is refused, not guessed.
    let stale = serve_read_span(3, &tenant_a, &blobs, &span("doc-a", 6, 2, 4));
    assert!(stale.refusal_text().unwrap().starts_with("TOC_MISSING"));
    assert!(
        serve_read_span(4, &tenant_a, &blobs, &span("doc-a", 7, 2, 99))
            .refusal_text()
            .unwrap()
            .starts_with("SPAN_OUT_OF_BOUNDS")
    );
    assert!(
        serve_read_span(5, &tenant_a, &blobs, &span("doc-a", 7, 5, 4))
            .refusal_text()
            .unwrap()
            .starts_with("SPAN_INVERTED")
    );
}

// spec: MEDULLA-DECISION-ENGINE-R153.5
#[test]
fn span_response_binds_toc_and_source_digests() {
    let blobs = MemoryBlobs::default();
    let core = GraphCore::new();
    let staged = ingest(&blobs, &core, "doc-b", 3);

    let served = result_of(&serve_read_span(1, &core, &blobs, &span("doc-b", 3, 5, 7)));

    // Recompute both digests from the stored bytes and compare every field.
    let toc_bytes = blobs.get(&served.toc_digest).unwrap().unwrap();
    assert_eq!(
        served.toc_digest,
        format!("{:x}", Sha256::digest(&toc_bytes))
    );
    assert_eq!(served.toc_digest, staged.toc_digest);
    let source_bytes = blobs.get(&served.source_digest).unwrap().unwrap();
    assert_eq!(
        served.source_digest,
        format!("{:x}", Sha256::digest(&source_bytes))
    );
    assert_eq!(source_bytes, SOURCE.as_bytes());
    assert_eq!(served.document_id, "doc-b");
    assert_eq!(served.revision, 3);
    assert_eq!((served.line_start, served.line_end), (5, 7));
    assert_eq!(served.text, "## Part A\na1\na2\n");
    assert_eq!(
        &source_bytes[served.byte_start as usize..served.byte_end as usize],
        served.text.as_bytes()
    );
    // The last line has no newline, and is served whole.
    let last = result_of(&serve_read_span(
        2,
        &core,
        &blobs,
        &span("doc-b", 3, 11, 11),
    ));
    assert_eq!(last.text, "last line");
    assert_eq!(last.byte_end as usize, SOURCE.len());
}

// spec: MEDULLA-DECISION-ENGINE-R153.6
#[test]
fn served_ingest_then_bounded_span_read() {
    let blobs = MemoryBlobs::default();
    let core = GraphCore::new();
    // Ingest, then read: every section range of the original source comes back
    // as the exact bytes of that range, and an out-of-bounds range is refused.
    let staged = ingest(&blobs, &core, "doc-journey", 9);
    let lines: Vec<&str> = SOURCE.split_inclusive('\n').collect();
    let total = lines.len() as u64;
    assert_eq!(staged.toc.total_lines, total);

    for (start, end) in [(1u64, 1u64), (2, 4), (5, 7), (8, 10), (11, 11), (1, total)] {
        let served = result_of(&serve_read_span(
            start,
            &core,
            &blobs,
            &span("doc-journey", 9, start, end),
        ));
        let expected: String = lines[(start - 1) as usize..end as usize].concat();
        assert_eq!(served.text, expected, "lines {start}..={end}");
        assert_eq!(served.toc_digest, staged.toc_digest);
        assert_eq!(
            &SOURCE.as_bytes()[served.byte_start as usize..served.byte_end as usize],
            expected.as_bytes()
        );
    }

    let past_end = serve_read_span(100, &core, &blobs, &span("doc-journey", 9, 5, total + 1));
    assert!(past_end.result.is_none());
    assert!(past_end
        .refusal_text()
        .unwrap()
        .starts_with("SPAN_OUT_OF_BOUNDS"));
}

// spec: MEDULLA-DECISION-ENGINE-R153.3
#[cfg(feature = "security")]
#[test]
fn read_span_respects_visibility() {
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use medulla_server_api::carrier::GraphReadAuthority;
    use std::sync::Arc;

    let blobs = MemoryBlobs::default();
    let raw = Arc::new(GraphCore::new());
    let staged =
        build_and_store_toc(&mut Sink(&blobs), "bob-doc", 1, SOURCE.as_bytes(), 16).unwrap();
    let mut node = serde_json::Map::new();
    node.insert("type".into(), "Document".into());
    node.insert("_owner".into(), "bob".into());
    node.insert("_visibility".into(), "private".into());
    for (key, value) in staged.node_properties() {
        node.insert(key.into(), value);
    }
    raw.add_node("bob-doc".into(), properties(node.into()));

    let isolation = medulla_server_auth::access::two_agent_isolation();
    let alice = GraphReadAuthority::from_verified(
        &VerifiedRequestContext::verified_for_test("alice"),
        &isolation,
    )
    .unwrap();
    let bob = GraphReadAuthority::from_verified(
        &VerifiedRequestContext::verified_for_test("bob"),
        &isolation,
    )
    .unwrap();
    let ask = span("bob-doc", 1, 1, 2);

    let hidden = serve_read_span(1, &alice.project_core(&raw), &blobs, &ask);
    assert!(hidden.result.is_none());
    assert!(hidden
        .refusal_text()
        .unwrap()
        .starts_with("DOCUMENT_NOT_FOUND"));
    let owned = result_of(&serve_read_span(2, &bob.project_core(&raw), &blobs, &ask));
    assert_eq!(owned.text, "preamble\n# Title\n");
}

/// Counts every blob read, so a refusal can prove it read nothing.
struct CountingBlobs<'a> {
    inner: &'a MemoryBlobs,
    reads: std::sync::atomic::AtomicUsize,
}

impl SpanBlobs for CountingBlobs<'_> {
    fn get(&self, digest: &str) -> Result<Option<Vec<u8>>, String> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.get(digest)
    }
}

// spec: MEDULLA-DECISION-ENGINE-R153.4
#[test]
fn read_span_refusals() {
    let memory = MemoryBlobs::default();
    let core = GraphCore::new();
    ingest(&memory, &core, "doc-r", 4);
    let blobs = CountingBlobs {
        inner: &memory,
        reads: std::sync::atomic::AtomicUsize::new(0),
    };
    let registered: Vec<&str> = medulla_types::source_toc::REFUSALS
        .iter()
        .map(|code| code.as_str())
        .collect();

    let cases = [
        ("SPAN_INVERTED", span("doc-r", 4, 5, 4)),
        ("SPAN_OUT_OF_BOUNDS", span("doc-r", 4, 2, 99)),
        ("SPAN_OUT_OF_BOUNDS", span("doc-r", 4, 0, 3)),
        ("SPAN_TOO_LARGE", span("doc-r", 4, 1, 5000)),
        ("TOC_MISSING", span("doc-r", 3, 2, 4)),
        ("DOCUMENT_NOT_FOUND", span("absent", 4, 1, 2)),
    ];
    for (code, request) in cases {
        let response = serve_read_span(1, &core, &blobs, &request);
        assert!(response.result.is_none(), "{code}: no result");
        assert!(
            response.refusal_text().unwrap().starts_with(code),
            "{code}: got {:?}",
            response.refusal_text()
        );
        if code != "DOCUMENT_NOT_FOUND" {
            assert!(
                registered.contains(&code),
                "{code} is in the REFUSALS table"
            );
        }
        assert_eq!(
            blobs.reads.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "{code}: a refusal reads no blob byte"
        );
    }

    // A valid range reads blobs, so the counter is live.
    let ok = serve_read_span(2, &core, &blobs, &span("doc-r", 4, 2, 4));
    assert!(ok.result.is_some());
    assert!(blobs.reads.load(std::sync::atomic::Ordering::SeqCst) > 0);
}
