// MEDULLA-MEDIA-VOCAB-R650.3: the wire contract of the media derivation feature of `SourceIngest`.
// The served remember-and-recall journey over a connector pack harness is listed under
// `## wave-6 gaps` in the spec gaps file; the admission and recall rules are proved in
// `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs`.

use medulla_source_types::source_ingestion::{fixtures, SourceIngestionRequest};
use medulla_types::media_derivation::MediaDerivationValue;
use serde_json::json;

fn derivation_feature() -> serde_json::Value {
    json!({
        "feature_id": "feature-caption",
        "object_id": "media-1",
        "kind": "media_derivation",
        "value_msgpack": rmp_serde::to_vec_named(&MediaDerivationValue {
            kind: "caption".into(),
            text: "a dog on a beach".into(),
            embedding: vec![1.0, 0.0],
        })
        .unwrap(),
        "model_version": "captioner-1",
    })
}

// spec: MEDULLA-MEDIA-VOCAB-R650.3
#[test]
fn media_derivation_feature_wire_contract() {
    let plain = fixtures::request();
    let mut batch = serde_json::to_value(plain.as_batch()).unwrap();
    batch["features"] = json!([derivation_feature()]);
    let request: SourceIngestionRequest = serde_json::from_value(batch).unwrap();
    assert_ne!(request.batch_digest().unwrap(), plain.batch_digest().unwrap());
    let stored: MediaDerivationValue =
        rmp_serde::from_slice(&request.as_batch().features.as_slice()[0].value_msgpack).unwrap();
    assert_eq!(stored.kind, "caption");
    assert_eq!(stored.embedding, vec![1.0, 0.0]);
}
