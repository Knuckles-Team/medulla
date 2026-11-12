//! Row-level spec vocabulary in the SDD ontology (MEDULLA-SDLC-R014.10): the
//! eight row classes extend classes that already roll up to the core, the
//! twelve row properties exist, `implementedBy` keeps its domain, and the
//! `sdd` source imports nothing.

mod sdd_pack_support;

use medulla_core::ontology_codegen::refuse_modules_importing_core;
use medulla_graph_core::graph::GraphSchemaSources;
use sdd_pack_support::{declared_and_parents, hierarchy, registered, BFO, KG};

const SDD: &str = "core:sdd@1";

// spec: MEDULLA-SDLC-R014.10
#[test]
fn row_classes_extend_core_and_none_imports_core() {
    let sources = GraphSchemaSources::default();
    let document = registered(&sources, SDD);
    refuse_modules_importing_core([(SDD, document.as_str())])
        .expect("the sdd source does not import the core document");

    let (declared, parents) = declared_and_parents(&document);
    let hierarchy = hierarchy(&sources);
    let expected = [
        ("BoundTest", format!("{KG}TestCase")),
        ("SeamSide", format!("{KG}Specification")),
        ("ShapeStatement", format!("{KG}Specification")),
        ("RefusalCode", format!("{KG}GlossaryTerm")),
        ("RowStatus", format!("{BFO}0000031")),
        ("SpecifiedStatus", format!("{KG}RowStatus")),
        ("LandedStatus", format!("{KG}RowStatus")),
        ("VerifiedStatus", format!("{KG}RowStatus")),
    ];
    for (class, parent) in expected {
        let iri = format!("{KG}{class}");
        assert!(declared.contains(&iri), "class {class} is declared");
        assert!(
            parents.get(&iri).is_some_and(|set| set.contains(&parent)),
            "class {class} extends {parent}"
        );
        assert!(hierarchy.reaches_core(&iri), "class {class} reaches core");
    }
    assert!(hierarchy.refuse_unrooted(&document).is_ok());

    for property in [
        "rowId",
        "rowStatus",
        "dependsOnRow",
        "seamProvider",
        "seamConsumer",
        "boundTest",
        "verifiedBy",
        "refusesWith",
        "statedBy",
        "usesTerm",
        "commitHash",
        "implementedByCommit",
    ] {
        assert!(
            document.contains(&format!(":{property} a owl:")),
            "property {property} is declared"
        );
    }
    let implemented_by = document
        .split(":implementedBy a owl:ObjectProperty")
        .nth(1)
        .and_then(|rest| rest.split("\n\n").next())
        .expect("implementedBy is declared");
    assert!(implemented_by.contains("rdfs:domain :SoftwareFeature"));
}
