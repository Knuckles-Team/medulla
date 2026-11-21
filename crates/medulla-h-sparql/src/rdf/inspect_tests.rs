//! `OntologyInspect` over inline documents and the composed GraphSchema sources.

use super::*;

const KG: &str = "https://knuckles-team.github.io/ontology/core#";
/// A compiled connector-manifest ontology fixture and its independently
/// computed canonical digest (89 triples).
const ARR_MANIFEST: &str =
    include_str!("../../../../tests/fixtures/ontology_inspect/arr_connector_manifest.ttl");
const ARR_PINNED_DIGEST: &str = "0bfdd9900775d98d945eb2a76a9b84d9a28312af81d08ec98333eca2764d534d";

const VOCABULARY: &str = "@prefix : <https://knuckles-team.github.io/ontology/core#> .\n\
    @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
    @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
    @prefix sh: <http://www.w3.org/ns/shacl#> .\n\
    :Agent a owl:Class ; rdfs:label \"Agent\" ; rdfs:comment \"An actor.\" .\n\
    :Planner a owl:Class ; rdfs:subClassOf :Agent .\n\
    :peerOf a owl:ObjectProperty, owl:SymmetricProperty ; rdfs:label \"peer of\" ;\n\
        rdfs:domain :Agent ; rdfs:range :Agent .\n\
    :name a owl:DatatypeProperty ; rdfs:range xsd:string .\n\
    :AgentShape a sh:NodeShape ; sh:targetClass :Agent .\n";

/// The graph the inspection fixtures are read on.
const TEST_GRAPH: &str = "inspect-graph";

fn kg(local: &str) -> String {
    format!("{KG}{local}")
}

fn inline(documents: &[&str]) -> Result<OntologyInspection, String> {
    let documents: Vec<String> = documents
        .iter()
        .map(|document| document.to_string())
        .collect();
    inspect(&GraphCore::new(), TEST_GRAPH, &documents, &[])
}

#[test]
fn a_compiled_connector_manifest_keeps_its_pinned_canonical_digest() {
    let view = inline(&[ARR_MANIFEST]).unwrap();
    assert_eq!(view.canonical_digest.as_deref(), Some(ARR_PINNED_DIGEST));
    assert_eq!(view.triple_count, 89);
    assert_eq!(view.schema_digests.len(), 1);
    assert_eq!(view.composed_digest, None);
    assert_eq!(
        view.ontologies,
        ["https://knuckles-team.github.io/ontology/connector/arr"]
    );
    let album = view
        .classes
        .iter()
        .find(|class| class.iri == kg("Album"))
        .unwrap();
    assert_eq!(album.label.as_deref(), Some("Album"));
}

#[test]
fn the_vocabulary_view_names_classes_properties_and_targets() {
    let view = inline(&[VOCABULARY]).unwrap();
    let classes: Vec<&str> = view
        .classes
        .iter()
        .map(|class| class.iri.as_str())
        .collect();
    assert_eq!(classes, [kg("Agent"), kg("Planner")]);
    assert_eq!(view.classes[0].comment.as_deref(), Some("An actor."));
    assert_eq!(view.classes[1].parents, [kg("Agent")]);

    let peer = &view.object_properties[0];
    assert_eq!(peer.iri, kg("peerOf"));
    assert_eq!(peer.label.as_deref(), Some("peer of"));
    assert_eq!(
        (peer.domains.clone(), peer.ranges.clone()),
        (vec![kg("Agent")], vec![kg("Agent")])
    );
    assert!(peer.symmetric);

    let name = &view.datatype_properties[0];
    assert_eq!(name.ranges, ["http://www.w3.org/2001/XMLSchema#string"]);
    assert!(!name.symmetric);
    assert_eq!(view.shape_target_classes, [kg("Agent")]);
    assert!(view.canonical_digest.is_some());
}

#[test]
fn the_canonical_digest_ignores_serialization_and_tracks_content() {
    let turtle = inline(&[VOCABULARY]).unwrap();
    let lines: Vec<String> = medulla_rdf::mapping::parse_turtle(VOCABULARY)
        .unwrap()
        .iter()
        .rev()
        .map(|triple| format!("{triple} ."))
        .collect();
    let ntriples = inline(&[&lines.join("\n")]).unwrap();
    assert_eq!(turtle.canonical_digest, ntriples.canonical_digest);
    assert_eq!(turtle.triple_count, ntriples.triple_count);
    let grown = format!("{VOCABULARY}:Gamma a owl:Class .\n");
    assert_ne!(
        inline(&[&grown]).unwrap().canonical_digest,
        turtle.canonical_digest
    );
}

#[test]
fn a_document_with_blank_nodes_has_no_canonical_digest() {
    let shapes = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
                  @prefix : <https://knuckles-team.github.io/ontology/core#> .\n\
                  :S a sh:NodeShape ; sh:targetClass :Agent ; sh:property [ sh:path :name ] .\n";
    let view = inline(&[shapes]).unwrap();
    assert_eq!(view.canonical_digest, None);
    assert_eq!(view.shape_target_classes, [kg("Agent")]);
}

#[test]
fn the_composed_schema_reads_the_live_core_catalog() {
    let core = GraphCore::new();
    let all = inspect(&core, TEST_GRAPH, &[], &[]).unwrap();
    assert!(all.composed_digest.is_some());
    assert!(all.triple_count > 0);
    assert!(
        all.ontologies
            .contains(&"https://knuckles-team.github.io/ontology/core".to_string()),
        "{:?}",
        all.ontologies
    );

    // A named-source filter reads a strict subset of the same composed catalog
    // (same composed digest, fewer triples than the unfiltered read).
    let catalog = inspect(&core, TEST_GRAPH, &[], &["core:catalog@1".to_string()]).unwrap();
    assert_eq!(catalog.schema_digests.len(), 1);
    assert_eq!(catalog.composed_digest, all.composed_digest);
    assert!(catalog.triple_count <= all.triple_count);
}

// spec: MEDULLA-TYPED-PACKS-R098
#[test]
fn ambiguous_unknown_and_malformed_requests_are_refused() {
    let core = GraphCore::new();
    let both = inspect(
        &core,
        TEST_GRAPH,
        &[VOCABULARY.to_string()],
        &["core:catalog@1".to_string()],
    );
    assert!(both.unwrap_err().contains("not both"));
    let unknown = inspect(&core, TEST_GRAPH, &[], &["core:no-such@1".to_string()]);
    assert!(unknown
        .unwrap_err()
        .contains("unknown GraphSchema source 'core:no-such@1'"));
    let malformed = inline(&["this is not turtle ."]);
    assert!(malformed
        .unwrap_err()
        .starts_with("documents[0] is not Turtle"));
}

#[test]
fn literal_lexical_forms_never_become_named_vocabulary_links() {
    let view = inline(&[r#"
        @prefix : <http://ex/> .
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix sh: <http://www.w3.org/ns/shacl#> .
        :FakeClass a "http://www.w3.org/2002/07/owl#Class" .
        :FakeOntology a "http://www.w3.org/2002/07/owl#Ontology" .
        :FakeObject a "http://www.w3.org/2002/07/owl#ObjectProperty" .
        :FakeDatatype a "http://www.w3.org/2002/07/owl#DatatypeProperty" .
        :C a owl:Class ; rdfs:subClassOf :Parent, "http://ex/FakeParent" ;
            rdfs:label "Class label" ; rdfs:comment "Class comment" .
        :p a owl:ObjectProperty, "http://www.w3.org/2002/07/owl#SymmetricProperty" ;
            rdfs:domain :C, "http://ex/FakeDomain" ;
            rdfs:range :Parent, "http://ex/FakeRange" .
        :s sh:targetClass :C, "http://ex/FakeTarget" .
    "#])
    .unwrap();
    assert!(view.ontologies.is_empty());
    assert!(view.datatype_properties.is_empty());
    assert_eq!(view.classes.len(), 1);
    assert_eq!(view.classes[0].iri, "http://ex/C");
    assert_eq!(view.classes[0].parents, ["http://ex/Parent"]);
    assert_eq!(view.classes[0].label.as_deref(), Some("Class label"));
    assert_eq!(view.classes[0].comment.as_deref(), Some("Class comment"));
    assert_eq!(view.object_properties.len(), 1);
    assert_eq!(view.object_properties[0].domains, ["http://ex/C"]);
    assert_eq!(view.object_properties[0].ranges, ["http://ex/Parent"]);
    assert!(!view.object_properties[0].symmetric);
    assert_eq!(view.shape_target_classes, ["http://ex/C"]);
}

#[test]
fn inspection_rejects_document_count_and_bytes_before_parsing() {
    let documents = vec![String::new(); MAX_INSPECT_DOCUMENTS + 1];
    assert!(inspect(&GraphCore::new(), TEST_GRAPH, &documents, &[])
        .unwrap_err()
        .contains("documents exceed"));
    let bytes = medulla_catalog_types::graph_schema::MAX_SCHEMA_DOCUMENT_BYTES;
    // An over-limit malformed document must fail its byte budget before syntax.
    let oversized = "!".repeat(bytes + 1);
    assert!(inline(&[&oversized])
        .unwrap_err()
        .contains(&format!("maximum of {bytes} bytes")));
    let boundary = " ".repeat(bytes);
    assert_eq!(inline(&[&boundary]).unwrap().triple_count, 0);
    assert!(inspect(&GraphCore::new(), TEST_GRAPH, &documents[..MAX_INSPECT_DOCUMENTS], &[]).is_ok());
}

#[test]
fn inspection_stops_at_the_first_triple_over_budget() {
    let limit = medulla_graph_core::graph::MAX_SCHEMA_DOCUMENT_TRIPLES;
    let boundary = format!("@prefix : <x:> .\n{}", ":a :b :c .\n".repeat(limit));
    assert_eq!(inline(&[&boundary]).unwrap().triple_count, 1);
    // Stay below the byte bound and place invalid syntax after the over-limit
    // triple: a parser that eagerly drains the document reports syntax instead.
    let oversized = format!("{boundary}:a :b :c .\nthis is not Turtle .");
    assert!(oversized.len() < medulla_catalog_types::graph_schema::MAX_SCHEMA_DOCUMENT_BYTES);
    let error = inline(&[&oversized]).unwrap_err();
    assert!(
        error.contains(&format!("maximum of {limit} triples")),
        "{error}"
    );
}

fn core_crosswalk(
    source: &str,
    target: &str,
) -> medulla_catalog_types::domain_pack::VocabularyCrosswalk {
    use medulla_catalog_types::domain_pack::{
        ApprovedTargetDeclaration, VocabularyCrosswalk, VocabularyCrosswalkRelation,
        VOCABULARY_CROSSWALK_SCHEMA_VERSION,
    };
    let mut record = VocabularyCrosswalk {
        schema_version: VOCABULARY_CROSSWALK_SCHEMA_VERSION,
        source_class_iri: kg(source),
        target_class_iri: format!("http://schema.org/{target}"),
        relation: VocabularyCrosswalkRelation::SkosCloseMatch,
        source_revision: GraphSchemaSources::default().composed_digest().to_hex(),
        target_vocabulary_revision: "ab".repeat(32),
        origin: "core-metadata-v1".to_string(),
        content_digest: String::new(),
        approved_target_declarations: vec![ApprovedTargetDeclaration {
            class_iri: format!("http://schema.org/{target}"),
            category: "continuant".to_string(),
        }],
    };
    record.content_digest = record.compute_content_digest();
    record
}

// spec: MEDULLA-TYPED-PACKS-R071.1.3
#[test]
fn inspection_returns_composed_crosswalks() {
    use medulla_graph_core::graph::{GraphSchemaSource, SchemaSourceOrigin};

    let records = vec![
        core_crosswalk("Agent", "Person"),
        core_crosswalk("Agent", "Organization"),
    ];
    let mut sources = GraphSchemaSources::default();
    let source = GraphSchemaSource::new(
        SchemaSourceOrigin::Pack {
            connector: "metadata".to_string(),
            record_id: "record-1".to_string(),
            tenant: None,
        },
        None,
        None,
        0,
    )
    .unwrap()
    .with_crosswalks(records.clone());
    sources
        .attach_dynamic("pack:metadata".to_string(), source)
        .unwrap();
    let composed = sources.composed_digest().to_hex();
    let core = GraphCore::new();
    core.install_schema_sources(std::sync::Arc::new(sources));

    let view = inspect(&core, TEST_GRAPH, &[], &[]).unwrap();
    assert!(view.committed);
    assert_eq!(view.composed_digest.as_deref(), Some(composed.as_str()));
    let mut expected: Vec<(String, String, String)> = records
        .iter()
        .map(|record| {
            (
                record.source_class_iri.clone(),
                record.target_class_iri.clone(),
                record.content_digest.clone(),
            )
        })
        .collect();
    expected.sort();
    let served: Vec<(String, String, String)> = view
        .crosswalks
        .iter()
        .map(|crosswalk| {
            (
                crosswalk.source_class_iri.clone(),
                crosswalk.target_class_iri.clone(),
                crosswalk.crosswalk_digest.clone(),
            )
        })
        .collect();
    assert_eq!(served, expected);
    for crosswalk in &view.crosswalks {
        assert_eq!(crosswalk.source_id, "pack:metadata");
        assert_eq!(crosswalk.relation, "skos_close_match");
        assert_eq!(crosswalk.crosswalk_digest.len(), 64);
        assert!(crosswalk
            .crosswalk_digest
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
    }

    // Inline inspection never claims activation and lists no crosswalk.
    let inline_view = inline(&[VOCABULARY]).unwrap();
    assert!(!inline_view.committed);
    assert!(inline_view.crosswalks.is_empty());
}

// spec: MEDULLA-TYPED-PACKS-R097.2.3
#[test]
fn inspection_reports_pack_revision_coverage() {
    use medulla_catalog_types::domain_pack::{compute_coverage_of, CrosswalkContext};
    use medulla_graph_core::graph::{GraphSchemaSource, SchemaSourceOrigin};

    let records = vec![core_crosswalk("Agent", "Person")];
    let revision = GraphSchemaSources::default().composed_digest().to_hex();
    let ctx = CrosswalkContext {
        source_revision: revision.clone(),
        scope: "core".to_string(),
        source_classes: [kg("Agent"), kg("Planner")]
            .into_iter()
            .map(|iri| (iri, "continuant".to_string()))
            .collect(),
        selected_sources: [kg("Agent"), kg("Planner")].into_iter().collect(),
        hierarchy: Default::default(),
    };
    let coverage = compute_coverage_of(&records, &ctx).unwrap();
    let mut sources = GraphSchemaSources::default();
    let pack = |connector: &str, records: Vec<_>, coverage| {
        GraphSchemaSource::new(
            SchemaSourceOrigin::Pack {
                connector: connector.to_string(),
                record_id: "record-1".to_string(),
                tenant: None,
            },
            None,
            None,
            0,
        )
        .unwrap()
        .with_crosswalks(records)
        .with_crosswalk_coverage(coverage)
    };
    let bound = |coverage: medulla_catalog_types::domain_pack::CrosswalkCoverage| {
        medulla_catalog_types::domain_pack::BoundCrosswalkCoverage {
            coverage,
            binding: medulla_catalog_types::domain_pack::CrosswalkCoverageBinding {
                tenant_id: "tenant-a".to_string(),
                graph: TEST_GRAPH.to_string(),
                pack_record_id: "record-1".to_string(),
                target_vocabulary_revisions: vec!["target-rev".to_string()],
                schema_digest: "ab".repeat(32),
            },
        }
    };
    sources
        .attach_dynamic(
            "pack:metadata".to_string(),
            pack("metadata", records.clone(), Some(bound(coverage.clone()))),
        )
        .unwrap();
    sources
        .attach_dynamic("pack:bare".to_string(), pack("bare", Vec::new(), None))
        .unwrap();
    let core = GraphCore::new();
    core.install_schema_sources(std::sync::Arc::new(sources));

    let view = inspect(&core, TEST_GRAPH, &[], &[]).unwrap();
    assert_eq!(
        view.coverage.len(),
        1,
        "only the source with crosswalks reports"
    );
    let served = &view.coverage[0];
    assert_eq!(served.source_id, "pack:metadata");
    assert_eq!(served.source_revision, revision);
    assert_eq!(served.declared, 2);
    assert_eq!(served.mapped, vec![kg("Agent")]);
    assert_eq!(served.unmapped.len(), 1);
    assert_eq!(served.unmapped[0].class_iri, kg("Planner"));
    assert_eq!(served.unmapped[0].reason, "no_declared_mapping");
    assert_eq!(served.source_digest, coverage.source_digest);
    assert_eq!(served.tenant_id, "tenant-a");
    assert_eq!(served.graph, TEST_GRAPH);
    assert_eq!(served.pack_record_id, "record-1");
    assert_eq!(served.target_vocabulary_revisions, vec!["target-rev".to_string()]);
    assert_eq!(served.schema_digest, "ab".repeat(32));

    // A coverage bound to another graph is not reported on this one.
    let elsewhere = inspect(&core, "another-graph", &[], &[]).unwrap();
    assert!(elsewhere.coverage.is_empty());

    // Selecting another source reports nothing; inline inspection reports none.
    let other = inspect(&core, TEST_GRAPH, &[], &["pack:bare".to_string()]).unwrap();
    assert!(other.coverage.is_empty());
    assert!(inline(&[VOCABULARY]).unwrap().coverage.is_empty());
}
