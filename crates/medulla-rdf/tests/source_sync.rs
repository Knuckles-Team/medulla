//! Sync role vocabulary (MEDULLA-SRC-AUTH-R012.1): the terms a source-sync
//! role declaration uses resolve in the virtual-graph core ontology.
#![cfg(feature = "rdf")]

mod common;

use common::has_iri;
use medulla_rdf::mapping::parse_turtle;

const DOCUMENT: &str = include_str!("../../medulla-core/ontology/virtual_graph-v1.ttl");
const VG: &str = "https://knuckles-team.github.io/ontology/virtual-graph#";
const KG: &str = "https://knuckles-team.github.io/ontology/core#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const OWL: &str = "http://www.w3.org/2002/07/owl#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";

// spec: MEDULLA-SRC-AUTH-R012.1
#[test]
fn sync_terms_resolve() {
    let triples = parse_turtle(DOCUMENT).unwrap();
    let domain = format!("{RDFS}domain");
    let range = format!("{RDFS}range");
    let class = |name: &str| format!("{VG}{name}");
    // (property, kind, domain class, range)
    let properties = [
        (
            "recordRole",
            "DatatypeProperty",
            "SourceAuthority",
            format!("{XSD}string"),
        ),
        (
            "syncDirection",
            "DatatypeProperty",
            "SourceAuthority",
            format!("{XSD}string"),
        ),
        (
            "conflictRule",
            "DatatypeProperty",
            "SourceAuthority",
            format!("{XSD}string"),
        ),
        (
            "syncConnection",
            "ObjectProperty",
            "SyncState",
            class("SourceConnection"),
        ),
        (
            "syncClass",
            "ObjectProperty",
            "SyncState",
            format!("{OWL}Class"),
        ),
        (
            "syncRecordKey",
            "DatatypeProperty",
            "SyncState",
            format!("{XSD}string"),
        ),
        (
            "baseDigest",
            "DatatypeProperty",
            "SyncState",
            format!("{XSD}string"),
        ),
        (
            "sourceRevision",
            "DatatypeProperty",
            "SyncState",
            format!("{XSD}string"),
        ),
        (
            "syncedAtMs",
            "DatatypeProperty",
            "SyncState",
            format!("{XSD}nonNegativeInteger"),
        ),
    ];
    for (name, kind, owner, range_iri) in properties {
        let subject = class(name);
        assert!(
            has_iri(&triples, &subject, RDF_TYPE, &format!("{OWL}{kind}")),
            "{name} kind"
        );
        assert!(
            has_iri(&triples, &subject, &domain, &class(owner)),
            "{name} domain"
        );
        assert!(
            has_iri(&triples, &subject, &range, &range_iri),
            "{name} range"
        );
    }
    assert!(has_iri(
        &triples,
        &class("SyncState"),
        RDF_TYPE,
        &format!("{OWL}Class")
    ));
    assert!(has_iri(
        &triples,
        &class("SyncState"),
        &format!("{RDFS}subClassOf"),
        &format!("{KG}DomainEntity")
    ));
}
