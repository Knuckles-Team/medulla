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

const SHAPES: &str = include_str!("../../medulla-core/ontology/virtual_graph-v1.shapes.ttl");

/// The connections, contracts and mappings the role rows point at: the native graph and one
/// SQL source, each with a contract and an approved mapping of `ex:Host`. `rows` is appended.
fn role_report(rows: &str) -> medulla_shacl::ValidationReport {
    let data = format!(
        "@prefix vg: <{VG}> .\n\
         @prefix ex: <http://example.org/> .\n\
         vg:NativeGraph a vg:SourceConnection ; vg:sourceKind \"sparql\" ; vg:endpointRef ex:ep0 ;\n\
           vg:declaredCapability \"read\" .\n\
         ex:ep0 vg:url \"https://native.example.invalid/sparql\" .\n\
         ex:cmdb a vg:SourceConnection ; vg:sourceKind \"sql\" ; vg:endpointRef ex:ep1 ;\n\
           vg:declaredCapability \"read\", \"copy\" .\n\
         ex:ep1 vg:url \"postgres://cmdb.example.invalid/app\" .\n\
         ex:c0 a vg:MetadataContract ; vg:contractSource vg:NativeGraph ; vg:schemaVersion \"1\" ;\n\
           vg:contentDigest \"sha256:{zero:0>64}\" ; vg:discoveredEntity \"Host\" ;\n\
           vg:discoveredField \"Host.id\" .\n\
         ex:m0 a vg:VirtualMapping ; vg:mapsContract ex:c0 ; vg:mapsOntologyClass ex:Host ;\n\
           vg:mapsEntity \"Host\" ; vg:keyField \"Host.id\" ; vg:approved true ;\n\
           vg:hasPredicateFieldPair ex:p0 .\n\
         ex:p0 a vg:PredicateFieldPair ; vg:predicate ex:hostname ; vg:field \"Host.id\" .\n\
         ex:c1 a vg:MetadataContract ; vg:contractSource ex:cmdb ; vg:schemaVersion \"1\" ;\n\
           vg:contentDigest \"sha256:{zero:0>64}\" ; vg:discoveredEntity \"Host\" ;\n\
           vg:discoveredField \"Host.id\" .\n\
         ex:m1 a vg:VirtualMapping ; vg:mapsContract ex:c1 ; vg:mapsOntologyClass ex:Host ;\n\
           vg:mapsEntity \"Host\" ; vg:keyField \"Host.id\" ; vg:approved true ;\n\
           vg:hasPredicateFieldPair ex:p1 .\n\
         ex:p1 a vg:PredicateFieldPair ; vg:predicate ex:hostname ; vg:field \"Host.id\" .\n\
         {rows}\n",
        zero = "0"
    );
    medulla_shacl::validate_turtle(SHAPES, &data).expect("the role shapes validate")
}

const NATIVE_ROW: &str = "ex:a0 a vg:SourceAuthority ; vg:authoritativeSource vg:NativeGraph ;\n\
    vg:authorityFor ex:Host ; vg:authorityMapping ex:m0 ; vg:authorityRank 1 ;\n\
    vg:authorityContract \"virtual-graph\" ; vg:recordRole \"system_of_record\" .";
const SECONDARY_ROW: &str = "ex:a1 a vg:SourceAuthority ; vg:authoritativeSource ex:cmdb ;\n\
    vg:authorityFor ex:Host ; vg:authorityMapping ex:m1 ; vg:authorityRank 2 ;\n\
    vg:authorityContract \"virtual-graph\" ; vg:recordRole \"federated_secondary\" ;\n\
    vg:syncDirection \"both\" ; vg:conflictRule \"hold_for_review\" .";

fn refused_with(rows: &str, message: &str) {
    let report = role_report(rows);
    assert!(!report.conforms, "must be refused: {rows}");
    assert!(
        report
            .results
            .iter()
            .any(|r| r.message.as_deref().unwrap_or("").contains(message)),
        "expected {message:?}, got {:?}",
        report.results
    );
}

// spec: MEDULLA-SRC-AUTH-R012.2
#[test]
fn sync_shapes_refuse_planted_faults() {
    // The clean pair conforms.
    let clean = role_report(&format!("{NATIVE_ROW}\n{SECONDARY_ROW}"));
    assert!(clean.conforms, "{:?}", clean.results);
    // 1. A connected source cannot hold system_of_record.
    refused_with(
        &SECONDARY_ROW.replace("federated_secondary", "system_of_record"),
        "Only the native graph may hold system_of_record",
    );
    // 2. A federated_secondary needs syncDirection.
    refused_with(
        &SECONDARY_ROW.replace(" ;\n    vg:syncDirection \"both\"", ""),
        "needs a syncDirection",
    );
    // 3. A federated_secondary needs conflictRule.
    refused_with(
        &SECONDARY_ROW.replace(" ; vg:conflictRule \"hold_for_review\"", ""),
        "needs a conflictRule",
    );
    // 4. An accelerated secondary needs a materialisationReason (and its bound).
    let accelerated = SECONDARY_ROW.replace(
        "vg:syncDirection",
        "vg:materialisation \"accelerated\" ; vg:freshnessMaxAgeSeconds 60 ; vg:syncDirection",
    );
    refused_with(&accelerated, "needs a materialisationReason");
    let reasoned = accelerated.replace(
        "vg:syncDirection",
        "vg:materialisationReason \"bulk reads\" ; vg:syncDirection",
    );
    let ok = role_report(&format!("{NATIVE_ROW}\n{reasoned}"));
    assert!(ok.conforms, "{:?}", ok.results);
    // 5. A class has at most one system_of_record per tenant.
    let second_native = NATIVE_ROW
        .replace("ex:a0", "ex:a9")
        .replace("authorityRank 1", "authorityRank 3");
    refused_with(
        &format!("{NATIVE_ROW}\n{second_native}"),
        "at most one system_of_record per tenant",
    );
    // The same two rows for different tenants are accepted.
    let tenant_a = NATIVE_ROW.replace(" .", " ; vg:scopeTenant \"a\" .");
    let tenant_b = second_native.replace(" .", " ; vg:scopeTenant \"b\" .");
    let ok = role_report(&format!("{tenant_a}\n{tenant_b}"));
    assert!(ok.conforms, "{:?}", ok.results);
    // 6. The role sets are closed.
    refused_with(
        &SECONDARY_ROW.replace("federated_secondary", "primary"),
        "recordRole is system_of_record or federated_secondary",
    );
    refused_with(
        &SECONDARY_ROW.replace("\"both\"", "\"sideways\""),
        "syncDirection is inbound, outbound or both",
    );
    refused_with(
        &SECONDARY_ROW.replace("\"hold_for_review\"", "\"coin_flip\""),
        "conflictRule is system_of_record_wins or hold_for_review",
    );
}
