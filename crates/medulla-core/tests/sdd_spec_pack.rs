//! The spec-driven development ontology pack (MEDULLA-SDLC-R014.5): twelve
//! classes that each extend a core class, six properties, no `owl:imports` of
//! the core document, and the `PACK_CLASS_UNROOTED` refusal.

mod sdd_pack_support;

use medulla_core::ontology_codegen::refuse_modules_importing_core;
use medulla_graph_core::graph::GraphSchemaSources;
use sdd_pack_support::{declared_and_parents, hierarchy, registered};

const PACK: &str = "core:sdd_spec@1";
const PACK_NS: &str = "http://knuckles.team/kg/sdd-spec#";
const CLASSES: [&str; 12] = [
    "Spec",
    "Requirement",
    "PlanSection",
    "Task",
    "Contract",
    "ChecklistItem",
    "ConstitutionPrinciple",
    "Goal",
    "Proposal",
    "Phase",
    "ReleaseBundle",
    "ProjectMode",
];
const PROPERTIES: [&str; 6] = [
    "dependsOn",
    "crossesSeam",
    "servesPhase",
    "governedBy",
    "inBundle",
    "targetsGoal",
];

// spec: MEDULLA-SDLC-R014.5
#[test]
fn every_class_extends_a_core_class_and_none_imports_core() {
    let sources = GraphSchemaSources::default();
    let document = registered(&sources, PACK);
    refuse_modules_importing_core([(PACK, document.as_str())])
        .expect("the pack does not import the core document");

    let (declared, parents) = declared_and_parents(&document);
    let hierarchy = hierarchy(&sources);
    for class in CLASSES {
        let iri = format!("{PACK_NS}{class}");
        assert!(declared.contains(&iri), "class {class} is declared");
        assert!(
            parents.get(&iri).is_some_and(|set| !set.is_empty()),
            "class {class} names an rdfs:subClassOf parent"
        );
        assert!(
            hierarchy.reaches_core(&iri),
            "class {class} has no path to a core class"
        );
    }
    assert_eq!(
        declared.len(),
        CLASSES.len(),
        "the pack declares only these"
    );
    for property in PROPERTIES {
        assert!(
            document.contains(&format!("sddspec:{property} a owl:ObjectProperty")),
            "property {property} is declared"
        );
    }
    assert_eq!(hierarchy.refuse_unrooted(&document), Ok(CLASSES.len()));
}

// spec: MEDULLA-SDLC-R014.5
#[test]
fn a_class_without_a_core_parent_is_refused_with_pack_class_unrooted() {
    let sources = GraphSchemaSources::default();
    let hierarchy = hierarchy(&sources);
    let orphan = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
                  @prefix p: <http://example.org/p#> .\n\
                  p:Orphan a owl:Class .\n";
    let refusal = hierarchy.refuse_unrooted(orphan).unwrap_err();
    assert!(refusal.starts_with("PACK_CLASS_UNROOTED: class http://example.org/p#Orphan"));
    let dangling = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
                    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                    @prefix p: <http://example.org/p#> .\n\
                    p:Island a owl:Class ; rdfs:subClassOf p:Nowhere .\n";
    assert!(hierarchy
        .refuse_unrooted(dangling)
        .unwrap_err()
        .starts_with("PACK_CLASS_UNROOTED"));
}
