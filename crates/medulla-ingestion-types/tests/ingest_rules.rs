//! Grounding against a class hierarchy (MEDULLA-INGEST-WORKER-R701.8).

use std::collections::{BTreeMap, BTreeSet};

use medulla_ingestion_types::extracted_fact::{EvidenceSpan, ExtractedFact, GroundingResult};
use medulla_ingestion_types::ingest_rules::{
    ground_fact, subclass_closure, GroundingCache, IngestRefusal, MAX_HIERARCHY_STEPS,
};

fn fact(subject: &str, predicate: &str, object: &str) -> ExtractedFact {
    ExtractedFact {
        subject: subject.into(),
        predicate: predicate.into(),
        object: object.into(),
        evidence: EvidenceSpan::new("doc:1", 0, 10).unwrap(),
        confidence: 0.9,
    }
}

fn edges(pairs: &[(&str, &str)]) -> BTreeMap<String, BTreeSet<String>> {
    let mut direct: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (class, parent) in pairs {
        direct
            .entry((*class).to_string())
            .or_default()
            .insert((*parent).to_string());
    }
    direct
}

// spec: MEDULLA-INGEST-WORKER-R701.8
/// Verifies MEDULLA-INGEST-WORKER-R701.8: a `Dog` subject grounds a predicate whose domain is
/// `Animal`, the grounded result keeps the declared classes, direction repair uses the same
/// check, and a cyclic or over-deep hierarchy refuses `ONTOLOGY_HIERARCHY_CYCLE`.
#[test]
fn subclass_entity_grounds_to_declared_domain_and_cycle_refuses() {
    let closure = subclass_closure(&edges(&[("Dog", "Mammal"), ("Mammal", "Animal")])).unwrap();
    assert_eq!(
        closure["Dog"],
        BTreeSet::from(["Mammal".to_string(), "Animal".to_string()])
    );
    let cache = GroundingCache {
        predicates: BTreeMap::from([(
            "owns".to_string(),
            ("Animal".to_string(), "Toy".to_string()),
        )]),
        classes: BTreeMap::from([
            ("Rex".to_string(), "Dog".to_string()),
            ("Bone".to_string(), "Toy".to_string()),
            ("Rock".to_string(), "Mineral".to_string()),
        ]),
        subclass_of: closure,
    };
    match ground_fact(fact("Rex", "owns", "Bone"), &cache).grounding {
        GroundingResult::Grounded {
            subject_class,
            object_class,
            direction_repaired,
            ..
        } => {
            assert_eq!(
                (subject_class.as_str(), object_class.as_str()),
                ("Animal", "Toy")
            );
            assert!(!direction_repaired);
        }
        other => panic!("a Dog grounds to the Animal domain, got {other:?}"),
    }
    // The swapped pair repairs by the same check.
    assert!(matches!(
        ground_fact(fact("Bone", "owns", "Rex"), &cache).grounding,
        GroundingResult::Grounded {
            direction_repaired: true,
            ..
        }
    ));
    // A class outside the hierarchy does not fit.
    assert_eq!(
        ground_fact(fact("Rock", "owns", "Bone"), &cache).grounding,
        GroundingResult::Ungrounded
    );
    // A cycle refuses with the stable code.
    let cycle = subclass_closure(&edges(&[("A", "B"), ("B", "C"), ("C", "A")])).unwrap_err();
    assert_eq!(cycle.code(), "ONTOLOGY_HIERARCHY_CYCLE");
    assert!(matches!(cycle, IngestRefusal::OntologyHierarchyCycle(_)));
    // A chain of exactly 64 steps closes; one more refuses.
    let chain = |links: usize| {
        let pairs: Vec<(String, String)> = (0..links)
            .map(|n| (format!("C{n}"), format!("C{}", n + 1)))
            .collect();
        let refs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        edges(&refs)
    };
    assert!(subclass_closure(&chain(MAX_HIERARCHY_STEPS)).is_ok());
    assert_eq!(
        subclass_closure(&chain(MAX_HIERARCHY_STEPS + 1))
            .unwrap_err()
            .code(),
        "ONTOLOGY_HIERARCHY_CYCLE"
    );
}
