//! The medulla machines other than WorkItem, described in `state-machines-v1.ttl`, match
//! their Rust edge tables, and every machine class extends a core state-machine class
//! (MEDULLA-ORCH-VOCAB-R811.6, organization constitution Article XXXI). The core vocabulary
//! and the WorkItem machine are checked in `state_machine_vocabulary.rs`.
#![cfg(feature = "rdf")]

mod common;

use std::collections::BTreeSet;

use common::{parse_all, KG, RDFS};
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

fn rust_edges<S: MachineState, E: MachineEvent, G>(machine: &MachineDef<S, E, G>) -> BTreeSet<Row> {
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

fn assert_matches<S: MachineState + std::fmt::Debug, E: MachineEvent, G>(
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

fn assert_extends_core(prefix: &str) {
    let triples = triples();
    let sub_class_of = format!("{RDFS}subClassOf");
    for (kind, core) in KINDS.iter().zip(CORE) {
        let class = NamedNode::new(kg(&format!("{prefix}{kind}"))).unwrap();
        let parents: Vec<&str> = triples
            .iter()
            .filter(|t| t.subject == class.clone().into() && t.predicate.as_str() == sub_class_of)
            .map(|t| iri(&t.object))
            .collect();
        assert!(
            parents.contains(&kg(core).as_str()),
            "{class} must extend :{core}, has {parents:?}"
        );
    }
}

#[test]
fn machine_file_has_no_owl_imports() {
    assert!(!TTL.contains("owl:imports"));
}

#[test]
fn governed_change_machine_matches_edge_table() {
    assert_matches(&medulla_mutation_types::governed_change::GOVERNED_CHANGE_MACHINE);
}
#[test]
fn governed_change_machine_classes_extend_core() {
    assert_extends_core("GovernedChange");
}

#[test]
fn user_status_machine_matches_edge_table() {
    assert_matches(&medulla_identity_types::identity::USER_STATUS_MACHINE);
}
#[test]
fn user_status_machine_classes_extend_core() {
    assert_extends_core("UserStatus");
}

#[test]
fn rbac_elevation_machine_matches_legal_table() {
    assert_matches(&medulla_identity_types::rbac_elevation::RBAC_ELEVATION_MACHINE);
}
#[test]
fn rbac_elevation_machine_classes_extend_core() {
    assert_extends_core("RbacElevation");
}

#[test]
fn control_lease_machine_matches_edge_table() {
    assert_matches(&medulla_work_types::control_lease::CONTROL_LEASE_MACHINE);
}
#[test]
fn control_lease_machine_classes_extend_core() {
    assert_extends_core("ControlLease");
}

#[test]
fn resource_reservation_machine_matches_edge_table() {
    assert_matches(&medulla_result_types::epistemic_operations::RESOURCE_RESERVATION_MACHINE);
}
#[test]
fn resource_reservation_machine_classes_extend_core() {
    assert_extends_core("ResourceReservation");
}

#[test]
fn membership_shrink_machine_matches_transition_is_valid() {
    assert_matches(&medulla_raft::raft::membership_shrink::MEMBERSHIP_SHRINK_MACHINE);
}
#[test]
fn membership_shrink_machine_classes_extend_core() {
    assert_extends_core("MembershipShrink");
}

#[test]
fn broker_message_machine_matches_edge_table() {
    assert_matches(&medulla_core::broker::message_machine::BROKER_MESSAGE_MACHINE);
}
#[test]
fn broker_message_machine_classes_extend_core() {
    assert_extends_core("BrokerMessage");
}

#[test]
fn defect_machine_matches_edge_table() {
    assert_matches(&medulla_source_types::defect_machine::DEFECT_MACHINE);
}
#[test]
fn defect_machine_classes_extend_core() {
    assert_extends_core("Defect");
}

#[test]
fn semantic_index_upgrade_machine_matches_edge_table() {
    assert_matches(&medulla_h_retrieve::semantic_index_runtime::SEMANTIC_INDEX_UPGRADE_MACHINE);
}
#[test]
fn semantic_index_upgrade_machine_classes_extend_core() {
    assert_extends_core("SemanticIndexUpgrade");
}

#[test]
fn shard_drain_machine_matches_edge_table() {
    assert_matches(&medulla_raft_xshard::drain::SHARD_DRAIN_MACHINE);
}
#[test]
fn shard_drain_machine_classes_extend_core() {
    assert_extends_core("ShardDrain");
}

#[test]
fn capacity_lease_machine_matches_edge_table() {
    assert_matches(&medulla_mutation_types::capacity_lease::CAPACITY_LEASE_MACHINE);
}
#[test]
fn capacity_lease_machine_classes_extend_core() {
    assert_extends_core("CapacityLease");
}
