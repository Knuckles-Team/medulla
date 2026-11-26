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
// spec: MEDULLA-ORCH-VOCAB-R812.6
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
    use medulla_rdf::oxrdf::{Literal, NamedNode, Subject, Term, Triple};
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
        assert_matches_as(machine, machine.name);
    }

    /// As [`assert_matches`], for a machine whose ontology instance carries another name than
    /// its Rust `MachineDef`: `ontology_name` is the local name of the `:StateMachine`
    /// instance, and the IRIs of its states, events and transitions begin with it.
    pub(super) fn assert_matches_as<S: MachineState + std::fmt::Debug, E: MachineEvent, G>(
        machine: &MachineDef<S, E, G>,
        ontology_name: &str,
    ) {
        let triples = triples();
        assert_eq!(
            turtle_edges(&triples, ontology_name),
            rust_edges(machine),
            "{} Turtle edges differ from the Rust edge table",
            machine.name
        );
        let has_state = kg("hasState");
        let subject = NamedNode::new(kg(ontology_name)).unwrap();
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
        let initial = iri(object_of(&triples, &kg(ontology_name), &kg("initialState")).unwrap());
        assert_eq!(
            initial,
            kg(&format!("{ontology_name}_{:?}", machine.initial))
        );
        for &terminal in machine.terminals {
            let state = kg(&format!("{ontology_name}_{terminal:?}"));
            let ty = iri(object_of(&triples, &state, RDF_TYPE).unwrap());
            assert!(
                ty.ends_with("TerminalState"),
                "{state} is not typed terminal"
            );
        }
    }

    /// The Turtle transitions of `machine` equal the transitions of a statechart in order:
    /// edge id `L<index>`, source state, event and target state.
    pub(super) fn assert_matches_chart(machine: &str, chart: &medulla_statechart::StatechartDef) {
        let triples = triples();
        let chart_edges: BTreeSet<Row> = chart
            .transitions
            .iter()
            .enumerate()
            .map(|(index, t)| {
                (
                    format!("L{index:02}"),
                    t.from.clone(),
                    t.event.clone(),
                    t.to.clone(),
                )
            })
            .collect();
        assert_eq!(
            turtle_edges(&triples, machine),
            chart_edges,
            "{machine} Turtle transitions differ from the statechart"
        );
        let has_state = kg("hasState");
        let subject = NamedNode::new(kg(machine)).unwrap();
        let declared: BTreeSet<String> = triples
            .iter()
            .filter(|t| t.subject == subject.clone().into() && t.predicate.as_str() == has_state)
            .map(|t| {
                literal(object_of(&triples, iri(&t.object), &kg("storedForm")).unwrap()).to_owned()
            })
            .collect();
        let states: BTreeSet<String> = chart.states.iter().map(|s| s.id.clone()).collect();
        assert_eq!(declared, states, "{machine} states differ from the chart");
        let initial = iri(object_of(&triples, &kg(machine), &kg("initialState")).unwrap());
        let initial_stored = literal(object_of(&triples, initial, &kg("storedForm")).unwrap());
        assert_eq!(initial_stored, chart.initial);
        let stored_form = kg("storedForm");
        let prefix = kg(&format!("{machine}_"));
        for state in &chart.states {
            let subject = triples
                .iter()
                .find_map(|t| match (&t.subject, &t.object) {
                    (Subject::NamedNode(node), Term::Literal(value))
                        if t.predicate.as_str() == stored_form
                            && node.as_str().starts_with(&prefix)
                            && Literal::value(value) == state.id =>
                    {
                        Some(node.as_str().to_owned())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no state with storedForm {}", state.id));
            let ty = iri(object_of(&triples, &subject, RDF_TYPE).unwrap());
            assert_eq!(
                ty.ends_with("TerminalState"),
                chart.is_final(&state.id),
                "{} terminal typing",
                state.id
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
// spec: MEDULLA-PLACEMENT-R783.6
#[test]
fn membership_shrink_machine_matches_edge_table() {
    sm::assert_matches(&medulla_raft::raft::membership_shrink::MEMBERSHIP_SHRINK_MACHINE);
}
// spec: MEDULLA-PLACEMENT-R783.1
#[test]
fn membership_shrink_machine_classes_extend_core() {
    sm::assert_extends_core("MembershipShrink");
}

// spec: MEDULLA-CQ-ORCH-R810.2
// spec: MEDULLA-CQ-ORCH-R810.5
#[test]
fn broker_message_machine_matches_edge_table() {
    sm::assert_matches(&medulla_core::broker::message_machine::BROKER_MESSAGE_MACHINE);
}
// spec: MEDULLA-CQ-ORCH-R810.2
#[test]
fn broker_message_machine_classes_extend_core() {
    sm::assert_extends_core("BrokerMessage");
}

// proves R811.2: Describe the analytics job machine in the ontology
// spec: MEDULLA-CQ-ORCH-R811.2
#[test]
fn analytics_job_machine_matches_edge_table() {
    sm::assert_matches(&medulla_jobs::job_machine::ANALYTICS_JOB_MACHINE);
}
// spec: MEDULLA-CQ-ORCH-R811.2
#[test]
fn analytics_job_machine_classes_extend_core() {
    sm::assert_extends_core("AnalyticsJob");
}

// spec: MEDULLA-SDLC-R030.2
// spec: MEDULLA-SDLC-R030.3
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

// spec: MEDULLA-PLACEMENT-R783.7
#[test]
fn move_phase_machine_matches_edge_table() {
    sm::assert_matches(&medulla_raft_xshard::cross_node_elasticity::MOVE_PHASE_MACHINE);
    sm::assert_extends_core("MovePhase");
}

// spec: MEDULLA-PLACEMENT-R783.11
#[test]
fn shard_placement_machine_matches_edge_table() {
    sm::assert_matches(&medulla_raft_xshard::cross_node_elasticity::SHARD_PLACEMENT_MACHINE);
    sm::assert_extends_core("ShardPlacement");
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

// spec: MEDULLA-ORCH-VOCAB-R815.6
#[test]
fn incident_record_machine_matches_edge_table() {
    sm::assert_matches(&medulla_catalog_types::incident_record_machine::INCIDENT_RECORD_MACHINE);
}
// spec: MEDULLA-ORCH-VOCAB-R815.6
#[test]
fn incident_record_machine_classes_extend_core() {
    sm::assert_extends_core("IncidentRecord");
}

// spec: MEDULLA-ORCH-VOCAB-R815.2
#[test]
fn agent_lifecycle_machine_matches_edge_table() {
    sm::assert_matches(
        &medulla_work_types::orchestration::agent_lifecycle_machine::AGENT_LIFECYCLE_MACHINE,
    );
}
// spec: MEDULLA-ORCH-VOCAB-R815.2
#[test]
fn agent_lifecycle_machine_classes_extend_core() {
    sm::assert_extends_core("AgentLifecycle");
}

// spec: MEDULLA-ORCH-VOCAB-R815.4
#[test]
fn gap_view_machine_matches_edge_table() {
    sm::assert_matches(&medulla_work_types::work_market::gap_machine::GAP_VIEW_MACHINE);
}
// spec: MEDULLA-ORCH-VOCAB-R815.4
#[test]
fn gap_view_machine_classes_extend_core() {
    sm::assert_extends_core("GapView");
}

// spec: MEDULLA-ORCH-VOCAB-R815.5
#[test]
fn incident_machine_matches_edge_table() {
    sm::assert_matches(&medulla_mutation_types::incident_machine::INCIDENT_MACHINE);
}
// spec: MEDULLA-ORCH-VOCAB-R815.5
#[test]
fn incident_machine_classes_extend_core() {
    sm::assert_extends_core("Incident");
}

// spec: MEDULLA-ORCH-VOCAB-R815.9
#[test]
fn workflow_machine_matches_edge_table() {
    sm::assert_matches(&medulla_work_types::orchestration::workflow_machine::WORKFLOW_MACHINE);
}
// spec: MEDULLA-ORCH-VOCAB-R815.9
#[test]
fn workflow_machine_classes_extend_core() {
    sm::assert_extends_core("Workflow");
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

// spec: MEDULLA-SDLC-R019.7
#[test]
fn delivery_proposal_machine_matches_edge_table() {
    sm::assert_matches(&medulla_source_types::delivery::DELIVERY_PROPOSAL_MACHINE);
}
// spec: MEDULLA-SDLC-R019.7
#[test]
fn delivery_proposal_machine_classes_extend_core() {
    sm::assert_extends_core("DeliveryProposal");
}

// spec: MEDULLA-FEDERATED-QUERY-R739.1
#[test]
fn shard_monitor_machine_matches_edge_table() {
    sm::assert_matches(
        &medulla_decision_types::decision::statistical::shard_monitor::SHARD_MONITOR_MACHINE,
    );
}
// spec: MEDULLA-FEDERATED-QUERY-R739.1
#[test]
fn shard_monitor_machine_classes_extend_core() {
    sm::assert_extends_core("ShardMonitor");
}

// spec: MEDULLA-INGEST-WORKER-R911.1
#[test]
fn enrichment_work_machine_matches_edge_table() {
    sm::assert_matches(&medulla_compute::parser::enrichment_work_machine::ENRICHMENT_WORK_MACHINE);
}
// spec: MEDULLA-INGEST-WORKER-R911.1
#[test]
fn enrichment_work_machine_classes_extend_core() {
    sm::assert_extends_core("EnrichmentWork");
}

// spec: MEDULLA-INGEST-WORKER-R911.2
#[test]
fn enrichment_machine_matches_edge_table() {
    sm::assert_matches(&medulla_compute::parser::enrichment_machine::ENRICHMENT_MACHINE);
}
// spec: MEDULLA-INGEST-WORKER-R911.2
#[test]
fn enrichment_machine_classes_extend_core() {
    sm::assert_extends_core("Enrichment");
}

// spec: MEDULLA-INGEST-WORKER-R911.3
#[test]
fn ingest_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::ingest_machine::INGEST_MACHINE);
}
// spec: MEDULLA-INGEST-WORKER-R911.3
#[test]
fn ingest_machine_classes_extend_core() {
    sm::assert_extends_core("Ingest");
}

// spec: MEDULLA-INGEST-WORKER-R911.4
#[test]
fn throughput_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::throughput_machine::THROUGHPUT_MACHINE);
}
// spec: MEDULLA-INGEST-WORKER-R911.4
#[test]
fn throughput_machine_classes_extend_core() {
    sm::assert_extends_core("Throughput");
}

// spec: MEDULLA-INGEST-WORKER-R912.1
#[test]
fn enumeration_run_machine_matches_edge_table() {
    sm::assert_matches(&medulla_h_ingest::source_ingestion::sweep::ENUMERATION_RUN_MACHINE);
}
// spec: MEDULLA-INGEST-WORKER-R912.1
#[test]
fn enumeration_run_machine_classes_extend_core() {
    sm::assert_extends_core("EnumerationRun");
}
// spec: MEDULLA-DECISION-ENGINE-R805.7
#[test]
fn revision_machine_matches_edge_table() {
    sm::assert_matches(&medulla_decision_outcome_types::decision::promotion::REVISION_MACHINE);
}
// spec: MEDULLA-DECISION-ENGINE-R805.7
#[test]
fn revision_machine_classes_extend_core() {
    sm::assert_extends_core("Revision");
}

// spec: MEDULLA-DECISION-ENGINE-R831.2
#[test]
fn feature_ledger_machine_matches_edge_table() {
    sm::assert_matches(&medulla_decision_types::decision::feature_match::FEATURE_LEDGER_MACHINE);
}

// spec: MEDULLA-DECISION-ENGINE-R831.4
#[test]
fn profile_grant_and_reservation_matches_edge_table() {
    sm::assert_matches(&medulla_types::profile_grant_machine::PROFILE_GRANT_MACHINE);
    sm::assert_matches(&medulla_types::reservation_machine::RESERVATION_MACHINE);
}

// spec: MEDULLA-CLUSTER-R004.5
#[test]
fn cluster_run_machine_matches_edge_table() {
    sm::assert_matches(&medulla_numeric::cluster::CLUSTER_RUN_MACHINE);
}
// spec: MEDULLA-CLUSTER-R004.5
#[test]
fn cluster_run_machine_classes_extend_core() {
    sm::assert_extends_core("ClusterRun");
}

// spec: MEDULLA-PLACEMENT-R751.756
// spec: MEDULLA-PLACEMENT-R785.1
#[test]
fn schedule_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::schedule_machine::SCHEDULE_MACHINE);
}
// spec: MEDULLA-PLACEMENT-R751.756
#[test]
fn schedule_machine_classes_extend_core() {
    sm::assert_extends_core("Schedule");
}

// spec: MEDULLA-TYPED-PACKS-R810.1
#[test]
fn admission_machine_matches_edge_table() {
    sm::assert_matches(&medulla_transaction::admission_machine::ADMISSION_MACHINE);
}
// spec: MEDULLA-TYPED-PACKS-R810.1
#[test]
fn admission_machine_classes_extend_core() {
    sm::assert_extends_core("Admission");
}

// spec: MEDULLA-TYPED-PACKS-R810.2
#[test]
fn agent_library_machine_matches_edge_table() {
    sm::assert_matches(
        &medulla_agent_types::agent_library::agent_library_machine::AGENT_LIBRARY_MACHINE,
    );
}
// spec: MEDULLA-TYPED-PACKS-R810.2
#[test]
fn agent_library_machine_classes_extend_core() {
    sm::assert_extends_core("AgentLibrary");
}

// spec: MEDULLA-TYPED-PACKS-R807
#[test]
fn conformance_evidence_machine_matches_edge_table() {
    sm::assert_matches(
        &medulla_catalog_types::connector_pack::conformance_evidence::CONFORMANCE_EVIDENCE_MACHINE,
    );
}
// spec: MEDULLA-TYPED-PACKS-R807
#[test]
fn conformance_evidence_machine_classes_extend_core() {
    sm::assert_extends_core("ConformanceEvidence");
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R905.1
#[test]
fn bar_machine_matches_edge_table() {
    sm::assert_matches(&medulla_result_types::compute_result::market::bar_machine::BAR_MACHINE);
}
// spec: MEDULLA-FINANCE-PRIMITIVES-R905.1
#[test]
fn bar_machine_classes_extend_core() {
    sm::assert_extends_core("Bar");
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R905.3
#[test]
fn signal_machine_matches_edge_table() {
    sm::assert_matches(
        &medulla_result_types::compute_result::market::signal_machine::SIGNAL_MACHINE,
    );
}
// spec: MEDULLA-FINANCE-PRIMITIVES-R905.3
#[test]
fn signal_machine_classes_extend_core() {
    sm::assert_extends_core("SignalData");
}

// spec: MEDULLA-ENGINE-PARITY-R021.1
#[test]
fn instance_machine_matches_edge_table() {
    sm::assert_matches(&medulla_statechart::instance_machine::INSTANCE_MACHINE);
}
// spec: MEDULLA-ENGINE-PARITY-R021.1
#[test]
fn instance_machine_classes_extend_core() {
    sm::assert_extends_core("Instance");
}

// spec: MEDULLA-ENGINE-PARITY-R021.2
#[test]
fn sql_form_machine_matches_edge_table() {
    sm::assert_matches(&medulla_mutation_types::sql_form_machine::SQL_FORM_MACHINE);
}
// spec: MEDULLA-ENGINE-PARITY-R021.2
#[test]
fn sql_form_machine_classes_extend_core() {
    sm::assert_extends_core("SqlForm");
}

// spec: MEDULLA-ENGINE-PARITY-R021.3
#[test]
fn statechart_instance_machine_matches_edge_table() {
    sm::assert_matches(
        &medulla_protocol_types::result_contract::coordination::STATECHART_INSTANCE_MACHINE,
    );
}
// spec: MEDULLA-ENGINE-PARITY-R021.3
#[test]
fn statechart_instance_machine_classes_extend_core() {
    sm::assert_extends_core("StatechartInstance");
}

// spec: MEDULLA-FEDERATED-QUERY-R738.16
#[test]
fn trace_outcome_machine_matches_edge_table() {
    sm::assert_matches(&medulla_result_types::epistemic_operations::TRACE_OUTCOME_MACHINE);
}
// spec: MEDULLA-FEDERATED-QUERY-R738.16
#[test]
fn trace_outcome_machine_classes_extend_core() {
    sm::assert_extends_core("TraceOutcome");
}

// spec: MEDULLA-FEDERATED-QUERY-R738.17
#[test]
fn uql_machine_matches_edge_table() {
    sm::assert_matches(&medulla_query_types::uql_machine::UQL_MACHINE);
}
// spec: MEDULLA-FEDERATED-QUERY-R738.17
#[test]
fn uql_machine_classes_extend_core() {
    sm::assert_extends_core("Uql");
}

// spec: MEDULLA-SDLC-R030.4
#[test]
fn spec_machine_matches_edge_table() {
    sm::assert_matches(&medulla_source_types::spec_machine::SPEC_MACHINE);
}
// spec: MEDULLA-SDLC-R030.4
#[test]
fn spec_machine_classes_extend_core() {
    sm::assert_extends_core("Spec");
}

// spec: MEDULLA-SDLC-R030.5
#[test]
fn task_machine_matches_edge_table() {
    sm::assert_matches(&medulla_source_types::task_machine::TASK_MACHINE);
}
// spec: MEDULLA-SDLC-R030.5
#[test]
fn task_machine_classes_extend_core() {
    sm::assert_extends_core("Task");
}

// spec: MEDULLA-FEDERATED-QUERY-R738.1
#[test]
fn capability_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::capability_machine::CAPABILITY_MACHINE);
}
// spec: MEDULLA-FEDERATED-QUERY-R738.1
#[test]
fn capability_machine_classes_extend_core() {
    sm::assert_extends_core("Capability");
}

// spec: MEDULLA-FEDERATED-QUERY-R738.3
#[test]
fn claim_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::claim_machine::CLAIM_MACHINE);
}
// spec: MEDULLA-FEDERATED-QUERY-R738.3
#[test]
fn claim_machine_classes_extend_core() {
    sm::assert_extends_core("Claim");
}

// spec: MEDULLA-FEDERATED-QUERY-R738.5
#[test]
fn contradiction_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::contradiction_machine::CONTRADICTION_MACHINE);
}
// spec: MEDULLA-FEDERATED-QUERY-R738.5
#[test]
fn contradiction_machine_classes_extend_core() {
    sm::assert_extends_core("Contradiction");
}

// spec: MEDULLA-FEDERATED-QUERY-R738.7
#[test]
fn gap_topic_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::gap_topic_machine::GAP_TOPIC_MACHINE);
}
// spec: MEDULLA-FEDERATED-QUERY-R738.7
#[test]
fn gap_topic_machine_classes_extend_core() {
    sm::assert_extends_core("GapTopic");
}

// spec: MEDULLA-FEDERATED-QUERY-R738.8
#[test]
fn geo_task_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::geo_task_machine::GEO_TASK_MACHINE);
}
// spec: MEDULLA-FEDERATED-QUERY-R738.8
#[test]
fn geo_task_machine_classes_extend_core() {
    sm::assert_extends_core("GeoTask");
}

// spec: MEDULLA-DURABLE-KERNEL-R874.12
#[test]
fn audit_machine_matches_edge_table() {
    sm::assert_matches(&medulla_store::redb_store::audit_machine::AUDIT_MACHINE);
    sm::assert_extends_core("Audit");
}

// spec: MEDULLA-DECISION-ENGINE-R830.5
#[test]
fn gap_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::gap_machine::GAP_MACHINE);
}
// spec: MEDULLA-DECISION-ENGINE-R830.5
#[test]
fn gap_machine_classes_extend_core() {
    sm::assert_extends_core("Gap");
}

// spec: MEDULLA-DECISION-ENGINE-R831.1
#[test]
fn verifiercase_machine_matches_edge_table() {
    sm::assert_matches(
        &medulla_decision_types::decision::verifier_case_machine::VERIFIER_CASE_MACHINE,
    );
}
// spec: MEDULLA-DECISION-ENGINE-R831.1
#[test]
fn verifiercase_machine_classes_extend_core() {
    sm::assert_extends_core("VerifierCase");
}

// spec: MEDULLA-DECISION-ENGINE-R830.6
#[test]
fn loop_machine_matches_statechart() {
    sm::assert_matches_chart(
        "LoopMachine",
        &medulla_store::loop_statechart::loop_statechart_def(),
    );
}

// spec: MEDULLA-DECISION-ENGINE-R830.6
#[test]
fn loop_machine_classes_extend_core() {
    sm::assert_extends_core("Loop");
}

// spec: MEDULLA-DURABLE-KERNEL-R874.6
#[test]
fn exact_retirement_machine_matches_edge_table() {
    sm::assert_matches(&medulla_direct_state::direct_state::EXACT_RETIREMENT_MACHINE);
    sm::assert_extends_core("ExactRetirement");
}

// spec: MEDULLA-DURABLE-KERNEL-R874.21
#[test]
fn retired_commit_and_projection_machines_are_absent() {
    for line in MACHINES.lines() {
        for retired in [":Commit", ":Projection"] {
            assert!(
                !line.starts_with(retired),
                "a retired machine term remains: {line}"
            );
        }
        for comment in [
            "# spec: MEDULLA-DURABLE-KERNEL-R874.2",
            "# spec: MEDULLA-DURABLE-KERNEL-R874.16",
        ] {
            assert_ne!(line.trim_end(), comment, "a retired spec comment remains");
        }
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R874.10
#[test]
fn mutation_batch_machine_matches_edge_table() {
    sm::assert_matches(&medulla_protocol_types::mutation_batch::MUTATION_BATCH_MACHINE);
    sm::assert_extends_core("MutationBatch");
}

// spec: MEDULLA-DURABLE-KERNEL-R874.13
#[test]
fn outbox_delivery_machine_matches_edge_table() {
    sm::assert_matches(&medulla_mutation_types::outbox_delivery_machine::OUTBOX_DELIVERY_MACHINE);
    sm::assert_extends_core("OutboxDelivery");
}

// spec: MEDULLA-DURABLE-KERNEL-R874.14
#[test]
fn parent_transaction_machine_matches_edge_table() {
    sm::assert_matches(
        &medulla_mutation_types::parent_transaction_machine::PARENT_TRANSACTION_MACHINE,
    );
    sm::assert_extends_core("ParentTransaction");
}

// spec: MEDULLA-DURABLE-KERNEL-R874.15
#[test]
fn participant_transaction_machine_matches_edge_table() {
    sm::assert_matches(
        &medulla_mutation_types::participant_transaction_machine::PARTICIPANT_TRANSACTION_MACHINE,
    );
    sm::assert_extends_core("ParticipantTransaction");
}

// spec: MEDULLA-DECISION-ENGINE-R173.7
#[test]
fn edit_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::edit_machine::EDIT_MACHINE);
}

// spec: MEDULLA-DECISION-ENGINE-R173.7
#[test]
fn edit_machine_classes_extend_core() {
    sm::assert_extends_core("Edit");
}

// spec: MEDULLA-DURABLE-KERNEL-R874.18
#[test]
fn semantic_machine_matches_edge_table() {
    sm::assert_matches(&medulla_index_types::semantic_index::SEMANTIC_MACHINE);
    sm::assert_extends_core("Semantic");
}


// spec: MEDULLA-DURABLE-KERNEL-R874.4
#[test]
fn direct_state_install_machine_matches_edge_table() {
    sm::assert_matches(&medulla_direct_state::direct_state::DIRECT_STATE_INSTALL_MACHINE);
    sm::assert_extends_core("DirectStateInstall");
}

// spec: MEDULLA-DURABLE-KERNEL-R874.17
#[test]
fn semantic_source_reconciliation_machine_matches_edge_table() {
    sm::assert_matches(
        &medulla_ann_codes::semantic_ann_codes::semantic_source_reconciliation_machine::SEMANTIC_SOURCE_RECONCILIATION_MACHINE,
    );
    sm::assert_extends_core("SemanticSourceReconciliation");
}

// spec: MEDULLA-DATAPLAT-VOCAB-R009.1
#[test]
fn access_request_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::access_request_machine::ACCESS_REQUEST_MACHINE);
}
// spec: MEDULLA-DATAPLAT-VOCAB-R009.1
#[test]
fn access_request_machine_classes_extend_core() {
    sm::assert_extends_core("AccessRequest");
}

// spec: MEDULLA-DATAPLAT-VOCAB-R009.2
#[test]
fn change_request_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::change_request_machine::CHANGE_REQUEST_MACHINE);
}
// spec: MEDULLA-DATAPLAT-VOCAB-R009.2
#[test]
fn change_request_machine_classes_extend_core() {
    sm::assert_extends_core("ChangeRequest");
}

// spec: MEDULLA-DATAPLAT-VOCAB-R009.3
#[test]
fn durable_unit_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::durable_unit_machine::DURABLE_UNIT_MACHINE);
}
// spec: MEDULLA-DATAPLAT-VOCAB-R009.3
#[test]
fn durable_unit_machine_classes_extend_core() {
    sm::assert_extends_core("DurableUnit");
}

// spec: MEDULLA-DATAPLAT-VOCAB-R009.4
#[test]
fn glossary_term_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::glossary_term_machine::GLOSSARY_TERM_MACHINE);
}
// spec: MEDULLA-DATAPLAT-VOCAB-R009.4
#[test]
fn glossary_term_machine_classes_extend_core() {
    sm::assert_extends_core("GlossaryTerm");
}

// spec: MEDULLA-DATAPLAT-VOCAB-R009.5
#[test]
fn ontology_proposal_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::ontology_proposal_machine::ONTOLOGY_PROPOSAL_MACHINE);
}
// spec: MEDULLA-DATAPLAT-VOCAB-R009.5
#[test]
fn ontology_proposal_machine_classes_extend_core() {
    sm::assert_extends_core("OntologyProposal");
}

// spec: MEDULLA-BUSINESS-OPS-R001.5
#[test]
fn obligation_instance_machine_matches_edge_table() {
    sm::assert_matches(&medulla_core::obligation_machine::OBLIGATION_INSTANCE_MACHINE);
}
// spec: MEDULLA-BUSINESS-OPS-R001.5
#[test]
fn obligation_instance_machine_classes_extend_core() {
    sm::assert_extends_core("ObligationInstance");
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R768.7
#[test]
fn session_machine_matches_edge_table() {
    sm::assert_matches(&medulla_server_ops::api::query_txn::session_machine::SESSION_MACHINE);
}
// spec: MEDULLA-UNIFIED-DATA-PLANE-R768.7
#[test]
fn session_machine_classes_extend_core() {
    sm::assert_extends_core("Session");
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R768.6
#[test]
fn lifecycle_machine_matches_edge_table() {
    sm::assert_matches(&medulla_modality::served::LIFECYCLE_MACHINE);
}
// spec: MEDULLA-UNIFIED-DATA-PLANE-R768.6
#[test]
fn lifecycle_machine_classes_extend_core() {
    sm::assert_extends_core("Lifecycle");
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R906.1
#[test]
fn innovation_shift_machine_matches_edge_table() {
    sm::assert_matches(&medulla_numeric::series::state_space::INNOVATION_SHIFT_MACHINE);
}
// spec: MEDULLA-FINANCE-PRIMITIVES-R906.1
#[test]
fn innovation_shift_machine_classes_extend_core() {
    sm::assert_extends_core("InnovationShift");
}

// spec: MEDULLA-BUSINESS-OPS-R001.7
#[test]
fn approval_request_machine_matches_edge_table() {
    sm::assert_matches(&medulla_core::approval_machine::APPROVAL_REQUEST_MACHINE);
}
// spec: MEDULLA-BUSINESS-OPS-R001.7
#[test]
fn approval_request_machine_classes_extend_core() {
    sm::assert_extends_core("ApprovalRequest");
}

// spec: MEDULLA-CONTRACT-R062.2
#[test]
fn hardware_job_machine_matches_edge_table() {
    sm::assert_matches(&medulla_quantum_hardware::job_machine::HARDWARE_JOB_MACHINE);
}

// spec: MEDULLA-CONTRACT-R062.2
#[test]
fn hardware_job_machine_classes_extend_core() {
    sm::assert_extends_core("HardwareJob");
}

// spec: MEDULLA-DATAPLAT-VOCAB-R009.6
#[test]
fn quality_rule_machine_matches_edge_table() {
    sm::assert_matches(&medulla_types::quality_rule_machine::QUALITY_RULE_MACHINE);
}

// spec: MEDULLA-DATAPLAT-VOCAB-R009.6
#[test]
fn quality_rule_machine_classes_extend_core() {
    sm::assert_extends_core("QualityRule");
}

// spec: MEDULLA-PLACEMENT-R783.8
#[test]
fn move_stage_machine_matches_edge_table() {
    sm::assert_matches(&medulla_raft::raft::placement::MOVE_STAGE_MACHINE);
}
// spec: MEDULLA-PLACEMENT-R783.8
#[test]
fn move_stage_machine_classes_extend_core() {
    sm::assert_extends_core("MoveStage");
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R768.4
#[test]
fn series_projection_machine_matches_edge_table() {
    sm::assert_matches(&medulla_tsdb::projection_machine::SERIES_PROJECTION_MACHINE);
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R768.4
#[test]
fn series_projection_machine_classes_extend_core() {
    sm::assert_extends_core("SeriesProjection");
}

// spec: MEDULLA-IDENTITY-R724.1
#[test]
fn application_admission_machine_matches_edge_table() {
    sm::assert_matches(
        &medulla_identity_types::application_admission_machine::APPLICATION_ADMISSION_MACHINE,
    );
}

// spec: MEDULLA-IDENTITY-R724.1
#[test]
fn application_admission_machine_classes_extend_core() {
    sm::assert_extends_core("ApplicationAdmission");
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R016.1
#[test]
fn materialization_status_machine_matches_edge_table() {
    sm::assert_matches(
        &medulla_types::materialization_status_machine::MATERIALIZATION_STATUS_MACHINE,
    );
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R016.1
#[test]
fn materialization_status_machine_classes_extend_core() {
    sm::assert_extends_core("MaterializationStatus");
}

// spec: MEDULLA-PLACEMENT-R784.1
#[test]
fn circuit_breaker_machine_matches_edge_table() {
    sm::assert_matches(&medulla_resource::circuit_breaker_machine::CIRCUIT_BREAKER_MACHINE);
}

// spec: MEDULLA-PLACEMENT-R784.1
#[test]
fn circuit_breaker_machine_classes_extend_core() {
    sm::assert_extends_core("CircuitBreaker");
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R016.2
#[test]
fn materialization_machine_matches_edge_table() {
    sm::assert_matches(&medulla_core::materialization_machine::MATERIALIZATION_MACHINE);
    sm::assert_extends_core("Materialization");
}

// spec: MEDULLA-DECISION-ENGINE-R708.2
#[test]
fn knowledge_flag_machine_matches_edge_table() {
    sm::assert_matches(&medulla_decision_types::knowledge_flag::KNOWLEDGE_FLAG_MACHINE);
    sm::assert_extends_core("KnowledgeFlag");
}

// spec: MEDULLA-DECISION-ENGINE-R710.2
#[test]
fn friction_machine_matches_edge_table() {
    sm::assert_matches(&medulla_decision_types::friction::FRICTION_MACHINE);
    sm::assert_extends_core("Friction");
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R011.5
#[test]
fn finance_receipt_machine_matches_edge_table() {
    sm::assert_matches(&medulla_finance_types::finance_receipt::FINANCE_RECEIPT_MACHINE);
}
// spec: MEDULLA-FINANCE-PRIMITIVES-R011.5
#[test]
fn finance_receipt_machine_classes_extend_core() {
    sm::assert_extends_core("FinanceReceipt");
}

// spec: MEDULLA-DECISION-ENGINE-R901
#[test]
fn agent_goal_machine_matches_edge_table() {
    sm::assert_matches(&medulla_source_types::agent_goal::GOAL_MACHINE);
}
// spec: MEDULLA-DECISION-ENGINE-R901
#[test]
fn agent_goal_machine_classes_extend_core() {
    sm::assert_extends_core("AgentGoal");
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R790
#[test]
fn connector_run_machine_matches_edge_table() {
    sm::assert_matches(&medulla_source_types::connector_run::CONNECTOR_RUN_MACHINE);
}
// spec: MEDULLA-UNIFIED-DATA-PLANE-R790
#[test]
fn connector_run_machine_classes_extend_core() {
    sm::assert_extends_core("ConnectorRun");
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R791
#[test]
fn source_mapping_machine_matches_edge_table() {
    sm::assert_matches(&medulla_source_types::source_connection::SOURCE_MAPPING_MACHINE);
}
// spec: MEDULLA-UNIFIED-DATA-PLANE-R791
#[test]
fn source_mapping_machine_classes_extend_core() {
    sm::assert_extends_core("SourceMapping");
}

// spec: MEDULLA-OBSERVE-R004.5
#[test]
fn observe_machines_match_edge_tables() {
    // The ontology instances carry the prefix Observe: `:TaskMachine` already names the
    // lifecycle of a work-plan task, and a second instance of that name would merge the two.
    sm::assert_matches_as(&medulla_observe::machine::task::TASK_MACHINE, "ObserveTaskMachine");
    sm::assert_matches_as(
        &medulla_observe::machine::readiness::READINESS_MACHINE,
        "ObserveReadinessMachine",
    );
    sm::assert_matches_as(
        &medulla_observe::machine::request::REQUEST_MACHINE,
        "ObserveRequestMachine",
    );
    for prefix in ["ObserveTask", "ObserveReadiness", "ObserveRequest"] {
        sm::assert_extends_core(prefix);
    }
    assert!(!MACHINES.contains("owl:imports"));
}
