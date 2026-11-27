//! The scale inventory classes of the infrastructure ontology
//! (MEDULLA-DECISION-ENGINE-R834): six classes under the one IRI base, each rooted
//! in the core system class.
#![cfg(feature = "rdf")]

mod common;

use common::{has_iri, parse_all, KG, RDFS};

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const OWL: &str = "http://www.w3.org/2002/07/owl#";
const INFRASTRUCTURE: &str = "https://knuckles-team.github.io/ontology/infrastructure#";

// spec: MEDULLA-DECISION-ENGINE-R834
#[test]
fn the_scale_inventory_classes_are_declared_and_rooted_in_the_core_system() {
    let triples = parse_all(&[
        include_str!("../../medulla-core/ontology/core-foundation-v1.ttl"),
        include_str!("../../medulla-core/ontology/infrastructure-v1.ttl"),
    ]);
    for class in [
        "ScaleFailureDomain",
        "ScaleResourcePool",
        "ScaleWorkloadClass",
        "ScaleUnit",
        "ScaleControllerRegistration",
        "ScaleAuthorityRevision",
    ] {
        let iri = format!("{INFRASTRUCTURE}{class}");
        assert!(
            has_iri(
                &triples,
                &iri,
                &format!("{RDF}type"),
                &format!("{OWL}Class")
            ),
            "{class} is declared as a class"
        );
        assert!(
            has_iri(
                &triples,
                &iri,
                &format!("{RDFS}subClassOf"),
                &format!("{KG}System")
            ),
            "{class} is rooted in the core system class"
        );
    }
}
