//! Skill graph bundle types (MEDULLA-TYPED-PACKS-R700.1).

use medulla_catalog_types::connector_pack::{
    ConformanceReport, SkillCandidate, SkillCandidateState, SkillGraphLayer, SkillGraphManifest,
};
use serde_json::json;

const HEX: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

// spec: MEDULLA-TYPED-PACKS-R700.1
#[test]
fn manifest_report_candidate_round_trip_and_reject_bad_digest() {
    // The manifest reads sources.json files[] and strips the `sha256:` prefix.
    let sources = json!({"files": [
        {"path": "index.json", "sha256": format!("sha256:{HEX}")},
        {"path": "reference/a.md", "sha256": HEX},
    ]});
    let manifest = SkillGraphManifest::from_sources_json(&sources).unwrap();
    assert_eq!(manifest.entries.len(), 2);
    assert_eq!(manifest.entries[0].sha256.as_str(), HEX);
    let wire = serde_json::to_value(&manifest).unwrap();
    assert_eq!(wire["entries"][0]["sha256"], json!(HEX));
    let back: SkillGraphManifest = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(back, manifest);

    // A bad digest is refused on every read path.
    for bad in [
        HEX.to_uppercase(),
        HEX[..63].to_string(),
        format!("{HEX}0"),
        "g".repeat(64),
    ] {
        let mut value = wire.clone();
        value["entries"][0]["sha256"] = json!(bad);
        assert!(serde_json::from_value::<SkillGraphManifest>(value).is_err());
        let from_sources = json!({"files": [{"path": "p", "sha256": format!("sha256:{bad}")}]});
        assert!(SkillGraphManifest::from_sources_json(&from_sources).is_err());
    }
    assert!(SkillGraphManifest::from_sources_json(&json!({})).is_err());
    assert!(SkillGraphManifest::from_sources_json(&json!({"files": [{"sha256": HEX}]})).is_err());

    // Unknown fields are refused.
    let mut extra = wire;
    extra["surprise"] = json!(1);
    assert!(serde_json::from_value::<SkillGraphManifest>(extra).is_err());

    // The report round trips and names its violations.
    let report = ConformanceReport {
        checked_rules: vec!["path_exists".into(), "hash_matches".into()],
        violations: vec!["reference/a.md: hash mismatch".into()],
    };
    let wire = serde_json::to_value(&report).unwrap();
    let back: ConformanceReport = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(back, report);
    assert!(!back.is_conformant());
    let mut extra = wire;
    extra["surprise"] = json!(1);
    assert!(serde_json::from_value::<ConformanceReport>(extra).is_err());

    // A candidate is `proposed` only.
    let candidate = SkillCandidate {
        candidate_id: "c-1".into(),
        source_process: "p-1".into(),
        state: SkillCandidateState::Proposed,
    };
    let wire = serde_json::to_value(&candidate).unwrap();
    assert_eq!(wire["state"], json!("proposed"));
    let back: SkillCandidate = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(back, candidate);
    let mut promoted = wire;
    promoted["state"] = json!("published");
    assert!(serde_json::from_value::<SkillCandidate>(promoted).is_err());
}

// spec: MEDULLA-TYPED-PACKS-R701.1
#[test]
fn layer_round_trip_and_reject_foreign_iri() {
    let iri = "https://knuckles-team.github.io/ontology/skillgraph/billing";
    let layer = SkillGraphLayer::new("billing", iri, 12).unwrap();
    let wire = serde_json::to_value(&layer).unwrap();
    assert_eq!(wire["ontology_iri"], json!(iri));
    let back: SkillGraphLayer = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(back, layer);

    // An IRI outside the layer namespace, or for another graph, is refused on every path.
    for bad in [
        "https://example.test/skillgraph/billing",
        "https://knuckles-team.github.io/ontology/skillgraph/other",
    ] {
        assert!(SkillGraphLayer::new("billing", bad, 1).is_err());
        let mut value = wire.clone();
        value["ontology_iri"] = json!(bad);
        assert!(serde_json::from_value::<SkillGraphLayer>(value).is_err());
    }
    // The class count is bounded and unknown fields are refused.
    assert!(SkillGraphLayer::new("billing", iri, 100_000).is_ok());
    assert!(SkillGraphLayer::new("billing", iri, 100_001).is_err());
    let mut extra = wire;
    extra["surprise"] = json!(1);
    assert!(serde_json::from_value::<SkillGraphLayer>(extra).is_err());
}
