//! Knowledge-factory vocabulary (MEDULLA-ORCH-VOCAB-R817): seven classes that roll up to
//! the core entity, ten links with a declared domain and range, and one shape per class.

mod sdd_pack_support;

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_shacl::validate_turtle;
use sdd_pack_support::{declared_and_parents, hierarchy, registered, BFO, KG};

const SOURCE: &str = "core:orchestration@1";
const SHAPES: &str = "core:orchestration-shapes@1";

const CLASSES: [&str; 7] = [
    "KnowledgeFlag",
    "CurationProposal",
    "FrictionReport",
    "FrictionStep",
    "SkillInvocation",
    "SkillEvalCase",
    "Persona",
];

// spec: MEDULLA-ORCH-VOCAB-R817.1
#[test]
fn seven_classes_roll_up_to_the_core_entity() {
    let sources = GraphSchemaSources::default();
    let document = registered(&sources, SOURCE);
    let (declared, parents) = declared_and_parents(&document);
    let hierarchy = hierarchy(&sources);
    for class in CLASSES {
        let iri = format!("{KG}{class}");
        assert!(declared.contains(&iri), "class {class} is declared");
        assert!(
            parents
                .get(&iri)
                .is_some_and(|set| set.contains(&format!("{BFO}0000001"))),
            "class {class} extends bfo:0000001, as :TaskStep does"
        );
        assert!(hierarchy.reaches_core(&iri), "class {class} reaches core");
        assert!(
            document.contains(&format!(":{class} a owl:Class ;\n    rdfs:label")),
            "class {class} carries a label"
        );
    }
    // Each comment names the owning decision-engine row.
    for class in CLASSES {
        let start = document
            .find(&format!(":{class} a owl:Class"))
            .expect("declared");
        let block = &document[start..start + document[start..].find(" .\n").unwrap()];
        assert!(
            block.contains("MEDULLA-DECISION-ENGINE-R7"),
            "the comment of {class} names its decision-engine row"
        );
    }
}

// spec: MEDULLA-ORCH-VOCAB-R817.2
#[test]
fn properties_have_domain_and_range() {
    let sources = GraphSchemaSources::default();
    let document = registered(&sources, SOURCE);
    for property in [
        "flags",
        "knowledgeObservedIn",
        "raisedBy",
        "knowledgeResolvedBy",
        "addresses",
        "curationProposes",
        "knowledgeFixedBy",
        "about",
        "scoredBy",
        "includes",
    ] {
        let start = document
            .find(&format!(":{property} a owl:ObjectProperty"))
            .unwrap_or_else(|| panic!("property {property} is declared"));
        let block = &document[start..start + document[start..].find(" .\n").unwrap()];
        assert!(
            block.contains("rdfs:domain :"),
            "{property} has a declared domain"
        );
        assert!(
            block.contains("rdfs:range "),
            "{property} has a declared range"
        );
    }
}

// spec: MEDULLA-ORCH-VOCAB-R817.3
#[test]
fn shapes_reject_free_text_on_an_invocation() {
    let sources = GraphSchemaSources::default();
    let shapes = sources
        .shapes()
        .find(|(id, _)| *id == SHAPES)
        .expect("orchestration carries shapes")
        .1
        .to_string();
    for class in CLASSES {
        assert!(
            shapes.contains(&format!("sh:targetClass :{class} ;")),
            "class {class} has a shape"
        );
    }
    let report = |facts: &str| {
        let data =
            format!("@prefix eg: <{KG}> .\n@prefix ex: <http://example.org/d/> .\n\n{facts}\n");
        validate_turtle(&shapes, &data).expect("knowledge-factory Turtle validates")
    };
    let good_flag = report(
        "ex:f a eg:KnowledgeFlag ; eg:evidence_class \"agent_asserted\" ; eg:target_ref \"topic:x\" ; eg:knowledgeObservedIn ex:t .",
    );
    assert!(good_flag.conforms, "{:?}", good_flag.results);
    let human_flag = report(
        "ex:f a eg:KnowledgeFlag ; eg:evidence_class \"independent\" ; eg:target_ref \"topic:x\" ; eg:knowledgeObservedIn ex:t .",
    );
    assert!(!human_flag.conforms, "a flag is agent-asserted only");
    let untargeted = report(
        "ex:f a eg:KnowledgeFlag ; eg:evidence_class \"agent_asserted\" ; eg:target_ref \"term:x\" ; eg:knowledgeObservedIn ex:t .",
    );
    assert!(
        !untargeted.conforms,
        "a flag with no :flags and no topic: target fails"
    );
    let good_invocation = report(
        "ex:i a eg:SkillInvocation ; eg:invocation_id \"i1\" ; eg:chosen_by \"agent\" ; eg:outcome \"ok\" ; eg:tokens_in 10 .",
    );
    assert!(good_invocation.conforms, "{:?}", good_invocation.results);
    let free_text = report(
        "ex:i a eg:SkillInvocation ; eg:invocation_id \"i1\" ; eg:prompt \"please summarise my notes\" .",
    );
    assert!(
        !free_text.conforms,
        "a literal outside the typed record fails the invocation shape"
    );
    let friction = report("ex:r a eg:FrictionReport .");
    assert!(!friction.conforms, "a friction report needs one :about");
}

// spec: MEDULLA-ORCH-VOCAB-R817.4
#[test]
fn skill_health_terms_are_declared() {
    let sources = GraphSchemaSources::default();
    let document = registered(&sources, SOURCE);
    let (declared, parents) = declared_and_parents(&document);
    let hierarchy = hierarchy(&sources);
    for class in ["SkillEvalReport", "FrictionGroup"] {
        let iri = format!("{KG}{class}");
        assert!(declared.contains(&iri), "class {class} is declared");
        assert!(
            parents
                .get(&iri)
                .is_some_and(|set| set.contains(&format!("{BFO}0000001"))),
            "class {class} extends bfo:0000001"
        );
        assert!(hierarchy.reaches_core(&iri), "class {class} reaches core");
    }
    for property in ["evaluates", "frictionOf"] {
        let start = document
            .find(&format!(":{property} a owl:ObjectProperty"))
            .unwrap_or_else(|| panic!("property {property} is declared"));
        let block = &document[start..start + document[start..].find(" .\n").unwrap()];
        assert!(block.contains("rdfs:domain :") && block.contains("rdfs:range "));
    }
    for property in [
        "flagStatus",
        "evaluated_skill",
        "suite",
        "pass_count",
        "fail_count",
        "case_count",
        "pass_rate_ppm",
        "band",
        "receipt_digest",
        "evaluated_ms",
    ] {
        assert!(
            document.contains(&format!(":{property} a owl:DatatypeProperty")),
            "datatype property {property} is declared"
        );
    }
}
