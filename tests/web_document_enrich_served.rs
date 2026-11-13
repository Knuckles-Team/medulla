//! MEDULLA-UNIFIED-DATA-PLANE-R651.7 and MEDULLA-DECISION-ENGINE-R802 through the real
//! `dispatch` shell over a redb-backed `ServerState`.
//!
//! A held web Document is enriched: its chunks, `chunkOf` edges and vectors
//! commit through the ChangeEnvelope authority, `SemanticSearch` finds a
//! chunk by its own vector, a re-run replays without advancing the graph, a
//! new text supersedes the earlier chunks, and both fetch backends store the
//! same chunk set. `SelectWebPages` then dedupes against the held pages.

#![cfg(all(
    feature = "redb",
    feature = "query",
    feature = "modality-serving",
    feature = "decide"
))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_decision_outcome_types::decision::web_fetch_selection::SelectWebPagesAnswer;
use medulla_decision_outcome_types::decision::web_fetch_selection::SelectWebPagesRequest;
use medulla_decision_outcome_types::decision::web_fetch_selection::SkipReason;
use medulla_decision_outcome_types::decision::web_fetch_selection::WebCandidate;
use medulla_exec::exec::TextEmbedder;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_source_types::source_ingestion::web_document::AdmissionOutcome;
use medulla_source_types::source_ingestion::web_document::EnrichDocumentReceipt;
use medulla_source_types::source_ingestion::web_document::EnrichDocumentRequest;
use medulla_source_types::source_ingestion::web_document::FetchBackend;

const SECRET: &str = "web-document-enrich-secret";
const GRAPH: &str = "webenrich";
const NOW: u64 = 1_800_000_000_000;
const HOUR: u64 = 3_600_000;

async fn call(state: &test_support::SharedState, id: u64, method: Method) -> Response {
    test_support::call_ok(state, SECRET, id, GRAPH, method).await
}

fn raw<T: serde::de::DeserializeOwned>(response: Response) -> T {
    match response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("typed result"),
        other => panic!("unexpected payload {other:?}"),
    }
}

fn page_text(extra: &str) -> String {
    let mut text = String::from("# Web ingest\n\nPages enter as Documents.\n\n");
    for section in 0..6 {
        text.push_str(&format!("## Section {section}\n\n"));
        text.push_str(&"Chunks carry offsets so a citation can quote the source. ".repeat(12));
        text.push_str("\n\n");
    }
    text.push_str(extra);
    text
}

async fn add_document(
    state: &test_support::SharedState,
    id: u64,
    node_id: &str,
    text: &str,
    url: &str,
    retrieved_at_ms: u64,
    digest: &str,
) {
    let properties = serde_json::json!({
        "type": "Document",
        "title": "Web ingest",
        "text": text,
        "url": url,
        "retrieved_at": retrieved_at_ms,
        "content_digest": digest,
    });
    call(
        state,
        id,
        Method::AddNode {
            node_id: node_id.into(),
            properties_msgpack: rmp_serde::to_vec_named(&properties).unwrap(),
        },
    )
    .await;
}

async fn enrich(
    state: &test_support::SharedState,
    id: u64,
    node_id: &str,
    backend: FetchBackend,
) -> EnrichDocumentReceipt {
    let request = EnrichDocumentRequest {
        document_id: node_id.into(),
        text_property: None,
        fetch_backend: Some(backend),
        max_chunk_bytes: Some(600),
        overlap_bytes: Some(80),
    };
    raw(call(
        state,
        id,
        Method::EnrichDocument {
            request: Box::new(request),
        },
    )
    .await)
}

async fn bool_call(state: &test_support::SharedState, id: u64, method: Method) -> bool {
    match call(state, id, method).await.result {
        Some(ResultPayload::Bool(value)) => value,
        other => panic!("unexpected payload {other:?}"),
    }
}

async fn chunk_props(state: &test_support::SharedState, id: u64, node: &str) -> serde_json::Value {
    let response = call(
        state,
        id,
        Method::GetNodeProperties {
            node_id: node.into(),
        },
    )
    .await;
    match response.result {
        Some(ResultPayload::Raw(bytes)) => {
            medulla_types::msgpack::decode_property_value(&bytes).unwrap()
        }
        Some(ResultPayload::Json(value)) => value,
        other => panic!("unexpected payload {other:?}"),
    }
}

/// What the store holds for a chunk that should be linked, for a failure message.
async fn diagnose(
    state: &test_support::SharedState,
    chunk: &str,
    receipt: &EnrichDocumentReceipt,
) -> String {
    let mut out = format!("receipt={receipt:?}; version={}", graph_version(state));
    let probes = [
        (
            "has_chunk",
            Method::HasNode {
                node_id: chunk.into(),
            },
        ),
        (
            "has_record",
            Method::HasNode {
                node_id: receipt.enrichment_id.clone(),
            },
        ),
        (
            "doc_to_chunk",
            Method::HasEdge {
                source_id: "doc:a".into(),
                target_id: chunk.into(),
            },
        ),
        (
            "doc_to_record",
            Method::HasEdge {
                source_id: "doc:a".into(),
                target_id: receipt.enrichment_id.clone(),
            },
        ),
        (
            "chunk_props",
            Method::GetNodeProperties {
                node_id: chunk.into(),
            },
        ),
        ("node_count", Method::NodeCount),
    ];
    for (n, (label, method)) in probes.into_iter().enumerate() {
        let response = test_support::dispatch(
            state,
            test_support::request(SECRET, 900 + n as u64, GRAPH, method),
        )
        .await;
        let value = match &response.result {
            Some(ResultPayload::Raw(bytes)) => medulla_types::msgpack::decode_property_value(bytes)
                .map(|v| v.to_string())
                .unwrap_or_else(|_| format!("{} raw bytes", bytes.len())),
            other => format!("{other:?}"),
        };
        out.push_str(&format!("; {label}={value} err={:?}", response.error));
    }
    out
}

fn graph_version(state: &test_support::SharedState) -> u64 {
    state
        .try_read()
        .unwrap()
        .registry
        .get(GRAPH)
        .unwrap()
        .core
        .version()
}

async fn durable_graph() -> (
    std::path::PathBuf,
    test_support::SharedPersistence,
    test_support::SharedState,
) {
    test_support::durable_graph_state(
        SECRET,
        "web-document-enrich-encryption-key",
        "medulla-webenrich",
        common::current_isolation(),
        GRAPH,
    )
    .await
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R651.7
// spec: MEDULLA-UNIFIED-DATA-PLANE-R651.7.3
// spec: MEDULLA-UNIFIED-DATA-PLANE-R651.7.5
// spec: MEDULLA-UNIFIED-DATA-PLANE-R651.7.6
// spec: MEDULLA-DECISION-ENGINE-R802
// spec: MEDULLA-DECISION-ENGINE-R802.3
// spec: MEDULLA-DECISION-ENGINE-R802.6
#[test]
fn test_web_page_enriches_once_for_both_backends() {
    test_support::on_engine(enrich_replay_supersede_and_search);
}

async fn enrich_replay_supersede_and_search() {
    let (_dir, _backend, state) = durable_graph().await;
    let text = page_text("");
    add_document(
        &state,
        2,
        "doc:a",
        &text,
        "https://example.org/a",
        NOW - 48 * HOUR,
        "sha256:aa",
    )
    .await;
    add_document(
        &state,
        3,
        "doc:b",
        &text,
        "https://example.org/b",
        NOW - HOUR,
        "sha256:bb",
    )
    .await;

    let a = enrich(&state, 4, "doc:a", FetchBackend::Webarchiver).await;
    assert!(!a.replayed);
    assert!(
        a.chunk_count > 3,
        "a multi-section page chunks into several windows"
    );
    assert_eq!(a.vector_count, a.chunk_count);
    assert_eq!(
        a.embedder, "hash",
        "no endpoint configured: the hash fallback is recorded"
    );
    assert_eq!(a.fetch_backend, Some(FetchBackend::Webarchiver));
    for (id, chunk) in a.chunk_ids.iter().enumerate() {
        let linked = bool_call(
            &state,
            10 + id as u64,
            Method::HasEdge {
                source_id: chunk.clone(),
                target_id: "doc:a".into(),
            },
        )
        .await;
        if !linked {
            panic!(
                "chunk {chunk} has no chunkOf edge: {}",
                diagnose(&state, chunk, &a).await
            );
        }
    }
    let first = chunk_props(&state, 40, &a.chunk_ids[1]).await;
    assert_eq!(first["fetch_backend"], "webarchiver");
    assert_eq!(first["embedder"], "hash");
    assert_eq!(first["url"], "https://example.org/a");
    let (start, end) = (
        first["start_char"].as_u64().unwrap() as usize,
        first["end_char"].as_u64().unwrap() as usize,
    );
    let cited: String = text.chars().skip(start).take(end - start).collect();
    assert_eq!(first["text"], cited, "offsets address the exact passage");

    // The vector path: SemanticSearch finds the chunk by its own vector.
    let query = medulla_exec::exec::HashEmbedder::new(a.dimensions as usize)
        .embed(&cited)
        .unwrap();
    let hits: Vec<(String, f32)> = raw(call(
        &state,
        41,
        Method::SemanticSearch {
            query_embedding: query,
            n_results: 3,
        },
    )
    .await);
    assert_eq!(
        hits.first().map(|h| h.0.as_str()),
        Some(a.chunk_ids[1].as_str())
    );

    // Idempotent by content digest: nothing is written again.
    let version = graph_version(&state);
    let again = enrich(&state, 42, "doc:a", FetchBackend::Webarchiver).await;
    assert!(again.replayed);
    assert_eq!(again.chunk_ids, a.chunk_ids);
    assert_eq!(
        graph_version(&state),
        version,
        "an enriched digest must not commit twice"
    );

    // Both backends store the same chunk set; only the recorded backend differs.
    let b = enrich(&state, 43, "doc:b", FetchBackend::Archivebox).await;
    assert_eq!(b.chunk_count, a.chunk_count);
    assert_eq!(b.content_digest, a.content_digest);
    let b_first = chunk_props(&state, 44, &b.chunk_ids[1]).await;
    assert_eq!(b_first["fetch_backend"], "archivebox");
    for key in ["text", "start_char", "end_char", "heading_path"] {
        assert_eq!(
            b_first[key], first[key],
            "{key} must not depend on the backend"
        );
    }

    // A new text supersedes: the earlier digest's chunks are removed.
    add_document(
        &state,
        45,
        "doc:a",
        &page_text("A new closing paragraph."),
        "https://example.org/a",
        NOW,
        "sha256:cc",
    )
    .await;
    let changed = enrich(&state, 46, "doc:a", FetchBackend::Webarchiver).await;
    assert!(!changed.replayed);
    assert_eq!(changed.superseded_chunks, a.chunk_count);
    assert!(
        !bool_call(
            &state,
            47,
            Method::HasNode {
                node_id: a.chunk_ids[0].clone()
            }
        )
        .await
    );
    assert!(
        bool_call(
            &state,
            48,
            Method::HasNode {
                node_id: changed.chunk_ids[0].clone()
            }
        )
        .await
    );

    // Selection over the held pages.
    let row = |url: &str, rank: u32, digest: Option<&str>| WebCandidate {
        url: url.into(),
        title: "Web ingest offsets".into(),
        snippet: "citation offsets".into(),
        engine: "searxng".into(),
        rank,
        content_digest: digest.map(str::to_string),
        published_at_ms: None,
    };
    let request = SelectWebPagesRequest {
        query: "web ingest citation offsets".into(),
        candidates: vec![
            row("https://example.org/b", 1, None),
            row("https://example.org/new", 2, None),
            row("https://example.org/copy", 3, Some("sha256:cc")),
            row("https://example.org/new?utm_source=x", 4, None),
        ],
        max_fetches: 4,
        freshness_window_s: Some(6 * 3_600),
        now_ms: Some(NOW + HOUR),
    };
    let answer: SelectWebPagesAnswer = raw(call(
        &state,
        49,
        Method::SelectWebPages {
            request: Box::new(request),
        },
    )
    .await);
    let reason = |url: &str| {
        answer
            .skipped
            .iter()
            .find(|s| s.url == url)
            .map(|s| s.reason)
    };
    assert_eq!(reason("https://example.org/b"), Some(SkipReason::HeldFresh));
    assert_eq!(
        reason("https://example.org/copy"),
        Some(SkipReason::DuplicateDigest)
    );
    assert_eq!(
        reason("https://example.org/new?utm_source=x"),
        Some(SkipReason::DuplicateUrl)
    );
    assert_eq!(answer.selected.len(), 1);
    assert_eq!(answer.selected[0].canonical_url, "https://example.org/new");
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R651.7.4
#[test]
fn test_web_document_refusals() {
    test_support::on_engine(refusals);
}

async fn refusals() {
    let (_dir, _backend, state) = durable_graph().await;
    let request = |id: &str| EnrichDocumentRequest {
        document_id: id.into(),
        text_property: None,
        fetch_backend: None,
        max_chunk_bytes: None,
        overlap_bytes: None,
    };
    let missing = test_support::dispatch(
        &state,
        test_support::request(
            SECRET,
            2,
            GRAPH,
            Method::EnrichDocument {
                request: Box::new(request("doc:none")),
            },
        ),
    )
    .await;
    assert!(format!("{:?}", missing.error).contains("DOCUMENT_NOT_FOUND"));
    let properties = serde_json::json!({"type": "Document", "url": "https://example.org/x"});
    call(
        &state,
        3,
        Method::AddNode {
            node_id: "doc:empty".into(),
            properties_msgpack: rmp_serde::to_vec_named(&properties).unwrap(),
        },
    )
    .await;
    let version = graph_version(&state);
    let empty = test_support::dispatch(
        &state,
        test_support::request(
            SECRET,
            4,
            GRAPH,
            Method::EnrichDocument {
                request: Box::new(request("doc:empty")),
            },
        ),
    )
    .await;
    assert!(format!("{:?}", empty.error).contains("DOCUMENT_TEXT_MISSING"));
    assert_eq!(graph_version(&state), version, "a refusal writes nothing");
}

// spec: MEDULLA-DECISION-ENGINE-R080.2
#[test]
fn persisted_facts_carry_their_rung_and_resist_overwrite() {
    test_support::on_engine(rung_and_overwrite);
}

async fn rung_and_overwrite() {
    let (_dir, _backend, state) = durable_graph().await;
    let text = page_text("");
    add_document(
        &state,
        2,
        "doc:a",
        &text,
        "https://example.org/a",
        NOW - HOUR,
        "sha256:aa",
    )
    .await;
    let receipt = enrich(&state, 3, "doc:a", FetchBackend::Webarchiver).await;
    let record = chunk_props(&state, 4, &receipt.enrichment_id).await;
    assert_eq!(
        record["producing_rung"], "embeddings",
        "the stored enrichment fact carries its producing rung"
    );

    // A fact an AST rung already established on the same Document.
    add_document(
        &state,
        5,
        "doc:b",
        &text,
        "https://example.org/b",
        NOW - HOUR,
        "sha256:bb",
    )
    .await;
    let seeded = "enrichment:doc:b:ast-seed";
    call(
        &state,
        6,
        Method::AddNode {
            node_id: seeded.into(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                "type": "DocumentEnrichment",
                "document_id": "doc:b",
                "content_digest": format!("sha256:{}", "ab".repeat(32)),
                "chunk_count": 0,
                "producing_rung": "ast_extraction",
            }))
            .unwrap(),
        },
    )
    .await;
    let version = graph_version(&state);
    let refused = test_support::dispatch(
        &state,
        test_support::request(
            SECRET,
            7,
            GRAPH,
            Method::EnrichDocument {
                request: Box::new(EnrichDocumentRequest {
                    document_id: "doc:b".into(),
                    text_property: None,
                    fetch_backend: Some(FetchBackend::Webarchiver),
                    max_chunk_bytes: Some(600),
                    overlap_bytes: Some(80),
                }),
            },
        ),
    )
    .await;
    assert!(
        format!("{:?}", refused.error).contains("RUNG_OUTRANKED"),
        "the embedding candidate is refused: {:?}",
        refused.error
    );
    assert_eq!(graph_version(&state), version, "a refusal writes nothing");
    let kept = chunk_props(&state, 8, seeded).await;
    assert_eq!(
        kept["producing_rung"], "ast_extraction",
        "the stored lower-rung fact stays in place"
    );
}

/// A page whose sections all differ, so no span repeats another.
fn varied_text() -> String {
    let words = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india",
        "juliet", "kilo", "lima",
    ];
    let mut text = String::from("# Varied page\n\n");
    for section in 0..6 {
        text.push_str(&format!("## Section {section}\n\n"));
        for n in 0..14 {
            text.push_str(&format!(
                "{} {} section {section} sentence {n} adds {} detail. ",
                words[(section + n) % 12],
                words[(section * 5 + n * 7) % 12],
                words[(n * 3 + section) % 12],
            ));
        }
        text.push_str("\n\n");
    }
    text
}

fn outcomes(receipt: &EnrichDocumentReceipt) -> Vec<AdmissionOutcome> {
    receipt.admissions.iter().map(|a| a.outcome).collect()
}

// spec: MEDULLA-DECISION-ENGINE-R079.2
#[test]
fn excluded_units_make_no_embedding_call() {
    test_support::on_engine(excluded_units_run);
}

async fn excluded_units_run() {
    let (_dir, _backend, state) = durable_graph().await;
    let text = varied_text();
    let generated = format!("// @generated by a tool\n{text}");
    let minified = format!("{text}\n{}", "m".repeat(1200));
    let cases = [
        (
            "doc:lock",
            text.clone(),
            "https://example.org/app/Cargo.lock",
        ),
        (
            "doc:vendor",
            text.clone(),
            "https://example.org/vendor/pkg/readme.md",
        ),
        ("doc:gen", generated, "https://example.org/gen.rs"),
        ("doc:min", minified, "https://example.org/app.min.js"),
    ];
    for (n, (id, body, url)) in cases.iter().enumerate() {
        add_document(&state, 2 + n as u64, id, body, url, NOW, "sha256:aa").await;
        let receipt = enrich(&state, 20 + n as u64, id, FetchBackend::Webarchiver).await;
        assert!(!receipt.admissions.is_empty(), "{id}: one verdict per span");
        assert!(
            outcomes(&receipt)
                .iter()
                .all(|o| *o == AdmissionOutcome::Excluded),
            "{id}: {:?}",
            receipt.admissions
        );
        assert_eq!(
            receipt.embedded_units, 0,
            "{id}: the embedder is not called"
        );
        assert_eq!((receipt.chunk_count, receipt.vector_count), (0, 0), "{id}");
    }
    // A human-written page is embedded span by span.
    add_document(
        &state,
        40,
        "doc:ok",
        &text,
        "https://example.org/ok",
        NOW,
        "sha256:bb",
    )
    .await;
    let ok = enrich(&state, 41, "doc:ok", FetchBackend::Webarchiver).await;
    assert_eq!(ok.embedded_units, ok.chunk_count);
    assert!(ok.chunk_count > 3);
    assert!(outcomes(&ok)
        .iter()
        .all(|o| *o == AdmissionOutcome::Admitted));
    assert_eq!(ok.admissions[0].class, "prose");

    // A changed page re-embeds only the spans whose text changed: an
    // identical span is skipped by exact digest and keeps its vector.
    let edited = format!("{text}\nA closing paragraph about hotel and india.\n");
    add_document(
        &state,
        42,
        "doc:ok",
        &edited,
        "https://example.org/ok",
        NOW,
        "sha256:cc",
    )
    .await;
    let again = enrich(&state, 43, "doc:ok", FetchBackend::Webarchiver).await;
    let skipped = outcomes(&again)
        .iter()
        .filter(|o| **o == AdmissionOutcome::SkippedExact)
        .count();
    assert!(skipped > 0, "unchanged spans skip: {:?}", again.admissions);
    assert!(again.embedded_units < again.chunk_count);
    assert_eq!(again.vector_count, again.chunk_count);
}

// spec: MEDULLA-DECISION-ENGINE-R079.3
#[test]
fn duplicate_low_entropy_and_oversized_units_do_not_embed() {
    test_support::on_engine(admission_bounds_run);
}

async fn admission_bounds_run() {
    let (_dir, _backend, state) = durable_graph().await;
    let sentence = "A distinct sentence about retrieval budgets and their ceilings.";
    let mut text = String::from("# Bounds\n\n");
    text.push_str(&format!("## Dup\n\n{sentence}\n\n"));
    text.push_str(&format!("## Dup\n\n{}\n\n", sentence.to_uppercase()));
    text.push_str(&format!("## Flat\n\n{}\n\n", "a".repeat(200)));
    text.push_str("## Tiny\n\nno\n\n");
    text.push_str(&varied_text());
    add_document(
        &state,
        2,
        "doc:b",
        &text,
        "https://example.org/b",
        NOW,
        "sha256:aa",
    )
    .await;
    let request = EnrichDocumentRequest {
        document_id: "doc:b".into(),
        text_property: None,
        fetch_backend: Some(FetchBackend::Webarchiver),
        max_chunk_bytes: Some(600),
        overlap_bytes: Some(80),
    };
    let receipt: EnrichDocumentReceipt = raw(call(
        &state,
        3,
        Method::EnrichDocument {
            request: Box::new(request),
        },
    )
    .await);
    let verdicts = outcomes(&receipt);
    let admitted = verdicts
        .iter()
        .filter(|o| **o == AdmissionOutcome::Admitted)
        .count() as u32;
    assert!(admitted > 0);
    assert_eq!(
        receipt.embedded_units, admitted,
        "only admitted spans embed"
    );
    assert_eq!(receipt.chunk_count, admitted);
    assert_eq!(receipt.vector_count, admitted);
    for skip in [
        AdmissionOutcome::Duplicate,
        AdmissionOutcome::LowEntropy,
        AdmissionOutcome::SizeOutOfBounds,
    ] {
        assert!(
            verdicts.contains(&skip),
            "{skip:?} expected: {:?}",
            receipt.admissions
        );
    }
    assert!(verdicts.len() as u32 > admitted);
}

/// A page of `sections` distinct prose sections under one `tag`: each section
/// is long enough that the chunker keeps it as one span, and no two sections
/// share text, so no unit is a duplicate of another.
fn distinct_page(tag: &str, sections: usize) -> String {
    let words = [
        "amber", "birch", "cedar", "dune", "ember", "fjord", "glade", "heath", "inlet", "jetty",
        "knoll", "larch", "marsh", "nook", "oasis", "pine", "quarry", "ridge", "spruce", "tarn",
    ];
    let mut text = format!("# Notes {tag}\n\n");
    for section in 0..sections {
        text.push_str(&format!("## Entry {tag} {section}\n\n"));
        for n in 0..9 {
            text.push_str(&format!(
                "{tag} entry {section} remark {n} pairs {} with {} near {} at mile {}. ",
                words[(section + n) % 20],
                words[(section * 3 + n * 7 + 1) % 20],
                words[(section * 11 + n * 5 + 2) % 20],
                section * 13 + n,
            ));
        }
        text.push_str("\n\n");
    }
    text
}

async fn enrich_wide(
    state: &test_support::SharedState,
    id: u64,
    node_id: &str,
) -> EnrichDocumentReceipt {
    let request = EnrichDocumentRequest {
        document_id: node_id.into(),
        text_property: None,
        fetch_backend: Some(FetchBackend::Webarchiver),
        max_chunk_bytes: Some(800),
        overlap_bytes: Some(0),
    };
    raw(call(
        state,
        id,
        Method::EnrichDocument {
            request: Box::new(request),
        },
    )
    .await)
}

/// Commit a retrieval-plan record through the served `Decide` and
/// `DecisionLog` Commit, as the test principal, and rewrite its stored outcome
/// to an executed option (the log join reads the stored entry; it does not
/// replay). Returns the record id.
async fn committed_executed_retrieval_record(state: &test_support::SharedState) -> String {
    use medulla_agent_types::agent_component::AgentComponentKind;
    use medulla_decision_types::decision::statistical::body::encode_body;
    use medulla_decision_types::decision::statistical::declared::{DeclaredNumber, DeclaredOption};
    use medulla_decision_types::decision::statistical::features::{
        FeatureKind, FeatureSchemaBody, FeatureSpec, MissingValue, FEATURE_SCHEMA_VERSION,
    };
    use medulla_decision_types::decision::statistical::log::{
        DecisionLogCommitted, DecisionLogEntry, DecisionLogOp,
    };
    use medulla_decision_types::decision::statistical::{
        CandidateSource, DecideRequest, DecisionBatch, QuestionKind, QuestionSafety,
        StatisticalOutcome, StatisticalQuestion,
    };
    use medulla_decision_types::decision::{DecisionPolicyRef, UnitRationalWire};
    use medulla_h_decide::decide::stat_test_support::Harness;
    use medulla_persistence_records::agent_component::test_component_draft;
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;
    use medulla_types::contract::BoundedVec;

    let store = state.write().await.ensure_agent_library().unwrap();
    let harness = Harness {
        _dir: tempfile::tempdir().unwrap(),
        state: state.clone(),
        store: store.clone(),
        nonce: std::cell::Cell::new(1),
    };
    let tenant = common::TEST_TENANT;
    let schema = FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![FeatureSpec {
            name: "score".to_string(),
            kind: FeatureKind::Number {
                key: "score".to_string(),
            },
            missing: MissingValue::Abstain,
        }])
        .unwrap(),
    };
    let encoded = encode_body(&schema).unwrap();
    let mut draft = test_component_draft(tenant, "schema-retrieval-plan");
    draft.kind = AgentComponentKind::FeatureSchema;
    draft.summary = "declared-option features".to_string();
    draft.classification = vec!["eg:capability/retrieval/web-search".to_string()];
    draft.content_digest = encoded.content_digest;
    draft.attributes = encoded.attributes;
    let schema_pin = harness.commit_in(tenant, draft, None).unwrap();
    let option = |id: &str, score: i64| DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::new(vec![DeclaredNumber {
            key: "score".to_string(),
            q32: score,
        }])
        .unwrap(),
        texts: BoundedVec::default(),
    };
    let asked = DecideRequest {
        tenant_id: tenant.to_string(),
        question: StatisticalQuestion {
            question_id: "au.retrieval-plan".to_string(),
            kind: QuestionKind::RetrievalPlan,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![
                option("plan-deep", 1 << 31),
                option("plan-hyde", 1 << 32),
            ])
            .unwrap(),
        },
        feature_schema: schema_pin,
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::default(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    };
    let batch: DecisionBatch = raw(call(
        state,
        300,
        Method::Decide {
            request: Box::new(asked),
        },
    )
    .await);
    let record = batch.records.as_slice()[0].clone();
    let committed: DecisionLogCommitted = raw(call(
        state,
        301,
        Method::DecisionLog {
            op: Box::new(DecisionLogOp::Commit {
                record: Box::new(record),
                evaluator: None,
            }),
        },
    )
    .await);
    let key = medulla_persistence_records::decision_jobs::record_key(&committed.record_id);
    let bytes = store.decision_artifact(tenant, &key).unwrap().unwrap();
    let mut entry: DecisionLogEntry =
        medulla_persistence_records::decision_jobs::decode_artifact(&bytes, "entry").unwrap();
    entry.record.outcome = StatisticalOutcome::Explored {
        option_id: "plan-hyde".to_string(),
        propensity: UnitRationalWire::new(1, 1).unwrap(),
    };
    let bytes = medulla_persistence_records::decision_jobs::encode_artifact(&entry).unwrap();
    store
        .put_decision_artifacts(tenant, &[(key, bytes)])
        .unwrap();
    committed.record_id
}

// spec: MEDULLA-DECISION-ENGINE-R079.5
#[test]
fn retrieval_feedback_demotes_unused_content() {
    test_support::on_engine(demotion_run);
}

async fn demotion_run() {
    use medulla_decision_types::decision::statistical::log::DecisionLogOp;
    use medulla_decision_types::decision::statistical::retrieval::{
        LearningWrite, RetrievalOutcome, ReturnedEvidence,
    };
    use medulla_types::contract::BoundedVec;

    let (_dir, _backend, state) = durable_graph().await;

    // Page one: 150 distinct prose spans embed, enough to pass
    // `embedding.demote_min_units`; nothing has been cited, but no class is
    // demoted yet because the demotion reads the units already held.
    add_document(
        &state,
        2,
        "doc:p1",
        &distinct_page("one", 150),
        "https://example.org/p1",
        NOW,
        "sha256:p1",
    )
    .await;
    let first = enrich_wide(&state, 3, "doc:p1").await;
    assert!(
        first.demoted_classes.is_empty(),
        "{:?}",
        first.demoted_classes
    );
    assert!(first.embedded_units >= 100, "{}", first.embedded_units);
    assert!(
        outcomes(&first)
            .iter()
            .all(|o| *o == AdmissionOutcome::Admitted),
        "{:?}",
        first.admissions
    );

    // Page two: the held prose has never been cited, so the class is demoted
    // and no span of the new page embeds.
    add_document(
        &state,
        4,
        "doc:p2",
        &distinct_page("two", 20),
        "https://example.org/p2",
        NOW,
        "sha256:p2",
    )
    .await;
    let second = enrich_wide(&state, 5, "doc:p2").await;
    assert_eq!(second.demoted_classes, vec!["prose".to_string()]);
    assert!(!second.admissions.is_empty());
    assert!(
        outcomes(&second)
            .iter()
            .all(|o| *o == AdmissionOutcome::Demoted),
        "{:?}",
        second.admissions
    );
    assert_eq!(
        second.embedded_units, 0,
        "a demoted class makes no embedding call"
    );

    // Retrieval cites one page-one chunk through the served decision log.
    let cited = first.chunk_ids[0].clone();
    let record_id = committed_executed_retrieval_record(&state).await;
    let learn = Method::DecisionLog {
        op: Box::new(DecisionLogOp::Learn {
            tenant_id: common::TEST_TENANT.to_string(),
            write: LearningWrite::RecordOutcome {
                outcome: Box::new(RetrievalOutcome {
                    record_id,
                    returned: BoundedVec::new(
                        first.chunk_ids[..3]
                            .iter()
                            .map(|id| ReturnedEvidence {
                                evidence_id: id.clone(),
                                content_class: Some("prose".to_string()),
                            })
                            .collect(),
                    )
                    .unwrap(),
                    cited: BoundedVec::new(vec![cited]).unwrap(),
                    query: None,
                    path: None,
                }),
            },
        }),
    };
    call(&state, 310, learn).await;

    // Page three: one citation lifts the demotion; its spans embed again.
    add_document(
        &state,
        6,
        "doc:p3",
        &distinct_page("three", 20),
        "https://example.org/p3",
        NOW,
        "sha256:p3",
    )
    .await;
    let third = enrich_wide(&state, 7, "doc:p3").await;
    assert!(
        third.demoted_classes.is_empty(),
        "{:?}",
        third.demoted_classes
    );
    assert!(
        outcomes(&third)
            .iter()
            .all(|o| *o == AdmissionOutcome::Admitted),
        "{:?}",
        third.admissions
    );
    assert_eq!(third.embedded_units, third.chunk_count);
    assert!(third.chunk_count > 0);
}
