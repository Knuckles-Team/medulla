//! Ontology edit chain records (MEDULLA-TYPED-PACKS-R651.1).

use medulla_types::contract::Digest256;
use medulla_types::ontology_edit::{EditKind, NamingFinding, NamingSeverity, OntologyEdit};

fn digest(seed: &str) -> Digest256 {
    Digest256::sha256(seed.as_bytes())
}

fn round_trip<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json: T = serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap();
    let packed: T = rmp_serde::from_slice(&rmp_serde::to_vec_named(value).unwrap()).unwrap();
    assert_eq!(&json, value);
    assert_eq!(&packed, value);
}

// spec: MEDULLA-TYPED-PACKS-R651.1
#[test]
fn edit_and_finding_round_trip() {
    let edits = [
        (EditKind::Add, None, Some(digest("a")), None, None),
        (
            EditKind::Change,
            Some(digest("a")),
            Some(digest("b")),
            Some(digest("e1")),
            Some("lease-1".to_string()),
        ),
        (
            EditKind::Remove,
            Some(digest("b")),
            None,
            Some(digest("e2")),
            None,
        ),
    ];
    for (index, (kind, before, after, prev, sealed)) in edits.into_iter().enumerate() {
        let edit = OntologyEdit {
            edit_id: format!("edit-{index:08}"),
            kind,
            subject_iri: "http://example.org/onto".to_string(),
            before_digest: before,
            after_digest: after,
            prev_digest: prev,
            sealed_by: sealed,
        };
        round_trip(&edit);
    }
    round_trip(&NamingFinding {
        subject_iri: "http://example.org/onto#bad_name".to_string(),
        rule: "class_pascal_case".to_string(),
        severity: NamingSeverity::Warn,
    });
    let unknown = serde_json::json!({
        "edit_id": "e", "kind": "add", "subject_iri": "s", "before_digest": null,
        "after_digest": null, "prev_digest": null, "sealed_by": null, "extra": 1
    });
    assert!(serde_json::from_value::<OntologyEdit>(unknown).is_err());
    let short = serde_json::json!({"prev_digest": "abc"});
    assert!(serde_json::from_value::<OntologyEdit>(short).is_err());
}
