//! The process and decision vocabulary's core roll-up (MEDULLA-ORCH-VOCAB-R014).
#![cfg(feature = "rdf")]

mod common;

use std::collections::BTreeSet;

use common::{parse_all, BFO, KG, RDFS};
use medulla_rdf::oxrdf::{NamedOrBlankNode, Term, Triple};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";

const FOUNDATION: &str = include_str!("../../medulla-core/ontology/core-foundation-v1.ttl");
const DMN: &str = include_str!("../../medulla-core/ontology/dmn-v1.ttl");
const ORCHESTRATION: &str = include_str!("../../medulla-core/ontology/orchestration-v1.ttl");
const ORCHESTRATION_SHAPES: &str =
    include_str!("../../medulla-core/ontology/orchestration-v1.shapes.ttl");
const BPMN: &str = include_str!("../../medulla-core/ontology/bpmn-v1.ttl");
const BPMN_SHAPES: &str = include_str!("../../medulla-core/ontology/bpmn-v1.shapes.ttl");
const ENTERPRISE: &str = include_str!("../../medulla-core/ontology/enterprise-v1.ttl");
const PROCESS_INTELLIGENCE: &str =
    include_str!("../../medulla-core/ontology/process_intelligence-v1.ttl");
const CAMUNDA_PACK: &str =
    include_str!("../../medulla-core/tests/fixtures/process_vocabulary/camunda_pack.ttl");
const ARIS_PACK: &str =
    include_str!("../../medulla-core/tests/fixtures/process_vocabulary/aris_pack.ttl");

const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const OWL_UNION_OF: &str = "http://www.w3.org/2002/07/owl#unionOf";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

fn named(triple: &Triple) -> Option<&str> {
    match &triple.subject {
        NamedOrBlankNode::NamedNode(node) => Some(node.as_str()),
        _ => None,
    }
}

fn parents<'a>(triples: &'a [Triple], class: &str) -> BTreeSet<&'a str> {
    triples
        .iter()
        .filter(|t| named(t) == Some(class) && t.predicate.as_str() == format!("{RDFS}subClassOf"))
        .filter_map(|t| match &t.object {
            Term::NamedNode(node) => Some(node.as_str()),
            _ => None,
        })
        .collect()
}

// spec: MEDULLA-ORCH-VOCAB-R014.3
#[test]
fn every_dmn_class_has_one_core_parent() {
    let triples = parse_all(&[FOUNDATION, DMN]);
    let foundation = parse_all(&[FOUNDATION]);
    let declared_in_foundation = |class: &str| {
        foundation.iter().any(|t| {
            named(t) == Some(class)
                && t.predicate.as_str() == RDF_TYPE
                && matches!(&t.object, Term::NamedNode(node) if node.as_str() == OWL_CLASS)
        })
    };
    let dmn_classes: BTreeSet<String> = parse_all(&[DMN])
        .iter()
        .filter(|t| {
            t.predicate.as_str() == RDF_TYPE
                && matches!(&t.object, Term::NamedNode(node) if node.as_str() == OWL_CLASS)
        })
        .filter_map(|t| named(t).map(str::to_owned))
        .collect();
    assert_eq!(dmn_classes.len(), 7, "{dmn_classes:?}");
    for class in &dmn_classes {
        assert!(class.starts_with(&format!("{KG}Dmn")), "{class}");
        let parents = parents(&triples, class);
        assert_eq!(parents.len(), 1, "{class} has parents {parents:?}");
        let parent = *parents.iter().next().unwrap();
        // The parent is a core class, or the one dmn class that is itself rooted in one.
        assert!(
            declared_in_foundation(parent) || dmn_classes.contains(parent),
            "{class} extends {parent}, which is not a core class"
        );
    }
    for (class, parent) in [
        ("DmnDecisionRequirements", "DecisionLogic"),
        ("DmnDecision", "DecisionLogic"),
        ("DmnDecisionTable", "DecisionLogic"),
        ("DmnRule", "DecisionRule"),
        ("DmnInputClause", "Concept"),
        ("DmnOutputClause", "Concept"),
        ("DmnInputData", "Concept"),
    ] {
        assert_eq!(
            parents(&triples, &format!("{KG}{class}")),
            BTreeSet::from([format!("{KG}{parent}").as_str()])
        );
    }
}

fn objects<'a>(triples: &'a [Triple], subject: &str, predicate: &str) -> Vec<&'a Term> {
    triples
        .iter()
        .filter(|t| named(t) == Some(subject) && t.predicate.as_str() == predicate)
        .map(|t| &t.object)
        .collect()
}

/// The class IRIs an `rdfs:domain` or `rdfs:range` value names: the IRI itself, or the
/// members of its `owl:unionOf` list.
fn members(triples: &[Triple], value: &Term) -> BTreeSet<String> {
    match value {
        Term::NamedNode(node) => BTreeSet::from([node.as_str().to_owned()]),
        Term::BlankNode(blank) => {
            let mut out = BTreeSet::new();
            let mut cell = triples
                .iter()
                .find(|t| {
                    matches!(&t.subject, NamedOrBlankNode::BlankNode(b) if b == blank)
                        && t.predicate.as_str() == OWL_UNION_OF
                })
                .map(|t| t.object.clone());
            while let Some(Term::BlankNode(list)) = cell {
                let of = |predicate: &str| {
                    triples
                        .iter()
                        .find(|t| {
                            matches!(&t.subject, NamedOrBlankNode::BlankNode(b) if *b == list)
                                && t.predicate.as_str() == predicate
                        })
                        .map(|t| t.object.clone())
                };
                if let Some(Term::NamedNode(node)) = of(RDF_FIRST) {
                    out.insert(node.as_str().to_owned());
                }
                cell = of(RDF_REST);
            }
            out
        }
        other => panic!("unexpected domain or range {other:?}"),
    }
}

fn domain_and_range(triples: &[Triple], property: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let collect = |predicate: &str| {
        objects(triples, property, predicate)
            .into_iter()
            .flat_map(|value| members(triples, value))
            .collect::<BTreeSet<String>>()
    };
    (collect(RDFS_DOMAIN), collect(RDFS_RANGE))
}

fn core(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| format!("{KG}{n}")).collect()
}

fn xsd(name: &str) -> BTreeSet<String> {
    BTreeSet::from([format!("{XSD}{name}")])
}

fn declared_class(triples: &[Triple], class: &str) -> bool {
    triples.iter().any(|t| {
        named(t) == Some(class)
            && t.predicate.as_str() == RDF_TYPE
            && matches!(&t.object, Term::NamedNode(node) if node.as_str() == OWL_CLASS)
    })
}

// spec: MEDULLA-ORCH-VOCAB-R014.1
#[test]
fn core_process_terms_resolve() {
    let triples = parse_all(&[FOUNDATION, ENTERPRISE, ORCHESTRATION]);
    // Five classes, each under its core parent.
    for (class, parent) in [
        ("ProcessStep", "TaskStep"),
        ("ProcessGate", "TaskStep"),
        ("ProcessEventNode", "TaskStep"),
        ("ProcessSequenceFlow", "Concept"),
        ("ProcessLane", "Concept"),
    ] {
        let iri = format!("{KG}{class}");
        assert!(declared_class(&triples, &iri), "{class} is declared");
        assert_eq!(
            parents(&triples, &iri),
            BTreeSet::from([format!("{KG}{parent}").as_str()]),
            "{class}"
        );
    }
    // Seven properties, each with its domain and range ("no range" is an empty set).
    let none = BTreeSet::new;
    for (property, domain, range) in [
        ("flowSource", core(&["ProcessSequenceFlow"]), core(&["TaskStep"])),
        ("flowTarget", core(&["ProcessSequenceFlow"]), core(&["TaskStep"])),
        ("flowCondition", core(&["ProcessSequenceFlow"]), xsd("string")),
        ("isDefaultFlow", core(&["ProcessSequenceFlow"]), xsd("boolean")),
        ("inLane", core(&["TaskStep"]), core(&["ProcessLane"])),
        ("hasProcessElement", core(&["ProcessModel"]), none()),
        ("eventPosition", core(&["ProcessEventNode"]), xsd("string")),
    ] {
        assert_eq!(
            domain_and_range(&triples, &format!("{KG}{property}")),
            (domain, range),
            "{property}"
        );
    }
    // :eventPosition is closed to four values.
    let planted = |value: &str| {
        let data = format!(
            "@prefix : <{KG}> .\n:event a :ProcessEventNode ; :eventPosition \"{value}\" .\n"
        );
        medulla_shacl::validate_turtle(ORCHESTRATION_SHAPES, &data).expect("validates")
    };
    for value in ["start", "end", "intermediate", "boundary"] {
        assert!(planted(value).conforms, "{value}");
    }
    assert!(!planted("middle").conforms);
}

// spec: MEDULLA-ORCH-VOCAB-R014.2
#[test]
fn every_bpmn_class_has_one_core_parent() {
    let triples = parse_all(&[FOUNDATION, ENTERPRISE, ORCHESTRATION, BPMN]);
    let bpmn_classes: BTreeSet<String> = parse_all(&[BPMN])
        .iter()
        .filter(|t| {
            t.predicate.as_str() == RDF_TYPE
                && matches!(&t.object, Term::NamedNode(node) if node.as_str() == OWL_CLASS)
        })
        .filter_map(|t| named(t).map(str::to_owned))
        .collect();
    let roots = core(&[
        "ProcessStep",
        "ProcessGate",
        "ProcessEventNode",
        "ProcessSequenceFlow",
        "ProcessLane",
        "ProcessModel",
        "Concept",
        "TaskStep",
    ]);
    let mut roots = roots;
    roots.insert(format!("{BFO}0000031"));
    for class in &bpmn_classes {
        assert!(class.starts_with(&format!("{KG}Bpmn")), "{class}");
        let reached: BTreeSet<&String> = ancestors(&triples, class)
            .into_iter()
            .filter_map(|a| roots.get(&a))
            .collect();
        // The abstract flow element and node classes sit beside their process root.
        assert!(!reached.is_empty(), "{class} reaches no core process term");
    }
    for (class, root) in [
        ("BpmnTask", "ProcessStep"),
        ("BpmnUserTask", "ProcessStep"),
        ("BpmnBusinessRuleTask", "ProcessStep"),
        ("BpmnSubProcess", "ProcessStep"),
        ("BpmnCallActivity", "ProcessStep"),
        ("BpmnGateway", "ProcessGate"),
        ("BpmnComplexGateway", "ProcessGate"),
        ("BpmnEvent", "ProcessEventNode"),
        ("BpmnBoundaryEvent", "ProcessEventNode"),
        ("BpmnSequenceFlow", "ProcessSequenceFlow"),
        ("BpmnLane", "ProcessLane"),
        ("BpmnProcess", "ProcessModel"),
        ("BpmnDefinitions", "ProcessModel"),
        ("BpmnTimerEventDefinition", "BpmnEventDefinition"),
    ] {
        assert!(
            ancestors(&triples, &format!("{KG}{class}")).contains(&format!("{KG}{root}")),
            "{class} is under {root}"
        );
    }
    assert!(bpmn_classes.contains(&format!("{KG}BpmnEventDefinition")));
    let event_definitions = bpmn_classes
        .iter()
        .filter(|c| ancestors(&triples, c).contains(&format!("{KG}BpmnEventDefinition")))
        .count();
    assert_eq!(event_definitions, 8);
    // The event link and the two source-preservation properties resolve.
    let all = parse_all(&[BPMN]);
    assert_eq!(
        domain_and_range(&all, &format!("{KG}hasEventDefinition")),
        (core(&["BpmnEvent"]), core(&["BpmnEventDefinition"]))
    );
    for property in ["bpmnElementId", "bpmnPreservedXml"] {
        assert_eq!(
            domain_and_range(&all, &format!("{KG}{property}")).1,
            xsd("string")
        );
    }
}

fn ancestors(triples: &[Triple], class: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut pending = vec![class.to_owned()];
    while let Some(next) = pending.pop() {
        for parent in parents(triples, &next) {
            if seen.insert(parent.to_owned()) {
                pending.push(parent.to_owned());
            }
        }
    }
    seen
}

// spec: MEDULLA-ORCH-VOCAB-R014.4
#[test]
fn step_binding_properties_resolve() {
    let triples = parse_all(&[FOUNDATION, ENTERPRISE, ORCHESTRATION]);
    let step = core(&["ProcessStep"]);
    for (property, domain, range) in [
        ("evaluatesDecision", step.clone(), core(&["DecisionLogic"])),
        ("approverRole", step.clone(), xsd("string")),
        ("toolArguments", step.clone(), xsd("string")),
        ("orchestrationUsesTool", core(&["Agent", "ProcessStep"]), core(&["Tool"])),
        ("hasPrompt", core(&["Agent", "ProcessStep"]), core(&["Prompt"])),
    ] {
        assert_eq!(
            domain_and_range(&triples, &format!("{KG}{property}")),
            (domain, range),
            "{property}"
        );
    }
    // The shared :usesTool carries no domain, and the lane names the agent kind.
    assert!(domain_and_range(&triples, &format!("{KG}usesTool")).0.is_empty());
    assert!(
        domain_and_range(&triples, &format!("{KG}hasAgentKind"))
            .0
            .contains(&format!("{KG}ProcessLane"))
    );
}

// spec: MEDULLA-ORCH-VOCAB-R014.9
#[test]
fn dag_node_kinds_are_closed_and_roll_up() {
    const KINDS: [&str; 13] = [
        "start", "end", "human", "agent", "decision", "branch", "fork", "join", "wait", "call",
        "boundary", "graph", "state",
    ];
    let core_docs = [FOUNDATION, ENTERPRISE, PROCESS_INTELLIGENCE, ORCHESTRATION, BPMN];
    for (pack, system) in [(CAMUNDA_PACK, "camunda"), (ARIS_PACK, "aris")] {
        let mut docs = core_docs.to_vec();
        docs.push(pack);
        let triples = parse_all(&docs);
        let core_classes: BTreeSet<String> = parse_all(&core_docs)
            .iter()
            .filter(|t| {
                t.predicate.as_str() == RDF_TYPE
                    && matches!(&t.object, Term::NamedNode(node) if node.as_str() == OWL_CLASS)
            })
            .filter_map(|t| named(t).map(str::to_owned))
            .collect();
        let annotated: Vec<(&str, String)> = triples
            .iter()
            .filter(|t| t.predicate.as_str().ends_with("#dagNodeKind"))
            .filter_map(|t| match (named(t), &t.object) {
                (Some(class), Term::Literal(value)) => Some((class, value.value().to_owned())),
                _ => None,
            })
            .collect();
        assert!(!annotated.is_empty(), "{system} annotates classes");
        for (class, value) in &annotated {
            assert!(KINDS.contains(&value.as_str()), "{class}: {value}");
            let reached = ancestors(&triples, class);
            assert!(
                reached.iter().any(|a| core_classes.contains(a)),
                "{system} class {class} reaches no core class"
            );
        }
    }
    // The core process classes the packs extend are annotated with the closed values only.
    let data = format!(
        "@prefix : <{KG}> .\n:Planted a <http://www.w3.org/2002/07/owl#Class> ; :dagNodeKind \"teleport\" .\n"
    );
    let refused = medulla_shacl::validate_turtle(ORCHESTRATION_SHAPES, &data).expect("validates");
    assert!(!refused.conforms);
    let gate = format!(
        "@prefix : <{KG}> .\n:Planted a <http://www.w3.org/2002/07/owl#Class> ; :canonicalGateKind \"nand\" .\n"
    );
    assert!(
        !medulla_shacl::validate_turtle(ORCHESTRATION_SHAPES, &gate)
            .expect("validates")
            .conforms
    );
    let valid = format!(
        "@prefix : <{KG}> .\n:Fine a <http://www.w3.org/2002/07/owl#Class> ; :dagNodeKind \"fork\" ; :canonicalGateKind \"or\" .\n"
    );
    assert!(
        medulla_shacl::validate_turtle(ORCHESTRATION_SHAPES, &valid)
            .expect("validates")
            .conforms
    );
}

// spec: MEDULLA-ORCH-VOCAB-R014.5
#[test]
fn bpmn_shapes_refuse_planted_faults() {
    const PREFIX: &str = "@prefix : <https://knuckles-team.github.io/ontology/core#> .\n";
    // A clean process: start -> gateway -> end, with one default flow, in one lane.
    let clean = format!(
        "{PREFIX}\
:model :hasProcessElement :start, :gate, :end, :f1, :f2, :f3 .\n\
:start a :BpmnStartEvent ; :bpmnElementId \"start\" ; :inLane :lane .\n\
:gate a :BpmnExclusiveGateway ; :bpmnElementId \"gate\" .\n\
:end a :BpmnEndEvent ; :bpmnElementId \"end\" .\n\
:lane a :BpmnLane ; :bpmnElementId \"lane\" .\n\
:f1 a :BpmnSequenceFlow ; :bpmnElementId \"f1\" ; :flowSource :start ; :flowTarget :gate .\n\
:f2 a :BpmnSequenceFlow ; :bpmnElementId \"f2\" ; :flowSource :gate ; :flowTarget :end ; :isDefaultFlow true .\n\
:f3 a :BpmnSequenceFlow ; :bpmnElementId \"f3\" ; :flowSource :gate ; :flowTarget :end .\n"
    );
    let report = |data: &str| medulla_shacl::validate_turtle(BPMN_SHAPES, data).expect("validates");
    let clean_report = report(&clean);
    assert!(clean_report.conforms, "{:?}", clean_report.results);

    // One planted fault per constraint; each is refused with its own message.
    let faults: [(&str, String); 7] = [
        (
            "exactly one flowSource",
            clean.replace(" ; :flowSource :start ; :flowTarget :gate", " ; :flowTarget :gate"),
        ),
        (
            "exactly one flowTarget",
            clean.replace(" ; :flowSource :start ; :flowTarget :gate", " ; :flowSource :start"),
        ),
        (
            "at most one outgoing default flow",
            clean.replace(":flowTarget :end .\n", ":flowTarget :end ; :isDefaultFlow true .\n"),
        ),
        (
            "start event has no incoming flow",
            format!("{clean}:f4 a :BpmnSequenceFlow ; :bpmnElementId \"f4\" ; :flowSource :gate ; :flowTarget :start .\n"),
        ),
        (
            "end event has no outgoing flow",
            format!("{clean}:f5 a :BpmnSequenceFlow ; :bpmnElementId \"f5\" ; :flowSource :end ; :flowTarget :gate .\n"),
        ),
        (
            "unique within its process model",
            clean.replace(":bpmnElementId \"end\"", ":bpmnElementId \"start\""),
        ),
        (
            "at most one lane",
            clean.replace(":inLane :lane .", ":inLane :lane, :lane2 ."),
        ),
    ];
    for (constraint, data) in &faults {
        assert_ne!(data, &clean, "{constraint}: the fault changes the data");
        let refused = report(data);
        assert!(!refused.conforms, "{constraint} is refused");
        assert!(
            refused
                .results
                .iter()
                .any(|r| r.message.as_deref().unwrap_or("").contains(constraint)),
            "{constraint}: {:?}",
            refused.results
        );
    }
    // A missing element id is refused too.
    let anonymous = clean.replace(" ; :bpmnElementId \"gate\"", "");
    assert!(!report(&anonymous).conforms);
}
