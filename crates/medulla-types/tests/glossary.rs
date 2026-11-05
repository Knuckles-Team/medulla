//! Business glossary term model (MEDULLA-TYPED-PACKS-R801.1).

use medulla_types::glossary::{
    GlossaryTerm, GlossaryTermStatus, SkosObject, SKOS_ALT_LABEL, SKOS_BROADER, SKOS_DEFINITION,
    SKOS_PREF_LABEL, SKOS_RELATED,
};

// spec: MEDULLA-TYPED-PACKS-R801.1
#[test]
fn term_round_trips_and_maps_to_skos() {
    let term = GlossaryTerm {
        scheme: "http://example.org/scheme/finance".into(),
        pref_label: "Net revenue".into(),
        alt_labels: vec!["Net sales".into()],
        definition: "Revenue after refunds and discounts.".into(),
        broader: vec!["http://example.org/term/revenue".into()],
        related: vec!["http://example.org/term/refund".into()],
        status: GlossaryTermStatus::InReview,
    };
    let json = serde_json::to_value(&term).unwrap();
    assert_eq!(json["status"], "in_review");
    let back: GlossaryTerm = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(back, term);
    let packed: GlossaryTerm =
        rmp_serde::from_slice(&rmp_serde::to_vec_named(&term).unwrap()).unwrap();
    assert_eq!(packed, term);

    for (status, wire) in [
        (GlossaryTermStatus::Draft, "draft"),
        (GlossaryTermStatus::Approved, "approved"),
        (GlossaryTermStatus::Deprecated, "deprecated"),
    ] {
        assert_eq!(serde_json::to_value(status).unwrap(), wire);
    }

    let mut unknown = json;
    unknown["extra"] = serde_json::json!(1);
    assert!(serde_json::from_value::<GlossaryTerm>(unknown).is_err());

    let pairs = term.skos_pairs();
    let has = |p: &str, o: SkosObject| pairs.iter().any(|(k, v)| *k == p && *v == o);
    assert!(has(
        SKOS_PREF_LABEL,
        SkosObject::Literal("Net revenue".into())
    ));
    assert!(has(SKOS_ALT_LABEL, SkosObject::Literal("Net sales".into())));
    assert!(has(
        SKOS_DEFINITION,
        SkosObject::Literal("Revenue after refunds and discounts.".into())
    ));
    assert!(has(
        SKOS_BROADER,
        SkosObject::Iri("http://example.org/term/revenue".into())
    ));
    assert!(has(
        SKOS_RELATED,
        SkosObject::Iri("http://example.org/term/refund".into())
    ));
}
