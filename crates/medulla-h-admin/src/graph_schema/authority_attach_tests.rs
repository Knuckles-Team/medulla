//! A generic attach of an authority document is validated against the core
//! virtual-graph shapes before anything is attached (MEDULLA-SRC-AUTH-R002.5).

use medulla_catalog_types::graph_schema::GraphSchemaOp;
use medulla_graph_core::graph::GraphCore;

/// One SQL connection, its contract, an approved mapping and an authority row ending in
/// `declaration`; `copy` adds the connection's copy capability.
fn document(declaration: &str, copy: bool) -> String {
    let capability = if copy {
        " ; vg:declaredCapability \"copy\""
    } else {
        ""
    };
    format!(
        "@prefix vg: <https://knuckles-team.github.io/ontology/virtual-graph#> .\n\
         @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
         @prefix ex: <urn:authority-attach:> .\n\
         ex:conn a vg:SourceConnection ; vg:sourceKind \"sql\" ; vg:endpointRef ex:ep ;\n\
           vg:foreignSourceName \"items\"{capability} .\n\
         ex:ep vg:url \"postgres://source.example.invalid/db\" .\n\
         ex:contract a vg:MetadataContract ; vg:contractSource ex:conn ; vg:schemaVersion \"1\" ;\n\
           vg:contentDigest \"sha256:{zero:0>64}\" ;\n\
           vg:discoveredEntity \"items\" ; vg:discoveredField \"items.id\", \"items.label\" .\n\
         ex:mapping a vg:VirtualMapping ; vg:mapsContract ex:contract ;\n\
           vg:mapsOntologyClass ex:Item ; vg:mapsEntity \"items\" ; vg:keyField \"items.id\" ;\n\
           vg:approved true ; vg:hasPredicateFieldPair ex:pair .\n\
         ex:pair a vg:PredicateFieldPair ; vg:predicate rdfs:label ; vg:field \"items.label\" .\n\
         ex:row a vg:SourceAuthority ; vg:authoritativeSource ex:conn ;\n\
           vg:authorityFor ex:Item ; vg:authorityMapping ex:mapping ;\n\
           vg:authorityRank 1 ; vg:authorityContract \"virtual-graph\" {declaration} .\n",
        zero = "0"
    )
}

const FULL: &str = "; vg:materialisation \"accelerated\" ; \
    vg:freshnessMaxAgeSeconds \"300\"^^xsd:nonNegativeInteger ; \
    vg:materialisationReason \"the table is slow and read often\"";

fn attach(core: &GraphCore, ontology: String) -> Result<(), String> {
    let op: GraphSchemaOp = serde_json::from_value(serde_json::json!({
        "op": "attach",
        "source_id": "admin:authority-attach",
        "ontology_ttl": ontology,
    }))
    .expect("an attach op");
    medulla_h_admin_schema::graph_schema::apply(core, "g", &op).map(drop)
}

// spec: MEDULLA-SRC-AUTH-R002.5
#[test]
fn an_accelerated_row_needs_reason_freshness_and_copy() {
    let core = GraphCore::new();
    let before = core.schema_sources().composed_digest().to_hex();
    for (row, copy, message) in [
        (
            FULL.replace(
                "; vg:materialisationReason \"the table is slow and read often\"",
                "",
            ),
            true,
            "materialisationReason",
        ),
        (
            FULL.replace(
                "; vg:freshnessMaxAgeSeconds \"300\"^^xsd:nonNegativeInteger ",
                "",
            ),
            true,
            "freshnessMaxAgeSeconds",
        ),
        (FULL.to_string(), false, "copy capability"),
    ] {
        let refused = attach(&core, document(&row, copy)).expect_err("must be refused");
        assert!(refused.starts_with("ONTOLOGY_INVALID"), "{refused}");
        assert!(refused.contains(message), "{refused}");
        assert_eq!(
            core.schema_sources().composed_digest().to_hex(),
            before,
            "a refused attach stores nothing"
        );
    }
    attach(&core, document(FULL, true)).expect("a complete accelerated row attaches");
    assert_ne!(core.schema_sources().composed_digest().to_hex(), before);
}

// spec: MEDULLA-SRC-AUTH-R002.5
#[test]
fn a_virtual_row_attaches_without_a_declaration() {
    let core = GraphCore::new();
    attach(&core, document("", false)).expect("a virtual row attaches");
}
