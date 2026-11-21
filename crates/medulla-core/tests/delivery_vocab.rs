//! Delivery vocabulary of the `sdlc_lifecycle` source (MEDULLA-SDLC-R019.1): eight
//! classes that each extend a core class and carry one SHACL shape, the links and the
//! datatype properties.

mod sdd_pack_support;

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_shacl::validate_turtle;
use sdd_pack_support::{declared_and_parents, hierarchy, registered, BFO, KG};

const SOURCE: &str = "core:sdlc_lifecycle@1";

fn shapes(sources: &GraphSchemaSources) -> String {
    sources
        .shapes()
        .find(|(id, _)| *id == SOURCE)
        .expect("sdlc_lifecycle carries shapes")
        .1
        .to_string()
}

// spec: MEDULLA-SDLC-R019.1
#[test]
fn delivery_classes_extend_core_and_are_shaped() {
    let sources = GraphSchemaSources::default();
    let document = registered(&sources, SOURCE);
    let shapes = shapes(&sources);
    let (declared, parents) = declared_and_parents(&document);
    let hierarchy = hierarchy(&sources);
    let expected = [
        ("Delivery", format!("{KG}Ticket")),
        ("DeliveryAssignment", format!("{BFO}0000015")),
        ("FeedbackEvent", format!("{BFO}0000015")),
        ("Review", format!("{BFO}0000015")),
        ("MergeArming", format!("{BFO}0000015")),
        ("DeliveryReport", format!("{BFO}0000031")),
        ("DeliveryMandate", format!("{BFO}0000031")),
        ("ReviewThread", format!("{BFO}0000031")),
    ];
    for (class, parent) in expected {
        let iri = format!("{KG}{class}");
        assert!(declared.contains(&iri), "class {class} is declared");
        assert!(
            parents.get(&iri).is_some_and(|set| set.contains(&parent)),
            "class {class} extends {parent}"
        );
        assert!(hierarchy.reaches_core(&iri), "class {class} reaches core");
        assert!(
            shapes.contains(&format!("sh:targetClass :{class} .")),
            "class {class} has a shape"
        );
    }
    for property in [
        "deliversRequirement",
        "opensProposal",
        "followsProposal",
        "assignedRun",
        "checkRunOf",
        "assignmentOf",
        "reportOf",
        "mandateOf",
        "feedbackFor",
        "feedbackOn",
        "reviewOf",
        "threadOf",
        "armingOf",
    ] {
        assert!(
            document.contains(&format!(":{property} a owl:ObjectProperty")),
            "link {property} is declared"
        );
    }
    for property in [
        "armedAtHead",
        "deliveryOutcome",
        "reportState",
        "feedbackKind",
        "reviewerKind",
    ] {
        assert!(
            document.contains(&format!(":{property} a owl:DatatypeProperty")),
            "property {property} is declared"
        );
    }

    let report = |facts: &str| {
        let data =
            format!("@prefix eg: <{KG}> .\n@prefix ex: <http://example.org/d/> .\n\n{facts}\n");
        validate_turtle(&shapes, &data).expect("delivery Turtle validates")
    };
    let good = report(
        "ex:d a eg:Delivery ; eg:deliversRequirement ex:r ; eg:deliveryOutcome \"merged\" .\n\
         ex:r a eg:Requirement .\n\
         ex:f a eg:FeedbackEvent ; eg:feedbackKind \"comment\" .",
    );
    assert!(good.conforms, "{:?}", good.results);
    let bad_kind = report("ex:f a eg:FeedbackEvent ; eg:feedbackKind \"a\", \"b\" .");
    assert!(!bad_kind.conforms, "two feedback kinds must fail the shape");
    let bad_requirement = report("ex:d a eg:Delivery ; eg:deliversRequirement ex:untyped .");
    assert!(
        !bad_requirement.conforms,
        "a delivered node that is no requirement must fail the shape"
    );
}
