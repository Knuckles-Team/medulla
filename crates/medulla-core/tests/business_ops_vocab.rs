//! Contract tests for the business operations pack and its shapes companion
//! (MEDULLA-BUSINESS-OPS-R001.1 to R001.8).

use std::collections::BTreeSet;

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_rdf::mapping::parse_turtle;
use medulla_rdf::oxrdf::{NamedNode, NamedOrBlankNode, Term, Triple};
use medulla_shacl::validate_turtle;

const KG: &str = "https://knuckles-team.github.io/ontology/core#";
const OWL: &str = "http://www.w3.org/2002/07/owl#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const PREFIXES: &str = "@prefix : <https://knuckles-team.github.io/ontology/core#> .\n\
                        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
                        @prefix ex: <http://example.org/business/> .\n";

const ONTOLOGY: &str = include_str!("../ontology/business_ops-v1.ttl");
const SHAPES: &str = include_str!("../ontology/business_ops-v1.shapes.ttl");

fn triples() -> Vec<Triple> {
    parse_turtle(ONTOLOGY).expect("the business operations ontology parses")
}

fn has_iri(triples: &[Triple], subject: &str, predicate: &str, object: &str) -> bool {
    triples.contains(&Triple::new(
        NamedNode::new(subject).unwrap(),
        NamedNode::new(predicate).unwrap(),
        NamedNode::new(object).unwrap(),
    ))
}

fn subject_str(node: &NamedOrBlankNode) -> Option<&str> {
    match node {
        NamedOrBlankNode::NamedNode(iri) => Some(iri.as_str()),
        _ => None,
    }
}

fn object_iri(term: &Term) -> Option<&str> {
    match term {
        Term::NamedNode(node) => Some(node.as_str()),
        _ => None,
    }
}

/// The subjects of one document that are declared `owl:Class`.
fn declared_classes(triples: &[Triple]) -> BTreeSet<String> {
    triples
        .iter()
        .filter(|t| {
            t.predicate.as_str() == format!("{RDF}type")
                && object_iri(&t.object) == Some(&format!("{OWL}Class"))
        })
        .filter_map(|t| subject_str(&t.subject).map(str::to_string))
        .collect()
}

fn assert_class(triples: &[Triple], local: &str) {
    assert!(
        has_iri(
            triples,
            &format!("{KG}{local}"),
            &format!("{RDF}type"),
            &format!("{OWL}Class")
        ),
        "{local} is a declared class"
    );
}

fn assert_parent(triples: &[Triple], local: &str, parent: &str) {
    assert!(
        has_iri(
            triples,
            &format!("{KG}{local}"),
            &format!("{RDFS}subClassOf"),
            &format!("{KG}{parent}")
        ),
        "{local} is a subclass of {parent}"
    );
}

/// Validates `data` (turtle terms under the `:` and `ex:` prefixes) against the shapes.
fn conforms(data: &str) -> (bool, Vec<String>) {
    let report = validate_turtle(SHAPES, &format!("{PREFIXES}{data}"))
        .expect("the data parses before validation");
    let paths = report
        .results
        .iter()
        .map(|result| result.path.clone().unwrap_or_default())
        .collect();
    (report.conforms, paths)
}

// spec: MEDULLA-BUSINESS-OPS-R001.1
#[test]
fn profile_and_governance_terms_with_shapes() {
    // The module loads through the core catalog under its one canonical id.
    let sources = GraphSchemaSources::default();
    sources.validate().expect("the core catalog validates");
    assert_eq!(
        sources
            .core
            .keys()
            .filter(|id| id.starts_with("core:business_ops@"))
            .count(),
        1,
        "business_ops is registered exactly once"
    );
    let triples = triples();
    for class in [
        "BusinessProfile",
        "Brand",
        "RoleAssignment",
        "ApprovalRule",
        "ClaimPolicy",
        "ApprovedPhrase",
        "ForbiddenPhrase",
    ] {
        assert_class(&triples, class);
    }
    assert_parent(&triples, "BusinessProfile", "LegalEntity");
    assert_parent(&triples, "ClaimPolicy", "Policy");
    assert_parent(&triples, "ApprovalRule", "GovernanceRule");

    // A profile and a rule that carry their required terms conform.
    let good = "ex:p a :BusinessProfile ; rdfs:label \"Acme\" .\n\
                ex:r a :ApprovalRule ; rdfs:label \"Refunds\" ; \
                  :approvalCeilingMinor 50000 ; :approvalCurrency \"USD\" ; \
                  :approvalQuorum 1 ; :approvalActionKind \"refund\" .\n";
    assert_eq!(conforms(good), (true, Vec::new()));

    // A profile with no label fails.
    let (ok, _) = conforms("ex:p a :BusinessProfile .\n");
    assert!(!ok, "a profile without a label is refused");

    // A rule with a ceiling that is not an integer, or no currency, fails.
    let (ok, paths) = conforms(
        "ex:r a :ApprovalRule ; rdfs:label \"Refunds\" ; \
         :approvalCeilingMinor 500.5 ; :approvalCurrency \"USD\" .\n",
    );
    assert!(!ok, "a non-integer ceiling is refused");
    assert!(paths.iter().any(|path| path.ends_with("approvalCeilingMinor>")));
    let (ok, paths) = conforms(
        "ex:r a :ApprovalRule ; rdfs:label \"Refunds\" ; :approvalCeilingMinor 500 .\n",
    );
    assert!(!ok, "a rule with no currency is refused");
    assert!(paths.iter().any(|path| path.ends_with("approvalCurrency>")));
}

// spec: MEDULLA-BUSINESS-OPS-R001.3
#[test]
fn money_terms_refuse_floats_and_overrefunds() {
    let triples = triples();
    for class in ["OrderLine", "Payment", "Refund", "Payout", "Fee"] {
        assert_class(&triples, class);
    }
    // The core :Order is reused, never redeclared.
    assert!(!declared_classes(&triples).contains(&format!("{KG}Order")));
    assert!(!declared_classes(&triples).contains(&format!("{KG}Customer")));

    let good = "ex:order a :Order .\n\
                ex:line a :OrderLine ; :lineOfOrder ex:order ; :lineQuantity 2 ; \
                  :unitPriceMinor 1999 ; :taxMinor 160 .\n\
                ex:pay a :Payment ; :paymentOfOrder ex:order ; :amountMinor 4158 ; \
                  :amountCurrency \"USD\" .\n\
                ex:ref a :Refund ; :refundOf ex:pay ; :amountMinor 1000 ; \
                  :amountCurrency \"USD\" .\n\
                ex:out a :Payout ; :settlesPayment ex:pay ; :amountMinor 4000 ; \
                  :amountCurrency \"USD\" .\n\
                ex:fee a :Fee ; :feeOfPayout ex:out ; :amountMinor 158 ; \
                  :amountCurrency \"USD\" .\n";
    assert_eq!(conforms(good), (true, Vec::new()));

    // A float literal on an amount property is refused.
    let (ok, paths) = conforms(
        "ex:pay a :Payment ; :amountMinor 41.58 ; :amountCurrency \"USD\" .\n",
    );
    assert!(!ok, "a float amount is refused");
    assert!(paths.iter().any(|path| path.ends_with("amountMinor>")));
    let (ok, _) = conforms("ex:line a :OrderLine ; :unitPriceMinor 19.99 .\n");
    assert!(!ok, "a float unit price is refused");

    // An amount with no currency is refused.
    let (ok, _) = conforms("ex:pay a :Payment ; :amountMinor 100 .\n");
    assert!(!ok, "an amount without a currency is refused");

    // A refund with no payment is refused.
    let (ok, paths) = conforms(
        "ex:ref a :Refund ; :amountMinor 100 ; :amountCurrency \"USD\" .\n",
    );
    assert!(!ok, "a refund without a payment is refused");
    assert!(paths.iter().any(|path| path.ends_with("refundOf>")));

    // A refund above its payment is refused; a refund equal to it conforms.
    let over = "ex:pay a :Payment ; :amountMinor 1000 ; :amountCurrency \"USD\" .\n\
                ex:ref a :Refund ; :refundOf ex:pay ; :amountMinor 1001 ; \
                  :amountCurrency \"USD\" .\n";
    let (ok, _) = conforms(over);
    assert!(!ok, "a refund above its payment is refused");
    let equal = over.replace("1001", "1000");
    assert_eq!(conforms(&equal), (true, Vec::new()));
}
