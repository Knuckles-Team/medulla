//! The ontology slice of one extraction unit (MEDULLA-INGEST-WORKER-R913.1, R913.2).
#![cfg(all(feature = "blob", feature = "decide"))]

use std::sync::Arc;

use medulla_graph_core::graph::{GraphSchemaSource, GraphSchemaSources, SchemaSourceOrigin};
use medulla_h_ingest::ontology_slice::slice_for_unit;
use medulla_ingestion_types::ingestion_wire::OntologySliceRequest;

const NS: &str = "http://example.org/o#";

/// `count` classes, each with a label, a comment holding its own marker word and a property from
/// it to the next class; the first class is a subclass of `Root`, which has its own property.
fn ontology(count: usize) -> String {
    let mut lines = vec![
        "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .".to_string(),
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .".to_string(),
        format!("@prefix ex: <{NS}> ."),
        "ex:Root a owl:Class ; rdfs:label \"Root\" .".to_string(),
        "ex:rootRel rdfs:domain ex:Root ; rdfs:range ex:Root .".to_string(),
    ];
    for n in 0..count {
        lines.push(format!(
            "ex:C{n:03} a owl:Class ; rdfs:label \"Species {n:03}\" ; \
             rdfs:comment \"Describes tokenmarker{n:03} in the field.\" ."
        ));
        lines.push(format!(
            "ex:rel{n:03} rdfs:domain ex:C{n:03} ; rdfs:range ex:C{:03} .",
            (n + 1) % count
        ));
    }
    lines.push("ex:C000 rdfs:subClassOf ex:Root .".to_string());
    lines.join("\n")
}

fn sources(document: &str) -> GraphSchemaSources {
    let mut sources = GraphSchemaSources::default();
    let source = GraphSchemaSource::new(
        SchemaSourceOrigin::Admin {
            name: "species".to_string(),
        },
        None,
        Some(Arc::from(document)),
        0,
    )
    .unwrap();
    sources
        .attach_dynamic("admin:species".to_string(), source)
        .unwrap();
    sources
}

fn request(text: &str, max_classes: u32) -> OntologySliceRequest {
    OntologySliceRequest {
        unit_text: text.to_string(),
        unit_embedding: None,
        max_classes,
    }
}

// spec: MEDULLA-INGEST-WORKER-R913.1
/// Verifies MEDULLA-INGEST-WORKER-R913.1: the slice holds the best-matching class, caps at
/// `max_classes`, carries the properties of each returned class and of its superclasses, is
/// deterministic, and is the whole ontology when the ontology is small.
#[test]
fn top_classes_with_connected_properties_and_bypass() {
    let large = sources(&ontology(60));
    let unit = "A paragraph about tokenmarker000 and tokenmarker017.";
    let slice = slice_for_unit(&large, &request(unit, 5), None).unwrap();
    assert!(!slice.bypassed);
    let iris: Vec<&str> = slice
        .classes
        .iter()
        .map(|class| class.iri.as_str())
        .collect();
    assert!(iris.contains(&format!("{NS}C000").as_str()));
    assert!(iris.contains(&format!("{NS}C017").as_str()));
    assert!(iris.len() <= 5);
    let properties: Vec<&str> = slice
        .properties
        .iter()
        .map(|property| property.iri.as_str())
        .collect();
    // Each returned class brings the property that starts at it.
    assert!(properties.contains(&format!("{NS}rel017").as_str()));
    // C000 is a subclass of Root, so the property of the superclass comes with it.
    assert!(properties.contains(&format!("{NS}rootRel").as_str()));
    // A property of a class that was not returned does not.
    assert!(!properties.contains(&format!("{NS}rel040").as_str()));
    // The same input gives the same slice.
    assert_eq!(
        slice,
        slice_for_unit(&large, &request(unit, 5), None).unwrap()
    );

    // 62 classes (60 and Root, and nothing more) fit under a cap of 256: the slice is all of it.
    let whole = slice_for_unit(&large, &request(unit, 256), None).unwrap();
    assert!(whole.bypassed);
    assert_eq!(whole.classes.len(), 61);
    assert_eq!(whole.properties.len(), 61);
}

// spec: MEDULLA-INGEST-WORKER-R913.1
/// The embedding arm fuses with the lexical arm when the request carries the unit's embedding.
#[test]
fn an_embedding_ranks_a_class_the_text_does_not_name() {
    let large = sources(&ontology(60));
    // The embedder places a class text at a vector that depends on its marker number only, so
    // the unit vector below sits next to class 033 and no other.
    let embed = |text: &str| -> Result<Vec<f32>, String> {
        let mut vector = vec![0.0f32; 4];
        vector[if text.contains("tokenmarker033") {
            0
        } else {
            1
        }] = 1.0;
        Ok(vector)
    };
    let mut asked = request("a paragraph with no marker word", 5);
    asked.unit_embedding = Some(vec![1.0, 0.0, 0.0, 0.0]);
    let slice = slice_for_unit(&large, &asked, Some(&embed)).unwrap();
    let iris: Vec<&str> = slice
        .classes
        .iter()
        .map(|class| class.iri.as_str())
        .collect();
    assert_eq!(iris, vec![format!("{NS}C033").as_str()]);
    // An embedding with no bound embedder, and one of the wrong width, refuse.
    assert_eq!(
        slice_for_unit(&large, &asked, None).unwrap_err().code,
        "INVALID_ARGUMENT"
    );
    asked.unit_embedding = Some(vec![1.0, 0.0]);
    assert_eq!(
        slice_for_unit(&large, &asked, Some(&embed))
            .unwrap_err()
            .code,
        "INVALID_ARGUMENT"
    );
}

// spec: MEDULLA-INGEST-WORKER-R913.2
/// Verifies MEDULLA-INGEST-WORKER-R913.2: an empty or oversized unit, a `max_classes` outside 1
/// to 256 and a graph with no admitted ontology each refuse with their code.
#[test]
fn each_refusal_names_its_code() {
    let held = sources(&ontology(10));
    let invalid =
        |request: OntologySliceRequest| slice_for_unit(&held, &request, None).unwrap_err().code;
    assert_eq!(invalid(request("   ", 8)), "INVALID_ARGUMENT");
    assert_eq!(invalid(request(&"x".repeat(65_537), 8)), "INVALID_ARGUMENT");
    assert_eq!(invalid(request("tokenmarker001", 0)), "INVALID_ARGUMENT");
    assert_eq!(invalid(request("tokenmarker001", 257)), "INVALID_ARGUMENT");
    // The engine's own core vocabulary is not an admitted tenant ontology.
    let none = GraphSchemaSources::default();
    let refusal = slice_for_unit(&none, &request("tokenmarker001", 8), None).unwrap_err();
    assert_eq!(refusal.code, "ONTOLOGY_NOT_ADMITTED");
    assert!(refusal.message().starts_with("ONTOLOGY_NOT_ADMITTED: "));
}
