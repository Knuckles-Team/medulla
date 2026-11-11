//! Placement target vocabulary and shapes contract (MEDULLA-PLACEMENT-R001.5).
//!
//! The registered `core:placement@1` source carries the `placement:Target`
//! class and the four per-target facts, and its shapes require all four on
//! every target, exactly once each: a complete target passes, a target
//! missing any one fact is refused.

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_rdf::mapping::parse_turtle;
use medulla_rdf::oxrdf::{NamedNode, Triple};

const KG: &str = "http://knuckles.team/kg#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";
const SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const SHACL_TARGET_CLASS: &str = "http://www.w3.org/ns/shacl#targetClass";
const PLACEMENT: &str = "http://knuckles.team/kg/placement#";
const EX: &str = "http://knuckles.team/test/placement#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

fn triple(subject: &str, predicate: &str, object: &str) -> Triple {
    Triple::new(
        NamedNode::new(subject).unwrap(),
        NamedNode::new(predicate).unwrap(),
        NamedNode::new(object).unwrap(),
    )
}

fn placement_source() -> (String, String) {
    let sources = GraphSchemaSources::default();
    sources
        .validate()
        .expect("the default core catalog validates");
    let source = &sources.core["core:placement@1"];
    let ontology = source
        .ontology_ttl
        .clone()
        .expect("the placement module carries an ontology document")
        .to_string();
    let shapes = source
        .shapes_ttl
        .clone()
        .expect("the placement module carries its shapes companion")
        .to_string();
    (ontology, shapes)
}

// spec: MEDULLA-PLACEMENT-R001.5
#[test]
fn missing_relative_cost_fails_validation() {
    let (ontology, shapes) = placement_source();
    // The registered vocabulary declares the target class, rooted in the core
    // foundation, and the four facts it reads.
    let declared = parse_turtle(&ontology).expect("the placement vocabulary parses");
    assert!(
        declared.contains(&triple(&format!("{PLACEMENT}Target"), RDF_TYPE, OWL_CLASS)),
        "placement:Target is a declared class"
    );
    assert!(
        declared.contains(&triple(
            &format!("{PLACEMENT}Target"),
            SUBCLASS_OF,
            &format!("{KG}DomainEntity")
        )),
        "the target class reaches the core foundation"
    );
    for fact in [
        "supportsHa",
        "supportsVolumes",
        "maxReplicas",
        "relativeCost",
    ] {
        assert!(
            declared.contains(&triple(
                &format!("{PLACEMENT}{fact}"),
                RDF_TYPE,
                "http://www.w3.org/2002/07/owl#DatatypeProperty"
            )),
            "the vocabulary declares the {fact} fact as a datatype property"
        );
    }
    // The registered shape targets every placement target.
    let shape_triples = parse_turtle(&shapes).expect("the placement shapes parse");
    assert!(
        shape_triples.contains(&triple(
            &format!("{PLACEMENT}TargetShape"),
            SHACL_TARGET_CLASS,
            &format!("{PLACEMENT}Target")
        )),
        "the shape targets the placement target class"
    );

    // A target that declares all four facts conforms.
    let complete = medulla_shacl::validate_turtle(
        &shapes,
        &format!(
            "@prefix placement: <{PLACEMENT}> .\n\
             @prefix ex: <{EX}> .\n\
             @prefix xsd: <{XSD}> .\n\
             ex:helm a placement:Target ;\n\
                 placement:supportsHa true ;\n\
                 placement:supportsVolumes true ;\n\
                 placement:maxReplicas \"32\"^^xsd:nonNegativeInteger ;\n\
                 placement:relativeCost \"5\"^^xsd:nonNegativeInteger .\n"
        ),
    )
    .expect("the registered shapes and the complete target parse");
    assert!(
        complete.conforms,
        "a target with all four facts conforms: {:?}",
        complete.results
    );

    // The same target without its relative cost is refused, and the refusal
    // names the missing fact.
    let missing = medulla_shacl::validate_turtle(
        &shapes,
        &format!(
            "@prefix placement: <{PLACEMENT}> .\n\
             @prefix ex: <{EX}> .\n\
             @prefix xsd: <{XSD}> .\n\
             ex:helm a placement:Target ;\n\
                 placement:supportsHa true ;\n\
                 placement:supportsVolumes true ;\n\
                 placement:maxReplicas \"32\"^^xsd:nonNegativeInteger .\n"
        ),
    )
    .expect("the registered shapes and the cost-less target parse");
    assert!(
        !missing.conforms,
        "a target missing its relative cost must be refused"
    );
    assert!(
        missing.results.iter().any(|result| {
            result
                .path
                .as_deref()
                .map(|path| path.trim_matches(['<', '>']))
                == Some(&format!("{PLACEMENT}relativeCost"))
        }),
        "the refusal names the relativeCost path: {:?}",
        missing.results
    );
}
