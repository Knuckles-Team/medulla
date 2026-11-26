//! Glossary rules (MEDULLA-TYPED-PACKS-R801.2).

use medulla_catalog_types::glossary::{GlossaryTerm, GlossaryTermStatus};
use medulla_core::glossary::{
    bind, check_broader_acyclic, label_matches, replacement, GlossaryError, GlossaryLabelIndex,
    PutScheme, DEFINES_ASSET,
};

const SCHEME: &str = "http://example.org/scheme/finance";

fn term(pref: &str, alts: &[&str], broader: &[&str], status: GlossaryTermStatus) -> GlossaryTerm {
    GlossaryTerm {
        scheme: SCHEME.into(),
        pref_label: pref.into(),
        alt_labels: alts.iter().map(|s| s.to_string()).collect(),
        definition: "definition".into(),
        broader: broader.iter().map(|s| s.to_string()).collect(),
        related: vec![],
        status,
        replaced_by: None,
    }
}

// spec: MEDULLA-TYPED-PACKS-R801.2
#[test]
fn alt_label_search_finds_term_and_deprecated_names_replacement() {
    let net = term(
        "Net revenue",
        &["Net sales", "Chiffre d'affaires net"],
        &[],
        GlossaryTermStatus::Approved,
    );
    assert!(label_matches(&net, "net revenue"));
    assert!(label_matches(&net, "SALES"));
    assert!(label_matches(&net, "chiffre"));
    assert!(!label_matches(&net, "margin"));
    assert!(!label_matches(&net, "  "));

    let link = bind("http://example.org/term/net", &net, "http://example.org/asset/orders")
        .expect("an approved term binds");
    assert_eq!(link.predicate, DEFINES_ASSET);
    assert_eq!(link.asset, "http://example.org/asset/orders");

    let mut old = term("Sales", &[], &[], GlossaryTermStatus::Deprecated);
    old.replaced_by = Some("http://example.org/term/net".into());
    assert_eq!(replacement(&old), Some("http://example.org/term/net"));
    assert_eq!(replacement(&net), None);
    assert_eq!(
        bind("http://example.org/term/sales", &old, "http://example.org/asset/orders"),
        Err(GlossaryError::Deprecated {
            replaced_by: Some("http://example.org/term/net".into())
        })
    );

    let a = "http://example.org/term/a";
    let b = "http://example.org/term/b";
    let c = "http://example.org/term/c";
    let chain = vec![
        (a.to_string(), term("A", &[], &[b], GlossaryTermStatus::Approved)),
        (b.to_string(), term("B", &[], &[c], GlossaryTermStatus::Approved)),
        (c.to_string(), term("C", &[], &[], GlossaryTermStatus::Approved)),
    ];
    assert_eq!(check_broader_acyclic(&chain, 64), Ok(()));
    let mut cyclic = chain.clone();
    cyclic[2].1.broader = vec![a.to_string()];
    assert!(matches!(
        check_broader_acyclic(&cyclic, 64),
        Err(GlossaryError::BroaderCycle(_))
    ));
    assert!(matches!(
        check_broader_acyclic(&chain, 1),
        Err(GlossaryError::BroaderTooDeep { .. })
    ));

    let put = PutScheme {
        scheme: SCHEME.into(),
        terms: chain,
    };
    assert_eq!(put.validate(), Ok(()));
    let mut wrong = put.clone();
    wrong.terms[0].1.scheme = "http://example.org/scheme/other".into();
    assert!(matches!(
        wrong.validate(),
        Err(GlossaryError::SchemeMismatch { .. })
    ));
}

// spec: MEDULLA-TYPED-PACKS-R801.4
#[test]
fn cycle_taken_label_and_missing_review_carry_their_codes() {
    use medulla_catalog_types::stewardship::{ChangeRequest, ChangeRequestState};
    use medulla_core::glossary::{check_broader_acyclic_over, check_label_free, require_review};
    use medulla_types::contract::ServerErrorCode;

    let stored = vec![
        ("t:a".to_string(), term("A", &[], &["t:b"], GlossaryTermStatus::Draft)),
        ("t:b".to_string(), term("B", &[], &[], GlossaryTermStatus::Approved)),
    ];
    // A new `b` that points back at `a` closes a cycle through the stored term.
    let cycle = check_broader_acyclic_over(
        stored.clone(),
        &[("t:b".to_string(), term("B", &[], &["t:a"], GlossaryTermStatus::Draft))],
    )
    .expect_err("a cycle through a stored term is refused");
    assert_eq!(cycle.code(), Some(ServerErrorCode::GlossaryCycle));

    let taken = check_label_free(
        &stored,
        "t:c",
        &term(" b ", &[], &[], GlossaryTermStatus::InReview),
    )
    .expect_err("an approved term already has the label");
    assert_eq!(taken.code(), Some(ServerErrorCode::GlossaryLabelTaken));
    assert!(check_label_free(&stored, "t:b", &stored[1].1).is_ok());

    let request = |state| ChangeRequest {
        target: "t:a".into(),
        proposed_digest: "sha256:x".into(),
        state,
        reviewers: vec!["steward".into()],
    };
    for missing in [None, Some(request(ChangeRequestState::Open))] {
        let refused = require_review("t:a", missing.as_ref()).expect_err("no approved request");
        assert_eq!(refused.code(), Some(ServerErrorCode::GlossaryUnreviewed));
    }
    assert!(require_review("t:a", Some(&request(ChangeRequestState::Approved))).is_ok());
    assert!(require_review("t:other", Some(&request(ChangeRequestState::Approved))).is_err());
}

// spec: MEDULLA-TYPED-PACKS-R801.6
#[test]
fn the_label_index_answers_exactly_what_a_scan_of_label_matches_answers() {
    let terms = vec![
        term("Net revenue", &["Net sales", "Chiffre d'affaires net"], &[], GlossaryTermStatus::Approved),
        term("Gross margin", &["Bruto margen"], &[], GlossaryTermStatus::Approved),
        term("Net margin", &["Neto margen", "Net profit"], &[], GlossaryTermStatus::Draft),
        term("Revenue growth", &[], &[], GlossaryTermStatus::Approved),
    ];
    let index = GlossaryLabelIndex::build(&terms);
    for query in [
        "net",
        "net sales",
        "net margin",
        "revenue net",
        "REV",
        "margen",
        "chiffre",
        "profit net",
        "growth rev",
        "absent",
        "  ",
        "",
    ] {
        let scanned: Vec<usize> = terms
            .iter()
            .enumerate()
            .filter(|(_, term)| label_matches(term, query))
            .map(|(position, _)| position)
            .collect();
        assert_eq!(index.search(&terms, query), scanned, "query {query:?}");
    }
    // A query word split across two labels of one term does not match, as in the scan.
    assert!(index.search(&terms, "sales margin").is_empty());
    // An index used over another term list answers nothing rather than a wrong position.
    assert!(index.search(&terms[..2], "net").is_empty());
}
