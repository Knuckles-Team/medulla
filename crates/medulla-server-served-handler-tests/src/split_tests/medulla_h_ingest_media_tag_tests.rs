// MEDULLA-MEDIA-VOCAB-R001.100.2: the wire contract of the media tag feature of `SourceIngest`.
// The served `prepare` cases (provenance and stale-revision refusals, replay) need a connector
// pack harness and are listed under `## wave-4b gaps` in the spec gaps file.

use medulla_source_types::source_ingestion::{fixtures, SourceIngestionRequest};
use medulla_types::media_tag::{MediaTagValue, TagKind, REFUSALS};
use serde_json::json;

fn tag_value() -> MediaTagValue {
    MediaTagValue {
        label: "dog".into(),
        origin: "model".into(),
        taxonomy_id: "tax-1".into(),
        taxonomy_version: "1".into(),
        tag_kind: TagKind::Object,
        confidence: 0.9,
        producer_id: "producer-1".into(),
        evidence_ref: "evidence-1".into(),
        media_revision: 3,
    }
}

fn feature(index: usize) -> serde_json::Value {
    json!({
        "feature_id": format!("feature-{index}"),
        "object_id": "media-1",
        "kind": "media_tag",
        "value_msgpack": rmp_serde::to_vec_named(&tag_value()).unwrap(),
        "model_version": "v1",
    })
}

fn batch_with(features: Vec<serde_json::Value>) -> serde_json::Value {
    let mut batch = serde_json::to_value(fixtures::request().as_batch()).unwrap();
    batch["features"] = serde_json::Value::Array(features);
    batch
}

// spec: MEDULLA-MEDIA-VOCAB-R001.100.2
#[test]
fn tag_ingest_wire_contract() {
    // A batch without features keeps its canonical bytes and digest.
    let plain = fixtures::request();
    let value = serde_json::to_value(plain.as_batch()).unwrap();
    assert!(value.get("features").is_none());
    let decoded: SourceIngestionRequest = serde_json::from_value(value).unwrap();
    assert_eq!(
        decoded.batch_digest().unwrap(),
        plain.batch_digest().unwrap()
    );

    // A tag feature decodes and changes the digest (the digest covers `features`).
    let tagged: SourceIngestionRequest =
        serde_json::from_value(batch_with(vec![feature(0)])).unwrap();
    assert_ne!(
        tagged.batch_digest().unwrap(),
        plain.batch_digest().unwrap()
    );
    let stored: MediaTagValue =
        rmp_serde::from_slice(&tagged.as_batch().features.as_slice()[0].value_msgpack).unwrap();
    assert_eq!(stored, tag_value());

    // 1025 features are refused at decode.
    let refusal = serde_json::from_value::<SourceIngestionRequest>(batch_with(
        (0..1025).map(feature).collect(),
    ))
    .unwrap_err()
    .to_string();
    assert!(
        refusal.contains("collection exceeds 1024 items"),
        "{refusal}"
    );

    // A duplicate feature id and an unsupported kind are refused.
    let duplicate =
        serde_json::from_value::<SourceIngestionRequest>(batch_with(vec![feature(0), feature(0)]))
            .unwrap_err()
            .to_string();
    assert!(
        duplicate.contains("duplicate source feature"),
        "{duplicate}"
    );
    let mut other = feature(1);
    other["kind"] = json!("embedding");
    let unsupported = serde_json::from_value::<SourceIngestionRequest>(batch_with(vec![other]))
        .unwrap_err()
        .to_string();
    assert!(
        unsupported.contains("source feature kind is not supported"),
        "{unsupported}"
    );

    // The two refusal codes of the tag ingest.
    let codes: Vec<&str> = REFUSALS.iter().map(|code| code.as_str()).collect();
    assert_eq!(
        codes,
        ["TAG_PROVENANCE_MISSING", "TAG_MEDIA_REVISION_STALE"]
    );
}
