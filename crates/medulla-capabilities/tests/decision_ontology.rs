//! MEDULLA-DECISION-ENGINE-R054.1: the decision ontology and shapes are derived
//! from the contract schemas at build time, never committed (MEDULLA-SDLC-R020.1).
//! This test asserts the properties directly instead of diffing a stored copy.
#![cfg(feature = "contract-schema")]

use medulla_capabilities::decision_ontology::{decision_ontology_ttl, decision_shapes_ttl};

const CLASSES: [&str; 4] = [
    "DecideRequest",
    "DecisionEvalOp",
    "DecisionFitOp",
    "StatisticalDecisionRecord",
];

// spec: MEDULLA-DECISION-ENGINE-R054.1
#[test]
fn generated_decision_ontology_is_byte_stable_and_covers_every_contract_class() {
    let ontology = decision_ontology_ttl();
    let shapes = decision_shapes_ttl();
    assert_eq!(ontology, decision_ontology_ttl(), "ontology is byte-stable");
    assert_eq!(shapes, decision_shapes_ttl(), "shapes are byte-stable");
    for class in CLASSES {
        assert!(
            ontology.contains(&format!(":{class} a owl:Class")),
            "{class} is an owl:Class"
        );
        assert!(shapes.contains(class), "{class} has a shape");
    }
    assert!(
        shapes.contains("sh:property"),
        "shapes carry property constraints"
    );
}
