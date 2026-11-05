//! MEDULLA-DECISION-ENGINE-R054.1: the committed decision ontology and shapes are
//! exactly what the generator emits.
#![cfg(feature = "contract-schema")]

use medulla_capabilities::decision_ontology::{decision_ontology_ttl, decision_shapes_ttl};

const ONTOLOGY: &str = include_str!("../../medulla-core/ontology/decision.ttl");
const SHAPES: &str = include_str!("../../medulla-core/ontology/decision.shapes.ttl");

// spec: MEDULLA-DECISION-ENGINE-R054.1
#[test]
fn generated_decision_ontology_matches_committed_files() {
    if std::env::var_os("MEDULLA_REGENERATE_DECISION_ONTOLOGY").is_some() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../medulla-core/ontology");
        std::fs::write(format!("{dir}/decision.ttl"), decision_ontology_ttl()).unwrap();
        std::fs::write(format!("{dir}/decision.shapes.ttl"), decision_shapes_ttl()).unwrap();
        return;
    }
    assert_eq!(
        ONTOLOGY,
        decision_ontology_ttl(),
        "decision.ttl is stale: rerun with MEDULLA_REGENERATE_DECISION_ONTOLOGY=1"
    );
    assert_eq!(
        SHAPES,
        decision_shapes_ttl(),
        "decision.shapes.ttl is stale: rerun with MEDULLA_REGENERATE_DECISION_ONTOLOGY=1"
    );
}
