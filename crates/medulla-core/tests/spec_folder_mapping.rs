//! Spec folder node kinds (MEDULLA-ORCH-VOCAB-R012.1).

use medulla_core::spec_folder::SpecNodeKind;

const ONTOLOGY: &str = include_str!("../ontology/sdd-v1.ttl");
const SHAPES: &str = include_str!("../ontology/sdd-v1.shapes.ttl");

// spec: MEDULLA-ORCH-VOCAB-R012.1
#[test]
fn node_kinds_are_closed_and_declared() {
    let names: Vec<&str> = SpecNodeKind::ALL.iter().map(|k| k.wire_name()).collect();
    assert_eq!(names, ["task", "goal", "document", "memory", "policy"]);
    for kind in SpecNodeKind::ALL {
        assert_eq!(SpecNodeKind::from_wire(kind.wire_name()), Some(kind));
    }
    assert_eq!(SpecNodeKind::from_wire("widget"), None);

    for (class, parent) in [
        ("SpecTask", "Action"),
        ("SpecGoal", "Goal"),
        ("SpecMemory", "Memory"),
        ("SpecPolicy", "Policy"),
    ] {
        let declaration = format!(":{class} a owl:Class ;");
        let at = ONTOLOGY
            .find(&declaration)
            .unwrap_or_else(|| panic!("{class} is declared"));
        let block = &ONTOLOGY[at..];
        let block = &block[..block.find(" .\n").unwrap()];
        assert!(block.contains(&format!("rdfs:subClassOf :{parent}")), "{class}");
        assert_eq!(block.matches("rdfs:subClassOf").count(), 1, "{class}");
        assert!(SHAPES.contains(&format!("sh:targetClass :{class} ;")), "{class}");
    }
    assert!(ONTOLOGY.contains(":classifiedByRule a owl:DatatypeProperty"));
    assert_eq!(SHAPES.matches("sh:path :classifiedByRule").count(), 4);
    assert_eq!(SHAPES.matches("sh:maxCount 1").count(), 4);
    assert_eq!(SpecNodeKind::Document.class_name(), "Specification");
}
