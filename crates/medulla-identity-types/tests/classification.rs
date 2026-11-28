//! Classification claim model (MEDULLA-TYPED-PACKS-R652.1).

use medulla_identity_core_types::tenancy::Marking;
use medulla_identity_types::classification::{
    ClaimKind, ClaimProvenance, ClaimState, ClassificationClaim,
};

fn claim(kind: ClaimKind) -> ClassificationClaim {
    let detector = kind == ClaimKind::Detector;
    ClassificationClaim {
        asset: "node:orders.card".to_string(),
        categories: vec![Marking::new("pci").unwrap()],
        provenance: ClaimProvenance {
            kind,
            detector_id: detector.then(|| "card-number".to_string()),
            sample_size: detector.then_some(200),
            hit_ratio: detector.then_some(0.5),
            scanned_at_ms: detector.then_some(1_700_000_000_000),
        },
        state: ClaimState::Proposed,
        evidence: vec!["scan:1".to_string()],
    }
}

// spec: MEDULLA-TYPED-PACKS-R652.1
#[test]
fn claim_round_trips_and_rejects_empty_categories() {
    for kind in [
        ClaimKind::Detector,
        ClaimKind::Propagated,
        ClaimKind::Manual,
    ] {
        let original = claim(kind);
        original.validate().expect("a well-formed claim is valid");
        let wire = serde_json::to_string(&original).unwrap();
        let back: ClassificationClaim = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, original);
    }
    let wire = serde_json::to_value(claim(ClaimKind::Manual)).unwrap();
    assert_eq!(wire["provenance"]["kind"], "manual");
    assert_eq!(wire["state"], "proposed");
    assert!(wire["provenance"].get("detector_id").is_none());

    let mut empty = claim(ClaimKind::Manual);
    empty.categories.clear();
    assert!(empty
        .validate()
        .unwrap_err()
        .starts_with("INVALID_ARGUMENT"));

    // Detector fields belong to a detector claim only, and a detector claim sets them all.
    let mut stray = claim(ClaimKind::Manual);
    stray.provenance.detector_id = Some("card-number".to_string());
    assert!(stray.validate().is_err());
    let mut partial = claim(ClaimKind::Detector);
    partial.provenance.sample_size = None;
    assert!(partial.validate().is_err());
    let mut out_of_range = claim(ClaimKind::Detector);
    out_of_range.provenance.hit_ratio = Some(1.5);
    assert!(out_of_range.validate().is_err());

    // An unregistered state or kind does not deserialize.
    let bad = serde_json::json!({
        "asset": "a", "categories": ["pci"], "state": "approved",
        "provenance": {"kind": "manual"}
    });
    assert!(serde_json::from_value::<ClassificationClaim>(bad).is_err());
}
