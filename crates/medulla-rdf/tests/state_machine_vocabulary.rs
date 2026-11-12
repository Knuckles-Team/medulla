//! The core state-machine vocabulary and the WorkItem machine description
//! (MEDULLA-ORCH-VOCAB-R811 and R812.3, R812.5).
#![cfg(feature = "rdf")]

mod common;

use std::collections::{BTreeMap, BTreeSet};

use medulla_rdf::mapping::parse_turtle;
use medulla_rdf::oxrdf::{NamedOrBlankNode, Term, Triple};
use medulla_shacl::validate_turtle;
use medulla_types::transition_refused::{MachineEvent, MachineState};
use medulla_work_types::work_item_machine::{
    WorkItemEvent, WorkItemStatus, WORK_ITEM_EDGES, WORK_ITEM_MACHINE, WORK_ITEM_STATES,
};

const KG: &str = "https://knuckles-team.github.io/ontology/core#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const SUBCLASS: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const OWL_IMPORTS: &str = "http://www.w3.org/2002/07/owl#imports";

const FOUNDATION: &str = include_str!("../../medulla-core/ontology/core-foundation-v1.ttl");
const MACHINES: &str = include_str!("../../medulla-core/ontology/state-machines-v1.ttl");
const SHAPES: &str = include_str!("../../medulla-core/ontology/state-machines-v1.shapes.ttl");

const CORE_CLASSES: [&str; 6] = [
    "StateMachine",
    "State",
    "TerminalState",
    "Transition",
    "TransitionEvent",
    "TransitionGuard",
];

fn kg(local: &str) -> String {
    format!("{KG}{local}")
}

fn iri(term: &Term) -> Option<String> {
    match term {
        Term::NamedNode(node) => Some(node.as_str().to_string()),
        _ => None,
    }
}

fn subject(triple: &Triple) -> Option<String> {
    match &triple.subject {
        NamedOrBlankNode::NamedNode(node) => Some(node.as_str().to_string()),
        _ => None,
    }
}

/// The triples of a set of documents.
struct Doc(Vec<Triple>);

impl Doc {
    fn of(documents: &[&str]) -> Self {
        Self(
            documents
                .iter()
                .flat_map(|document| parse_turtle(document).expect("the document parses"))
                .collect(),
        )
    }

    fn objects(&self, s: &str, p: &str) -> Vec<String> {
        self.0
            .iter()
            .filter(|t| subject(t).as_deref() == Some(s) && t.predicate.as_str() == p)
            .filter_map(|t| iri(&t.object))
            .collect()
    }

    fn literal(&self, s: &str, p: &str) -> Option<String> {
        self.0
            .iter()
            .filter(|t| subject(t).as_deref() == Some(s) && t.predicate.as_str() == p)
            .find_map(|t| match &t.object {
                Term::Literal(literal) => Some(literal.value().to_string()),
                _ => None,
            })
    }

    fn subjects(&self, p: &str, o: &str) -> Vec<String> {
        self.0
            .iter()
            .filter(|t| t.predicate.as_str() == p && iri(&t.object).as_deref() == Some(o))
            .filter_map(subject)
            .collect()
    }

    /// Every class `class` reaches through rdfs:subClassOf, itself included.
    fn ancestors(&self, class: &str) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![class.to_string()];
        while let Some(next) = stack.pop() {
            if seen.insert(next.clone()) {
                stack.extend(self.objects(&next, SUBCLASS));
            }
        }
        seen
    }
}

// spec: MEDULLA-ORCH-VOCAB-R811
#[test]
fn state_machine_terms_roll_up_to_core() {
    let doc = Doc::of(&[FOUNDATION, MACHINES]);
    for class in CORE_CLASSES {
        assert!(
            doc.ancestors(&kg(class)).contains(&kg("Concept")),
            ":{class} does not reach :Concept"
        );
    }
    assert!(!MACHINES.contains("owl:imports"));
    assert!(!SHAPES.contains("owl:imports"));
}

// spec: MEDULLA-ORCH-VOCAB-R811.1
#[test]
fn state_machine_classes_resolve() {
    let doc = Doc::of(&[FOUNDATION, MACHINES]);
    for class in CORE_CLASSES {
        assert!(
            doc.objects(&kg(class), RDF_TYPE)
                .contains(&"http://www.w3.org/2002/07/owl#Class".to_string()),
            ":{class} is not declared"
        );
        assert!(doc.ancestors(&kg(class)).contains(&kg("Concept")));
    }
    assert!(doc.ancestors(&kg("TerminalState")).contains(&kg("State")));
}

// spec: MEDULLA-ORCH-VOCAB-R811.1
#[test]
fn transition_event_is_distinct_from_core_event() {
    let doc = Doc::of(&[FOUNDATION, MACHINES]);
    assert!(
        !doc.objects(&kg("Event"), RDF_TYPE).is_empty(),
        "core :Event exists"
    );
    assert!(!doc.ancestors(&kg("TransitionEvent")).contains(&kg("Event")));
    assert!(!doc.ancestors(&kg("Event")).contains(&kg("TransitionEvent")));
}

// spec: MEDULLA-ORCH-VOCAB-R811.2
#[test]
fn state_machine_properties_resolve() {
    let doc = Doc::of(&[MACHINES]);
    let expected: [(&str, Option<&str>, &str); 8] = [
        ("hasState", Some("StateMachine"), "State"),
        ("initialState", Some("StateMachine"), "State"),
        ("fromState", Some("Transition"), "State"),
        ("toState", Some("Transition"), "State"),
        ("firedBy", Some("Transition"), "TransitionEvent"),
        ("guardedBy", Some("Transition"), "TransitionGuard"),
        ("currentState", None, "State"),
        (
            "storedForm",
            Some("State"),
            "http://www.w3.org/2001/XMLSchema#string",
        ),
    ];
    for (property, domain, range) in expected {
        let domains = doc.objects(&kg(property), DOMAIN);
        match domain {
            Some(domain) => assert_eq!(domains, [kg(domain)], ":{property} domain"),
            None => assert!(domains.is_empty(), ":{property} has no domain"),
        }
        let range = if range.starts_with("http") {
            range.to_string()
        } else {
            kg(range)
        };
        assert_eq!(
            doc.objects(&kg(property), RANGE),
            [range],
            ":{property} range"
        );
    }
}

const PREFIXES: &str = "@prefix : <https://knuckles-team.github.io/ontology/core#> . \
     @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . ";

/// A minimal well-formed machine: A to B on Go, B terminal.
const GOOD: &str = ":M a :StateMachine ; :initialState :M_A ; :hasState :M_A, :M_B . \
     :M_A a :State ; :storedForm \"a\" . \
     :M_B a :TerminalState ; :storedForm \"b\" . \
     :M_Event_Go a :TransitionEvent . \
     :M_T1 :fromState :M_A ; :toState :M_B ; :firedBy :M_Event_Go . \
     :rec :currentState :M_A .";

fn violations(data: &str) -> Vec<String> {
    let report = validate_turtle(SHAPES, &format!("{PREFIXES}{data}")).expect("fixtures parse");
    if report.conforms {
        return Vec::new();
    }
    vec![format!("{report:?}")]
}

// spec: MEDULLA-ORCH-VOCAB-R811.3
#[test]
fn state_machine_shapes_refuse_planted_faults() {
    assert!(violations(GOOD).is_empty(), "{:?}", violations(GOOD));
    let faults: [(&str, String, &str); 8] = [
        (
            "two from states",
            format!("{GOOD} :M_T1 :fromState :M_B ."),
            "fromState",
        ),
        (
            "no event",
            GOOD.replace(" ; :firedBy :M_Event_Go", ""),
            "firedBy",
        ),
        (
            "leaves a terminal state",
            format!("{GOOD} :M_T2 :fromState :M_B ; :toState :M_A ; :firedBy :M_Event_Go ."),
            "terminal",
        ),
        (
            "states of two machines",
            format!(
                "{GOOD} :N a :StateMachine ; :initialState :N_X ; :hasState :N_X . \
                 :N_X a :State ; :storedForm \"x\" . \
                 :M_T3 :fromState :M_A ; :toState :N_X ; :firedBy :M_Event_Go ."
            ),
            "one machine",
        ),
        (
            "two current states",
            format!("{GOOD} :rec :currentState :M_B ."),
            "currentState",
        ),
        (
            "two initial states",
            format!("{GOOD} :M :initialState :M_B ."),
            "initialState",
        ),
        (
            "missing stored form",
            GOOD.replace(":M_A a :State ; :storedForm \"a\" .", ":M_A a :State ."),
            "storedForm",
        ),
        (
            "duplicate stored form",
            GOOD.replace(":storedForm \"b\"", ":storedForm \"a\""),
            "storedForm",
        ),
    ];
    for (name, data, needle) in faults {
        let found = violations(&data);
        assert!(!found.is_empty(), "planted fault '{name}' was not refused");
        assert!(
            found[0].contains(needle),
            "fault '{name}' failed the wrong constraint: {}",
            found[0]
        );
    }
}

/// The pack extension rule of R811.6 for one document: every `:StateMachine` instance has
/// state, transition, event and guard classes that reach the core classes through
/// rdfs:subClassOf, and the document has no owl:imports.
fn pack_machine_conformance(document: &str, with: &Doc) -> Result<usize, String> {
    let own = Doc::of(&[document]);
    let machines = own.subjects(RDF_TYPE, &kg("StateMachine"));
    if machines.is_empty() {
        return Ok(0);
    }
    if own.0.iter().any(|t| t.predicate.as_str() == OWL_IMPORTS) {
        return Err("a pack machine file uses owl:imports".into());
    }
    let combined = Doc(with
        .0
        .iter()
        .cloned()
        .chain(own.0.iter().cloned())
        .collect());
    for machine in &machines {
        let states = combined.objects(machine, &kg("hasState"));
        if states.is_empty() {
            return Err(format!("{machine} declares no state"));
        }
        let typed = |instance: &str, core: &str| {
            combined
                .objects(instance, RDF_TYPE)
                .iter()
                .any(|class| combined.ancestors(class).contains(&kg(core)))
        };
        for state in &states {
            if !typed(state, "State") {
                return Err(format!("{state} has no core :State ancestor"));
            }
        }
        for transition in combined.subjects(&kg("fromState"), &states[0]) {
            if !typed(&transition, "Transition") {
                return Err(format!("{transition} has no core :Transition ancestor"));
            }
            for event in combined.objects(&transition, &kg("firedBy")) {
                if !typed(&event, "TransitionEvent") {
                    return Err(format!("{event} has no core :TransitionEvent ancestor"));
                }
            }
            for guard in combined.objects(&transition, &kg("guardedBy")) {
                if !typed(&guard, "TransitionGuard") {
                    return Err(format!("{guard} has no core :TransitionGuard ancestor"));
                }
            }
        }
    }
    Ok(machines.len())
}

// spec: MEDULLA-ORCH-VOCAB-R811.6
#[test]
fn every_pack_machine_extends_core() {
    let core = Doc::of(&[FOUNDATION, MACHINES]);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../medulla-core/ontology");
    let mut machines = 0;
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "ttl") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        if !text.contains("StateMachine") {
            continue;
        }
        machines += pack_machine_conformance(&text, &core)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }
    assert!(machines >= 1, "the WorkItem machine is found");
}

const PACK: &str = "@prefix : <https://knuckles-team.github.io/ontology/core#> . \
     @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
     :LampState rdfs:subClassOf :State . :LampTransition rdfs:subClassOf :Transition . \
     :LampEvent rdfs:subClassOf :TransitionEvent . \
     :LampMachine a :StateMachine ; :initialState :LampMachine_Off ; \
         :hasState :LampMachine_Off, :LampMachine_On . \
     :LampMachine_Off a :LampState ; :storedForm \"off\" . \
     :LampMachine_On a :LampState ; :storedForm \"on\" . \
     :LampMachine_Event_Toggle a :LampEvent . \
     :LampMachine_L01 a :LampTransition ; :fromState :LampMachine_Off ; \
         :toState :LampMachine_On ; :firedBy :LampMachine_Event_Toggle .";

// spec: MEDULLA-ORCH-VOCAB-R811.6
#[test]
fn pack_machine_with_owl_imports_is_refused() {
    let core = Doc::of(&[FOUNDATION, MACHINES]);
    assert_eq!(pack_machine_conformance(PACK, &core), Ok(1));
    let imports =
        format!("{PACK} <http://example.org/lamp> <{OWL_IMPORTS}> <https://knuckles-team.github.io/ontology/core> .");
    assert!(pack_machine_conformance(&imports, &core)
        .unwrap_err()
        .contains("owl:imports"));
}

// spec: MEDULLA-ORCH-VOCAB-R811.6
#[test]
fn pack_machine_without_core_ancestor_is_refused() {
    let core = Doc::of(&[FOUNDATION, MACHINES]);
    let detached = PACK.replace(":LampEvent rdfs:subClassOf :TransitionEvent .", "");
    assert!(pack_machine_conformance(&detached, &core)
        .unwrap_err()
        .contains("TransitionEvent"));
    let detached = PACK.replace(":LampState rdfs:subClassOf :State .", "");
    assert!(pack_machine_conformance(&detached, &core)
        .unwrap_err()
        .contains(":State"));
}

fn state_name(state: WorkItemStatus) -> &'static str {
    match state {
        WorkItemStatus::Submitted => "Submitted",
        WorkItemStatus::Ready => "Ready",
        WorkItemStatus::Leased => "Leased",
        WorkItemStatus::Running => "Running",
        WorkItemStatus::Succeeded => "Succeeded",
        WorkItemStatus::Failed => "Failed",
        WorkItemStatus::Cancelled => "Cancelled",
        WorkItemStatus::DeadLetter => "DeadLetter",
    }
}

// spec: MEDULLA-ORCH-VOCAB-R812.3
#[test]
fn work_item_machine_matches_edge_table() {
    let doc = Doc::of(&[MACHINES]);
    let local = |value: &str| value.trim_start_matches(KG).to_string();
    let mut described = BTreeMap::new();
    for transition in doc.subjects(RDF_TYPE, &kg("WorkItemTransition")) {
        let id = doc.literal(&transition, &kg("edgeId")).expect("edge id");
        assert_eq!(transition, kg(&format!("WorkItemMachine_{id}")));
        let one = |p: &str| {
            let values = doc.objects(&transition, &kg(p));
            assert!(values.len() <= 1, "{transition} has several :{p}");
            values.first().map(|v| local(v))
        };
        described.insert(
            id,
            (
                one("fromState"),
                one("firedBy"),
                one("guardedBy"),
                one("toState"),
            ),
        );
    }
    let table: BTreeMap<_, _> = WORK_ITEM_EDGES
        .iter()
        .map(|edge| {
            (
                edge.id.to_string(),
                (
                    Some(format!("WorkItemMachine_{}", state_name(edge.from))),
                    Some(format!(
                        "WorkItemMachine_Event_{}",
                        edge.event.ontology_name()
                    )),
                    edge.guard
                        .map(|guard| format!("WorkItemMachine_Guard_{}", guard.ontology_name())),
                    Some(format!("WorkItemMachine_{}", state_name(edge.to))),
                ),
            )
        })
        .collect();
    assert_eq!(described, table);
    assert_eq!(
        doc.objects(&kg("WorkItemMachine"), &kg("initialState")),
        [kg(&format!(
            "WorkItemMachine_{}",
            state_name(WORK_ITEM_MACHINE.initial)
        ))]
    );
}

// spec: MEDULLA-ORCH-VOCAB-R812.3
#[test]
fn work_item_machine_classes_extend_core() {
    let doc = Doc::of(&[FOUNDATION, MACHINES]);
    for (class, core) in [
        ("WorkItemState", "State"),
        ("WorkItemTerminalState", "TerminalState"),
        ("WorkItemTransition", "Transition"),
        ("WorkItemEvent", "TransitionEvent"),
        ("WorkItemGuard", "TransitionGuard"),
    ] {
        let ancestors = doc.ancestors(&kg(class));
        assert!(
            ancestors.contains(&kg(core)),
            ":{class} does not extend :{core}"
        );
        assert!(ancestors.contains(&kg("Concept")));
    }
    assert!(!MACHINES.contains("owl:imports"));
    assert!(pack_machine_conformance(MACHINES, &Doc::of(&[FOUNDATION])).is_ok());
    let whole = format!("{MACHINES}\n{FOUNDATION}");
    assert!(
        validate_turtle(SHAPES, &whole).expect("parses").conforms,
        "the WorkItem description conforms to the core shapes"
    );
}

// spec: MEDULLA-ORCH-VOCAB-R812.5
#[test]
fn work_item_exports_match_rust_enum() {
    let doc = Doc::of(&[MACHINES]);
    let machine = kg("WorkItemMachine");
    let states: BTreeSet<String> = doc.objects(&machine, &kg("hasState")).into_iter().collect();
    let expected: BTreeSet<String> = WORK_ITEM_STATES
        .iter()
        .map(|s| kg(&format!("WorkItemMachine_{}", state_name(*s))))
        .collect();
    assert_eq!(states, expected);
    for state in WORK_ITEM_STATES {
        let instance = kg(&format!("WorkItemMachine_{}", state_name(state)));
        assert_eq!(
            doc.literal(&instance, &kg("storedForm")).as_deref(),
            Some(state.stored())
        );
        let terminal = doc
            .objects(&instance, RDF_TYPE)
            .contains(&kg("WorkItemTerminalState"));
        assert_eq!(terminal, state.is_terminal(), "{instance}");
    }
    let events: BTreeSet<String> = doc
        .subjects(RDF_TYPE, &kg("WorkItemEvent"))
        .into_iter()
        .collect();
    let expected: BTreeSet<String> = WorkItemEvent::ALL
        .iter()
        .map(|e| kg(&format!("WorkItemMachine_Event_{}", e.ontology_name())))
        .collect();
    assert_eq!(events, expected);
    // Event names are stable wire text.
    assert!(WorkItemEvent::ALL.iter().all(|e| !e.name().is_empty()));
}

// The medulla machines other than WorkItem (MEDULLA-ORCH-VOCAB-R811.6): each Turtle edge
// set equals its Rust edge table and each machine class extends a core class.
mod sm {
    use std::collections::BTreeSet;

    use super::common::{parse_all, KG, RDFS};
    use medulla_rdf::oxrdf::{Literal, NamedNode, Term, Triple};
    use medulla_types::transition_refused::{MachineDef, MachineEvent, MachineState};

    const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    const TTL: &str = include_str!("../../medulla-core/ontology/state-machines-v1.ttl");
    const CORE: [&str; 5] = [
        "State",
        "TerminalState",
        "Transition",
        "TransitionEvent",
        "TransitionGuard",
    ];
    const KINDS: [&str; 5] = ["State", "TerminalState", "Transition", "Event", "Guard"];

    fn triples() -> Vec<Triple> {
        parse_all(&[TTL])
    }

    fn kg(local: &str) -> String {
        format!("{KG}{local}")
    }

    fn object_of<'a>(triples: &'a [Triple], subject: &str, predicate: &str) -> Option<&'a Term> {
        let subject = NamedNode::new(subject).unwrap();
        triples
            .iter()
            .find(|t| t.subject == subject.clone().into() && t.predicate.as_str() == predicate)
            .map(|t| &t.object)
    }

    fn iri(term: &Term) -> &str {
        match term {
            Term::NamedNode(node) => node.as_str(),
            other => panic!("expected an IRI, got {other}"),
        }
    }

    fn literal(term: &Term) -> &str {
        match term {
            Term::Literal(value) => Literal::value(value),
            other => panic!("expected a literal, got {other}"),
        }
    }

    type Row = (String, String, String, String);

    /// The `(edge id, from stored form, event, to stored form)` rows of one machine's Turtle.
    fn turtle_edges(triples: &[Triple], machine: &str) -> BTreeSet<Row> {
        let prefix = kg(&format!("{machine}_"));
        let event_prefix = kg(&format!("{machine}_Event_"));
        let from_state = kg("fromState");
        let stored = |state: &str| {
            literal(object_of(triples, state, &kg("storedForm")).expect("storedForm")).to_owned()
        };
        triples
            .iter()
            .filter(|t| t.predicate.as_str() == from_state)
            .filter_map(|t| {
                let subject = t.subject.to_string();
                subject
                    .strip_prefix('<')
                    .and_then(|s| s.strip_suffix('>'))
                    .map(str::to_owned)
            })
            .filter(|subject| subject.starts_with(&prefix))
            .map(|subject| {
                let from = iri(object_of(triples, &subject, &from_state).unwrap()).to_owned();
                let to = iri(object_of(triples, &subject, &kg("toState")).unwrap()).to_owned();
                let event = iri(object_of(triples, &subject, &kg("firedBy")).unwrap()).to_owned();
                (
                    subject[prefix.len()..].to_owned(),
                    stored(&from),
                    event
                        .strip_prefix(&event_prefix)
                        .expect("event IRI")
                        .to_owned(),
                    stored(&to),
                )
            })
            .collect()
    }

    fn rust_edges<S: MachineState, E: MachineEvent, G>(
        machine: &MachineDef<S, E, G>,
    ) -> BTreeSet<Row> {
        machine
            .edges
            .iter()
            .map(|e| {
                let id = e.id.to_owned();
                (
                    id,
                    e.from.stored().into(),
                    e.event.name().into(),
                    e.to.stored().into(),
                )
            })
            .collect()
    }

    pub(super) fn assert_matches<S: MachineState + std::fmt::Debug, E: MachineEvent, G>(
        machine: &MachineDef<S, E, G>,
    ) {
        let triples = triples();
        assert_eq!(
            turtle_edges(&triples, machine.name),
            rust_edges(machine),
            "{} Turtle edges differ from the Rust edge table",
            machine.name
        );
        let has_state = kg("hasState");
        let subject = NamedNode::new(kg(machine.name)).unwrap();
        let declared: BTreeSet<String> = triples
            .iter()
            .filter(|t| t.subject == subject.clone().into() && t.predicate.as_str() == has_state)
            .map(|t| {
                literal(object_of(&triples, iri(&t.object), &kg("storedForm")).unwrap()).to_owned()
            })
            .collect();
        let rust: BTreeSet<String> = machine
            .states
            .iter()
            .map(|s| s.stored().to_owned())
            .collect();
        assert_eq!(declared, rust, "{} states differ", machine.name);
        let initial = iri(object_of(&triples, &kg(machine.name), &kg("initialState")).unwrap());
        assert_eq!(
            initial,
            kg(&format!("{}_{:?}", machine.name, machine.initial))
        );
        for &terminal in machine.terminals {
            let state = kg(&format!("{}_{:?}", machine.name, terminal));
            let ty = iri(object_of(&triples, &state, RDF_TYPE).unwrap());
            assert!(
                ty.ends_with("TerminalState"),
                "{state} is not typed terminal"
            );
        }
    }

    pub(super) fn assert_extends_core(prefix: &str) {
        let triples = triples();
        let sub_class_of = format!("{RDFS}subClassOf");
        for (kind, core) in KINDS.iter().zip(CORE) {
            let class = NamedNode::new(kg(&format!("{prefix}{kind}"))).unwrap();
            let parents: Vec<&str> = triples
                .iter()
                .filter(|t| {
                    t.subject == class.clone().into() && t.predicate.as_str() == sub_class_of
                })
                .map(|t| iri(&t.object))
                .collect();
            assert!(
                parents.contains(&kg(core).as_str()),
                "{class} must extend :{core}, has {parents:?}"
            );
        }
    }
}

// spec: MEDULLA-IDENTITY-R720.2
#[test]
fn governed_change_machine_matches_edge_table() {
    sm::assert_matches(&medulla_mutation_types::governed_change::GOVERNED_CHANGE_MACHINE);
}
// spec: MEDULLA-IDENTITY-R720.2
#[test]
fn governed_change_machine_classes_extend_core() {
    sm::assert_extends_core("GovernedChange");
}

// spec: MEDULLA-IDENTITY-R721.2
#[test]
fn user_status_machine_matches_edge_table() {
    sm::assert_matches(&medulla_identity_types::identity::USER_STATUS_MACHINE);
}
// spec: MEDULLA-IDENTITY-R721.2
#[test]
fn user_status_machine_classes_extend_core() {
    sm::assert_extends_core("UserStatus");
}

// spec: MEDULLA-IDENTITY-R722.1
#[test]
fn rbac_elevation_machine_matches_legal_table() {
    sm::assert_matches(&medulla_identity_types::rbac_elevation::RBAC_ELEVATION_MACHINE);
}
// spec: MEDULLA-IDENTITY-R722.1
#[test]
fn rbac_elevation_machine_classes_extend_core() {
    sm::assert_extends_core("RbacElevation");
}

// spec: MEDULLA-PLACEMENT-R782.2
#[test]
fn control_lease_machine_matches_edge_table() {
    sm::assert_matches(&medulla_work_types::control_lease::CONTROL_LEASE_MACHINE);
}
// spec: MEDULLA-PLACEMENT-R782.2
#[test]
fn control_lease_machine_classes_extend_core() {
    sm::assert_extends_core("ControlLease");
}

// spec: MEDULLA-PLACEMENT-R780.2
#[test]
fn lane_hold_machine_matches_edge_table() {
    sm::assert_matches(&medulla_result_types::epistemic_operations::LANE_HOLD_MACHINE);
}
// spec: MEDULLA-PLACEMENT-R780.2
#[test]
fn lane_hold_machine_classes_extend_core() {
    sm::assert_extends_core("LaneHold");
}

// spec: MEDULLA-PLACEMENT-R783.5
#[test]
fn partition_move_machine_matches_edge_table() {
    sm::assert_matches(&medulla_raft::raft::placement::PARTITION_MOVE_MACHINE);
}
// spec: MEDULLA-PLACEMENT-R783.5
#[test]
fn partition_move_machine_classes_extend_core() {
    sm::assert_extends_core("PartitionMove");
}

// spec: MEDULLA-PLACEMENT-R781.2
#[test]
fn resource_reservation_machine_matches_edge_table() {
    sm::assert_matches(&medulla_result_types::epistemic_operations::RESOURCE_RESERVATION_MACHINE);
}
// spec: MEDULLA-PLACEMENT-R781.2
#[test]
fn resource_reservation_machine_classes_extend_core() {
    sm::assert_extends_core("ResourceReservation");
}

// spec: MEDULLA-PLACEMENT-R783.1
#[test]
fn membership_shrink_machine_matches_transition_is_valid() {
    sm::assert_matches(&medulla_raft::raft::membership_shrink::MEMBERSHIP_SHRINK_MACHINE);
}
// spec: MEDULLA-PLACEMENT-R783.1
#[test]
fn membership_shrink_machine_classes_extend_core() {
    sm::assert_extends_core("MembershipShrink");
}

// spec: MEDULLA-CQ-ORCH-R810.2
#[test]
fn broker_message_machine_matches_edge_table() {
    sm::assert_matches(&medulla_core::broker::message_machine::BROKER_MESSAGE_MACHINE);
}
// spec: MEDULLA-CQ-ORCH-R810.2
#[test]
fn broker_message_machine_classes_extend_core() {
    sm::assert_extends_core("BrokerMessage");
}

// spec: MEDULLA-SDLC-R030.2
#[test]
fn defect_machine_matches_edge_table() {
    sm::assert_matches(&medulla_source_types::defect_machine::DEFECT_MACHINE);
}
// spec: MEDULLA-SDLC-R030.2
#[test]
fn defect_machine_classes_extend_core() {
    sm::assert_extends_core("Defect");
}

// spec: MEDULLA-DURABLE-KERNEL-R872.2
#[test]
fn semantic_index_upgrade_machine_matches_edge_table() {
    sm::assert_matches(&medulla_h_retrieve::semantic_index_runtime::SEMANTIC_INDEX_UPGRADE_MACHINE);
}
// spec: MEDULLA-DURABLE-KERNEL-R872.2
#[test]
fn semantic_index_upgrade_machine_classes_extend_core() {
    sm::assert_extends_core("SemanticIndexUpgrade");
}

// spec: MEDULLA-PLACEMENT-R783.4
#[test]
fn shard_drain_machine_matches_edge_table() {
    sm::assert_matches(&medulla_raft_xshard::drain::SHARD_DRAIN_MACHINE);
}
// spec: MEDULLA-PLACEMENT-R783.4
#[test]
fn shard_drain_machine_classes_extend_core() {
    sm::assert_extends_core("ShardDrain");
}

// spec: MEDULLA-PLACEMENT-R782.2
#[test]
fn capacity_lease_machine_matches_edge_table() {
    sm::assert_matches(&medulla_mutation_types::capacity_lease::CAPACITY_LEASE_MACHINE);
}
// spec: MEDULLA-PLACEMENT-R782.2
#[test]
fn capacity_lease_machine_classes_extend_core() {
    sm::assert_extends_core("CapacityLease");
}

// spec: MEDULLA-DECISION-ENGINE-R174.1
#[test]
fn generation_cycle_machine_matches_edge_table() {
    sm::assert_matches(
        &medulla_decision_types::decision::statistical::retrieval_generation::GENERATION_CYCLE_MACHINE,
    );
}
// spec: MEDULLA-DECISION-ENGINE-R174.1
#[test]
fn generation_cycle_machine_classes_extend_core() {
    sm::assert_extends_core("GenerationCycle");
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R014.2.3
#[test]
fn catalog_probe_machine_matches_edge_table() {
    sm::assert_matches(&medulla_catalog_types::probe_machine::CATALOG_PROBE_MACHINE);
}
// spec: MEDULLA-UNIFIED-DATA-PLANE-R014.2.3
#[test]
fn catalog_probe_machine_classes_extend_core() {
    sm::assert_extends_core("CatalogProbe");
}
