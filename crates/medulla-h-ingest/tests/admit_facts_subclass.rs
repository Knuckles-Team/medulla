#![cfg(all(feature = "redb", feature = "blob"))]
//! The grounding cache reads the subclass closure (MEDULLA-INGEST-WORKER-R701.9).

use std::collections::{BTreeMap, BTreeSet};

use medulla_h_ingest::source_ingestion::facts::{build_grounding_cache, with_reasoner_closure};
use medulla_ingestion_types::extracted_fact::{ExtractedFact, GroundingResult};
use medulla_ingestion_types::ingest_rules::ground_fact;

const FLAT_ONTOLOGY: &str = r#"
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
    @prefix ex: <http://example.org/o#> .
    ex:ownedBy rdfs:domain ex:Animal ; rdfs:range ex:Person .
    ex:Rex a ex:Dog ; rdfs:label "Rex" .
    ex:Ada a ex:Person ; rdfs:label "Ada" .
"#;

const HIERARCHY_ONTOLOGY: &str = r#"
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
    @prefix ex: <http://example.org/o#> .
    ex:Dog rdfs:subClassOf ex:Mammal .
    ex:Mammal rdfs:subClassOf ex:Animal .
    ex:ownedBy rdfs:domain ex:Animal ; rdfs:range ex:Person .
    ex:Rex a ex:Dog ; rdfs:label "Rex" .
    ex:Ada a ex:Person ; rdfs:label "Ada" .
"#;

fn rex_owned_by_ada() -> ExtractedFact {
    serde_json::from_value(serde_json::json!({
        "subject": "Rex",
        "predicate": "ownedBy",
        "object": "Ada",
        "evidence": {"source_ref": "doc:1", "start": 0, "end": 12},
        "confidence": 0.9
    }))
    .unwrap()
}

fn is_grounded(cache: &medulla_ingestion_types::ingest_rules::GroundingCache) -> bool {
    matches!(
        ground_fact(rex_owned_by_ada(), cache).grounding,
        GroundingResult::Grounded { .. }
    )
}

// spec: MEDULLA-INGEST-WORKER-R701.9
/// Verifies MEDULLA-INGEST-WORKER-R701.9: the cache a batch grounds against takes its
/// `subclass_of` from the closure the reasoner holds. An ontology with no hierarchy leaves a `Dog`
/// ungrounded; the reasoner's closure alone grounds it, and the closure of the ontology's own
/// edges grounds it when the reasoner holds none.
#[test]
fn batch_cache_reads_reasoner_closure() {
    let flat = build_grounding_cache(&[FLAT_ONTOLOGY]).unwrap();
    assert!(flat.subclass_of.is_empty());
    assert!(!is_grounded(&flat), "no hierarchy, so a Dog is not an Animal");

    let class = |name: &str| format!("http://example.org/o#{name}");
    let closure: BTreeMap<String, BTreeSet<String>> = BTreeMap::from([(
        class("Dog"),
        BTreeSet::from([class("Mammal"), class("Animal")]),
    )]);
    let from_reasoner = with_reasoner_closure(&flat, &closure);
    assert!(is_grounded(&from_reasoner));
    assert_eq!(from_reasoner.subclass_of, closure);
    // The base cache is not changed by the batch's closure.
    assert!(flat.subclass_of.is_empty());

    let from_ontology = build_grounding_cache(&[HIERARCHY_ONTOLOGY]).unwrap();
    assert!(is_grounded(&from_ontology));
    assert_eq!(from_ontology.subclass_of[&class("Dog")], closure[&class("Dog")]);
}

// spec: MEDULLA-INGEST-WORKER-R701.8
#[test]
fn a_cyclic_ontology_refuses_when_the_cache_builds() {
    let cyclic = r#"
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex: <http://example.org/o#> .
        ex:A rdfs:subClassOf ex:B .
        ex:B rdfs:subClassOf ex:A .
    "#;
    let error = build_grounding_cache(&[cyclic]).unwrap_err();
    assert!(error.starts_with("ONTOLOGY_HIERARCHY_CYCLE"), "{error}");
}
