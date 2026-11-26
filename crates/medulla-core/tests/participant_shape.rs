//! The core `:Participant` class (MEDULLA-DECISION-ENGINE-R617.7): it extends the parent
//! `:Person` and `:Agent` share, and its shape requires a person or an agent to carry
//! `:represents`.

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_rdf::mapping::parse_turtle;
use medulla_rdf::oxrdf::{NamedOrBlankNode, Term};

const KG: &str = "https://knuckles-team.github.io/ontology/core#";
const BFO_CONTINUANT: &str = "http://purl.obolibrary.org/obo/BFO_0000004";
const RDFS_SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";

fn shapes() -> String {
    let sources = GraphSchemaSources::default();
    sources
        .core
        .get("core:foundation-shapes@1")
        .expect("the core foundation shapes are registered")
        .shapes_ttl
        .clone()
        .expect("the core foundation source carries shapes")
        .to_string()
}

fn parents_of(class: &str) -> Vec<String> {
    let sources = GraphSchemaSources::default();
    let ontology = sources
        .ontologies()
        .find(|(id, _)| *id == "core:foundation@1")
        .expect("the core foundation ontology is registered")
        .1
        .to_string();
    parse_turtle(&ontology)
        .expect("the core foundation ontology parses")
        .into_iter()
        .filter_map(|triple| match (&triple.subject, triple.predicate.as_str(), &triple.object) {
            (NamedOrBlankNode::NamedNode(subject), RDFS_SUBCLASS_OF, Term::NamedNode(parent))
                if subject.as_str() == class =>
            {
                Some(parent.as_str().to_string())
            }
            _ => None,
        })
        .collect()
}

fn data(kind: &str, represents: Option<&str>) -> String {
    let represents = represents
        .map(|node| format!("    :represents <{node}> ;\n"))
        .unwrap_or_default();
    format!(
        "@prefix : <{KG}> .\n<urn:medulla:participant:ws-1:ada> a :Participant ;\n{represents}    :participantKind \"{kind}\" .\n"
    )
}

fn conforms(data: &str) -> bool {
    medulla_shacl::validate_turtle(&shapes(), data)
        .expect("validation runs")
        .conforms
}

// spec: MEDULLA-DECISION-ENGINE-R617.7
#[test]
fn participant_extends_the_shared_parent() {
    let parents = parents_of(&format!("{KG}Participant"));
    assert_eq!(parents, [BFO_CONTINUANT.to_string()]);
    // Neither :Person nor :Agent extends the other, and neither extends :Participant.
    assert!(!parents_of(&format!("{KG}Person")).contains(&format!("{KG}Agent")));
    assert!(!parents_of(&format!("{KG}Agent")).contains(&format!("{KG}Person")));
    assert!(!parents_of(&format!("{KG}Person")).contains(&format!("{KG}Participant")));
}

// spec: MEDULLA-DECISION-ENGINE-R617.7
#[test]
fn person_without_represents_fails_shape() {
    let node = "urn:medulla:person:ada";
    assert!(conforms(&data("person", Some(node))));
    assert!(conforms(&data("agent", Some("urn:medulla:agent:planner"))));
    assert!(conforms(&data("object", None)));
    assert!(!conforms(&data("person", None)), "a person must represent a node");
    assert!(!conforms(&data("agent", None)), "an agent must represent a node");
    assert!(!conforms(&data("robot", Some(node))), "the kind is one of three values");
}
