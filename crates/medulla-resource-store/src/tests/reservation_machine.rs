//! MEDULLA-PLACEMENT-R781.1 — the resource reservation machine: one test per edge and refusal.

use medulla_result_types::epistemic_operations::{
    ReservationEvent, ResourceReservationRecordState, RESOURCE_RESERVATION_MACHINE,
};
use medulla_types::transition_refused::{MachineState, TransitionRefusalReason};

fn edge_case(
    id: &str,
    from: ResourceReservationRecordState,
    event: ReservationEvent,
    to: ResourceReservationRecordState,
) {
    let edge = RESOURCE_RESERVATION_MACHINE
        .edges
        .iter()
        .find(|e| e.id == id)
        .unwrap();
    assert_eq!((edge.from, edge.event, edge.to), (from, event, to));
    assert_eq!(from.apply(event, |_| true), Ok(to));
    if edge.guard.is_some() {
        let refused = from.apply(event, |_| false).unwrap_err();
        assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
    }
}

/// Every (state, event) pair with no edge.
fn illegal_pairs() -> Vec<(ResourceReservationRecordState, ReservationEvent)> {
    let machine = &RESOURCE_RESERVATION_MACHINE;
    let mut out = Vec::new();
    for &state in machine.states {
        for &event in machine.events {
            if !machine
                .edges
                .iter()
                .any(|e| e.from == state && e.event == event)
            {
                out.push((state, event));
            }
        }
    }
    out
}

// spec: MEDULLA-PLACEMENT-R781.1

#[test]
fn edge_v01_absent_reserve() {
    edge_case(
        "V01",
        ResourceReservationRecordState::Absent,
        ReservationEvent::Reserve,
        ResourceReservationRecordState::Reserved,
    );
}

// spec: MEDULLA-PLACEMENT-R781.1

#[test]
fn edge_v02_reserved_release() {
    edge_case(
        "V02",
        ResourceReservationRecordState::Reserved,
        ReservationEvent::Release,
        ResourceReservationRecordState::Released,
    );
}

// spec: MEDULLA-PLACEMENT-R781.1

#[test]
fn edge_v03_reserved_reclaim() {
    edge_case(
        "V03",
        ResourceReservationRecordState::Reserved,
        ReservationEvent::Reclaim,
        ResourceReservationRecordState::Reclaimed,
    );
}

// spec: MEDULLA-PLACEMENT-R781.1

#[test]
fn edge_v04_reserved_expire() {
    edge_case(
        "V04",
        ResourceReservationRecordState::Reserved,
        ReservationEvent::Expire,
        ResourceReservationRecordState::Expired,
    );
}

// spec: MEDULLA-PLACEMENT-R781.1

#[test]
fn edge_v05_reserved_supersede() {
    edge_case(
        "V05",
        ResourceReservationRecordState::Reserved,
        ReservationEvent::Supersede,
        ResourceReservationRecordState::Superseded,
    );
}

// spec: MEDULLA-PLACEMENT-R781.1

#[test]
fn every_illegal_pair_is_refused() {
    let machine = &RESOURCE_RESERVATION_MACHINE;
    let pairs = illegal_pairs();
    assert!(!pairs.is_empty());
    for (state, event) in pairs {
        let refused = state.apply(event, |_| true).unwrap_err();
        let expected = if machine.is_terminal(state) {
            TransitionRefusalReason::TerminalState
        } else {
            TransitionRefusalReason::IllegalTransition
        };
        assert_eq!(refused.reason, expected, "{state:?} {event:?}");
    }
    for &terminal in machine.terminals {
        for &event in machine.events {
            assert!(terminal.apply(event, |_| true).is_err());
        }
    }
}

// spec: MEDULLA-PLACEMENT-R781.1

#[test]
fn refusal_carries_machine_from_event() {
    let refused = ResourceReservationRecordState::Absent
        .apply(ReservationEvent::Release, |_| true)
        .unwrap_err();
    assert_eq!(refused.machine, "ResourceReservationMachine");
    assert_eq!(
        refused.from,
        ResourceReservationRecordState::Absent.stored()
    );
    assert_eq!(refused.event, "Release");
    assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
}

/// Lines that assign the state field directly (`.<field> = <Type>::`), outside the
/// file's `#[cfg(test)] mod tests`.
fn direct_state_writes<'a>(source: &'a str, field: &str, ty: &str) -> Vec<&'a str> {
    let needle = format!(".{field} = {ty}::");
    let mut out = Vec::new();
    let mut previous = "";
    for line in source.lines().map(str::trim) {
        if previous == "#[cfg(test)]" && line.starts_with("mod tests") {
            break;
        }
        previous = line;
        if !line.starts_with("//") && line.contains(&needle) {
            out.push(line);
        }
    }
    out
}

// spec: MEDULLA-PLACEMENT-R781.1

#[test]
fn no_writer_sets_state_outside_apply() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/redb_store/resource/write_lifecycle.rs"
    );
    let source = std::fs::read_to_string(path).expect("writer source");
    let writes = direct_state_writes(&source, "state", "ResourceReservationRecordState");
    assert!(
        writes.is_empty(),
        "direct state writes in src/redb_store/resource/write_lifecycle.rs: {writes:?}"
    );
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/redb_store/resource/codec.rs"
    );
    let source = std::fs::read_to_string(path).expect("writer source");
    let writes = direct_state_writes(&source, "state", "ResourceReservationRecordState");
    assert!(
        writes.is_empty(),
        "direct state writes in src/redb_store/resource/codec.rs: {writes:?}"
    );
}

// spec: MEDULLA-PLACEMENT-R781.1

#[test]
fn every_transition_and_refusal_is_covered() {
    let machine = &RESOURCE_RESERVATION_MACHINE;
    assert_eq!(machine.edges.len(), 5);
    assert_eq!(machine.states.len(), 6);
    let mut ids: Vec<_> = machine.edges.iter().map(|e| e.id).collect();
    ids.dedup();
    assert_eq!(ids.len(), machine.edges.len(), "edge ids are unique");
    for edge in machine.edges {
        assert!(
            !machine.is_terminal(edge.from),
            "{} leaves a terminal state",
            edge.id
        );
    }
    for &state in machine.states {
        assert_eq!(machine.state_from_stored(state.stored()), Some(state));
        for &event in machine.events {
            let legal = state.apply(event, |_| true).is_ok();
            let has_edge = machine.edges_from(state).any(|e| e.event == event);
            assert_eq!(legal, has_edge, "{state:?} {event:?}");
        }
    }
}
