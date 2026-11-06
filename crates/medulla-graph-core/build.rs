//! Renders the decision contract's ontology and shapes into `OUT_DIR`
//! (MEDULLA-DECISION-ENGINE-R054.1, MEDULLA-SDLC-R020.1).
//!
//! The documents are derived from the decision DTOs' JSON schemas by
//! `medulla_decision_types::ontology`, so they are produced where they are
//! consumed and never committed: a decision-contract change needs no
//! follow-up regeneration commit.

use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    std::fs::write(
        out.join("decision.ttl"),
        medulla_decision_types::ontology::decision_ontology_ttl(),
    )
    .expect("write decision.ttl");
    std::fs::write(
        out.join("decision.shapes.ttl"),
        medulla_decision_types::ontology::decision_shapes_ttl(),
    )
    .expect("write decision.shapes.ttl");
    println!("cargo:rerun-if-changed=build.rs");
}
