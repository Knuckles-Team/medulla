//! Glossary rules (MEDULLA-TYPED-PACKS-R801.2).

use medulla_catalog_types::glossary::{GlossaryTerm, GlossaryTermStatus};
use medulla_core::glossary::{
    bind, check_broader_acyclic, label_matches, replacement, GlossaryError, PutScheme,
    DEFINES_ASSET,
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
