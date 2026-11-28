// MEDULLA-MEDIA-VOCAB-R002.5.1 to R002.5.5: the served journeys of paper ingest. Paper records
// run through `SourceIngest` of a connector pack whose `paper` stream maps to `ResearchPaper`;
// the receipt's `papers` report, the stored nodes and the `cites` edges are read back as a
// `kg:read` reader.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use medulla_served_test_harness::connector_pack_harness::{Served, GRAPH, OTHER_TENANT, TENANT};

const CONNECTOR: &str = "paper-source";
const STREAM: &str = "paper";

async fn served() -> Served {
    Served::provisioned(GRAPH, CONNECTOR, &[(STREAM, "ResearchPaper")]).await
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The node id of the paper with `canonical_id`.
fn node_id(canonical_id: &str) -> String {
    format!("paper:{}", sha256_hex(canonical_id.as_bytes()))
}

/// The `id` of a paper: the SHA-256 of the compact JSON `[canonical_id, revision]`.
fn identity(canonical_id: &str, revision: Option<&str>) -> String {
    sha256_hex(
        serde_json::to_string(&json!([canonical_id, revision]))
            .unwrap()
            .as_bytes(),
    )
}

fn provenance(field: &str) -> Value {
    json!([{"provider_id": "crossref", "source_id": format!("{field}-1"), "location": field}])
}

/// A valid paper payload with every cited field provenanced.
fn paper(canonical_id: &str) -> Value {
    json!({
        "id": identity(canonical_id, None),
        "canonical_id": canonical_id,
        "title": "A Study",
        "authors": ["Ada", "Grace"],
        "abstract": "What was found.",
        "publication_date": "2026-01-02",
        "references": [
            {"provider_id": "crossref", "record_id": "10.1234/abc", "original_id": "https://doi.org/10.1234/ABC"},
            {"provider_id": "arxiv", "record_id": "arXiv:2101.00001"},
        ],
        "field_provenance": {
            "title": provenance("title"),
            "authors": provenance("authors"),
            "abstract": provenance("abstract"),
            "publication_date": provenance("publication_date"),
        },
    })
}

fn receipt(canonical_id: &str, tenant: &str, sha: &str) -> Value {
    json!({
        "tenant": tenant,
        "paper_id": canonical_id,
        "source_url": "https://example.org/paper.pdf",
        "media_type": "application/pdf",
        "byte_count": 1024,
        "sha256": sha,
        "artifact_ref": "artifacts/paper.pdf",
    })
}

fn refusals(
    receipt: &medulla_source_types::source_ingestion::SourceIngestionReceipt,
) -> Vec<(String, String)> {
    receipt
        .papers
        .refusals
        .iter()
        .map(|refusal| (refusal.record_id.clone(), refusal.code.clone()))
        .collect()
}

// spec: MEDULLA-MEDIA-VOCAB-R002.5.1
#[tokio::test]
async fn mixed_result_refuses_only_the_unidentified_paper() {
    let served = served().await;
    let mut unidentified = paper("doi:10.1234/bad");
    unidentified["id"] = json!(identity("doi:10.1234/other", None));
    let accepted = served
        .ingest(
            STREAM,
            &[("good", paper("doi:10.1234/good")), ("bad", unidentified)],
        )
        .await;
    // One commit: the valid paper is stored, the unidentified one is reported and absent.
    assert_eq!(
        refusals(&accepted),
        vec![("bad".to_string(), "PAPER_IDENTITY_INVALID".to_string())]
    );
    assert!(served
        .properties(&node_id("doi:10.1234/good"))
        .await
        .is_some());
    assert!(served
        .properties(&node_id("doi:10.1234/bad"))
        .await
        .is_none());

    // A receipt of another tenant refuses only that paper.
    let sha = sha256_hex(b"document");
    let mut foreign = paper("doi:10.1234/foreign");
    foreign["document_receipt"] = receipt("doi:10.1234/foreign", OTHER_TENANT, &sha);
    let mut own = paper("doi:10.1234/own");
    own["document_receipt"] = receipt("doi:10.1234/own", TENANT, &sha);
    let accepted = served
        .ingest(STREAM, &[("foreign", foreign), ("own", own)])
        .await;
    assert_eq!(
        refusals(&accepted),
        vec![("foreign".to_string(), "PAPER_RECEIPT_MISMATCH".to_string())]
    );
    assert!(served
        .properties(&node_id("doi:10.1234/foreign"))
        .await
        .is_none());
    assert!(served
        .properties(&node_id("doi:10.1234/own"))
        .await
        .is_some());
}

// spec: MEDULLA-MEDIA-VOCAB-R002.5.2
#[tokio::test]
async fn receipt_bound_paper_facts_round_trip() {
    let served = served().await;
    let canonical_id = "doi:10.1234/facts";
    let document = sha256_hex(b"the document");
    let mut payload = paper(canonical_id);
    payload["document_receipt"] = receipt(canonical_id, TENANT, &document);
    payload["field_provenance"]["title"] = json!([
        {"provider_id": "crossref", "source_id": "title-1", "location": "title", "document_sha256": document}
    ]);
    let accepted = served.ingest(STREAM, &[("facts", payload.clone())]).await;
    assert!(refusals(&accepted).is_empty(), "{:?}", accepted.papers);

    let read = |properties: &serde_json::Map<String, Value>, key: &str| -> String {
        properties
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("the paper has no `{key}`"))
            .to_string()
    };
    let properties = served
        .properties(&node_id(canonical_id))
        .await
        .expect("the paper node");
    // The facts.
    assert_eq!(read(&properties, "type"), "ResearchPaper");
    assert_eq!(read(&properties, "canonical_id"), canonical_id);
    assert_eq!(read(&properties, "title"), "A Study");
    assert_eq!(read(&properties, "authors"), r#"["Ada","Grace"]"#);
    assert_eq!(read(&properties, "abstract"), "What was found.");
    assert_eq!(read(&properties, "publishedAt"), "2026-01-02");
    assert_eq!(read(&properties, "doi"), "10.1234/abc");
    assert_eq!(read(&properties, "arxivId"), "2101.00001");
    assert!(properties.get("citationCount").is_none());
    // The receipt digest and the document digest, read separately from the facts.
    let sorted: BTreeMap<String, Value> = payload["document_receipt"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    assert_eq!(
        read(&properties, "receipt_digest"),
        sha256_hex(serde_json::to_string(&sorted).unwrap().as_bytes())
    );
    assert_eq!(read(&properties, "document_digest"), document);
    // The field provenance: one entry per element, the document-bound one by digest.
    let entries: Vec<Value> = serde_json::from_str(&read(&properties, "provenance")).unwrap();
    assert_eq!(entries.len(), 4);
    let title = entries
        .iter()
        .find(|entry| entry["field"] == "title")
        .expect("a title provenance entry");
    assert_eq!(
        title["provider_ref_or_document_digest"],
        json!(format!("sha256:{document}"))
    );
    assert_eq!(title["extraction_location"], "title");
    let authors = entries
        .iter()
        .find(|entry| entry["field"] == "authors")
        .expect("an authors provenance entry");
    assert_eq!(
        authors["provider_ref_or_document_digest"],
        "crossref:authors-1"
    );

    // Replay of the same identity, revision and digest is idempotent.
    let replay = served.ingest(STREAM, &[("facts", payload)]).await;
    assert!(refusals(&replay).is_empty());
    let replayed = served
        .properties(&node_id(canonical_id))
        .await
        .expect("the paper node");
    assert_eq!(replayed, properties);
    assert_eq!(
        served.uql_ids("MATCH (:ResearchPaper)").await,
        vec![node_id(canonical_id)]
    );
}

// spec: MEDULLA-MEDIA-VOCAB-R002.5.3
#[tokio::test]
async fn count_only_record_creates_no_edge() {
    let served = served().await;
    let mut citing = paper("doi:10.1234/citing");
    citing["citation_edges"] = json!([
        {"cited_canonical_id": "doi:10.1234/cited", "source_evidence": [
            {"provider_id": "crossref", "source_id": "ref-1", "location": "references/1"}]},
        {"cited_canonical_id": "unidentified:".to_string() + &"0".repeat(64), "source_evidence": []},
        {"cited_canonical_id": "  ", "source_evidence": []},
    ]);
    let mut counted = paper("doi:10.1234/counted");
    counted["citation_count"] = json!(42);
    let accepted = served
        .ingest(STREAM, &[("citing", citing), ("counted", counted)])
        .await;
    assert!(refusals(&accepted).is_empty());

    // One edge for the one identified declaration, from the citing paper; none from the count.
    let edges = served.uql_ids("EDGES :cites").await;
    assert_eq!(edges.len(), 1, "{edges:?}");
    assert!(edges[0].starts_with(&format!(
        "{}|{}|",
        node_id("doi:10.1234/citing"),
        node_id("doi:10.1234/cited")
    )));
    // The cited paper is a stub; a full record for it overwrites the stub without the marker.
    let stub = served
        .properties(&node_id("doi:10.1234/cited"))
        .await
        .expect("the stub");
    assert_eq!(stub.get("stub"), Some(&json!(true)));
    assert_eq!(stub.get("type"), Some(&json!("ResearchPaper")));
    let accepted = served
        .ingest(STREAM, &[("cited", paper("doi:10.1234/cited"))])
        .await;
    assert!(refusals(&accepted).is_empty());
    let full = served
        .properties(&node_id("doi:10.1234/cited"))
        .await
        .expect("the full paper");
    assert!(full.get("stub").is_none());
    assert_eq!(full.get("title"), Some(&json!("A Study")));
    assert_eq!(served.uql_ids("EDGES :cites").await.len(), 1);
}

// spec: MEDULLA-MEDIA-VOCAB-R002.5.5
#[tokio::test]
async fn paper_refusals_leave_store_unchanged() {
    let served = served().await;
    let sha = sha256_hex(b"document");
    let mut identity_invalid = paper("doi:10.1234/identity");
    identity_invalid["references"] = json!([]);
    let mut receipt_mismatch = paper("doi:10.1234/receipt");
    receipt_mismatch["document_receipt"] = receipt("doi:10.1234/other", TENANT, &sha);
    let mut artifact_absolute = paper("doi:10.1234/absolute");
    artifact_absolute["document_receipt"] = receipt("doi:10.1234/absolute", TENANT, &sha);
    artifact_absolute["document_receipt"]["artifact_ref"] = json!("/etc/paper.pdf");
    let mut artifact_other_document = paper("doi:10.1234/document");
    artifact_other_document["field_provenance"]["title"] = json!([
        {"provider_id": "crossref", "source_id": "title-1", "location": "title",
         "document_sha256": sha256_hex(b"another document")}
    ]);
    let mut provenance_missing = paper("doi:10.1234/provenance");
    provenance_missing["field_provenance"]["abstract"] = json!([]);

    let accepted = served
        .ingest(
            STREAM,
            &[
                ("identity", identity_invalid),
                ("receipt", receipt_mismatch),
                ("absolute", artifact_absolute),
                ("document", artifact_other_document),
                ("provenance", provenance_missing),
            ],
        )
        .await;
    assert_eq!(
        refusals(&accepted),
        vec![
            ("identity".to_string(), "PAPER_IDENTITY_INVALID".to_string()),
            ("receipt".to_string(), "PAPER_RECEIPT_MISMATCH".to_string()),
            (
                "absolute".to_string(),
                "PAPER_ARTIFACT_UNAVAILABLE".to_string()
            ),
            (
                "document".to_string(),
                "PAPER_ARTIFACT_UNAVAILABLE".to_string()
            ),
            (
                "provenance".to_string(),
                "PAPER_FIELD_PROVENANCE_MISSING".to_string()
            ),
        ]
    );
    // Every refusal happened before the paper's nodes were added: the store holds no paper.
    for canonical_id in ["identity", "receipt", "absolute", "document", "provenance"] {
        assert!(
            served
                .properties(&node_id(&format!("doi:10.1234/{canonical_id}")))
                .await
                .is_none(),
            "{canonical_id} left a node"
        );
    }
    assert!(served.uql_ids("MATCH (:ResearchPaper)").await.is_empty());
    assert!(served.uql_ids("EDGES :cites").await.is_empty());
}
