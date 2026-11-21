//! The staleness rule of a stored skill binding (MEDULLA-REPO-INGEST-R002.1.3).

use medulla_ingestion_types::skill_plan::skill_binding_stale;
use medulla_types::contract::Digest256;

fn digest(byte: u8) -> Digest256 {
    Digest256::from_bytes([byte; 32])
}

fn stored(document: u8, capability: u8, types: u8) -> serde_json::Value {
    serde_json::json!({
        "document_digest": digest(document).to_hex(),
        "capability_digest": digest(capability).to_hex(),
        "type_digest": digest(types).to_hex(),
    })
}

// spec: MEDULLA-REPO-INGEST-R002.1.3
#[test]
fn binding_staleness_names_the_first_differing_digest() {
    let (d, c, t) = (digest(1), digest(2), digest(3));
    assert_eq!(skill_binding_stale(&stored(1, 2, 3), &d, &c, &t), None);
    assert_eq!(
        skill_binding_stale(&stored(9, 2, 3), &d, &c, &t),
        Some("document_digest")
    );
    assert_eq!(
        skill_binding_stale(&stored(1, 9, 3), &d, &c, &t),
        Some("capability_digest")
    );
    assert_eq!(
        skill_binding_stale(&stored(1, 2, 9), &d, &c, &t),
        Some("type_digest")
    );
    // The first in order wins when several differ.
    assert_eq!(
        skill_binding_stale(&stored(9, 9, 9), &d, &c, &t),
        Some("document_digest")
    );
    assert_eq!(
        skill_binding_stale(&stored(1, 9, 9), &d, &c, &t),
        Some("capability_digest")
    );
    // A node without a property counts as differing.
    let missing = serde_json::json!({
        "document_digest": d.to_hex(),
        "type_digest": t.to_hex(),
    });
    assert_eq!(
        skill_binding_stale(&missing, &d, &c, &t),
        Some("capability_digest")
    );
    assert_eq!(
        skill_binding_stale(&serde_json::json!({}), &d, &c, &t),
        Some("document_digest")
    );
}
