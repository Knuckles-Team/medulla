//! MEDULLA-IDENTITY-R721.1 — the user status machine: one test per edge and refusal.

use crate::identity::model::{UserStatus, UserStatusEvent, USER_STATUS_MACHINE};
use medulla_types::transition_refused::{MachineState, TransitionRefusalReason};

fn edge_case(id: &str, from: UserStatus, event: UserStatusEvent, to: UserStatus) {
    let edge = USER_STATUS_MACHINE
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
fn illegal_pairs() -> Vec<(UserStatus, UserStatusEvent)> {
    let machine = &USER_STATUS_MACHINE;
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

#[test]
fn edge_u01_active_disable() {
    edge_case(
        "U01",
        UserStatus::Active,
        UserStatusEvent::Disable,
        UserStatus::Disabled,
    );
}

#[test]
fn edge_u02_disabled_enable() {
    edge_case(
        "U02",
        UserStatus::Disabled,
        UserStatusEvent::Enable,
        UserStatus::Active,
    );
}

#[test]
fn edge_u03_active_require_reset() {
    edge_case(
        "U03",
        UserStatus::Active,
        UserStatusEvent::RequireReset,
        UserStatus::PendingReset,
    );
}

#[test]
fn edge_u04_disabled_require_reset() {
    edge_case(
        "U04",
        UserStatus::Disabled,
        UserStatusEvent::RequireReset,
        UserStatus::PendingReset,
    );
}

#[test]
fn edge_u05_pending_reset_complete_reset() {
    edge_case(
        "U05",
        UserStatus::PendingReset,
        UserStatusEvent::CompleteReset,
        UserStatus::Active,
    );
}

#[test]
fn edge_u06_pending_reset_disable() {
    edge_case(
        "U06",
        UserStatus::PendingReset,
        UserStatusEvent::Disable,
        UserStatus::Disabled,
    );
}

#[test]
fn edge_u07_pending_verification_verify() {
    edge_case(
        "U07",
        UserStatus::PendingVerification,
        UserStatusEvent::Verify,
        UserStatus::Active,
    );
}

#[test]
fn edge_u08_pending_verification_disable() {
    edge_case(
        "U08",
        UserStatus::PendingVerification,
        UserStatusEvent::Disable,
        UserStatus::Disabled,
    );
}

#[test]
fn edge_u09_active_deprovision() {
    edge_case(
        "U09",
        UserStatus::Active,
        UserStatusEvent::Deprovision,
        UserStatus::Deprovisioned,
    );
}

#[test]
fn edge_u10_disabled_deprovision() {
    edge_case(
        "U10",
        UserStatus::Disabled,
        UserStatusEvent::Deprovision,
        UserStatus::Deprovisioned,
    );
}

#[test]
fn edge_u11_pending_reset_deprovision() {
    edge_case(
        "U11",
        UserStatus::PendingReset,
        UserStatusEvent::Deprovision,
        UserStatus::Deprovisioned,
    );
}

#[test]
fn edge_u12_pending_verification_deprovision() {
    edge_case(
        "U12",
        UserStatus::PendingVerification,
        UserStatusEvent::Deprovision,
        UserStatus::Deprovisioned,
    );
}

#[test]
fn every_illegal_pair_is_refused() {
    let machine = &USER_STATUS_MACHINE;
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

#[test]
fn refusal_carries_machine_from_event() {
    let refused = UserStatus::Active
        .apply(UserStatusEvent::Enable, |_| true)
        .unwrap_err();
    assert_eq!(refused.machine, "UserStatusMachine");
    assert_eq!(refused.from, UserStatus::Active.stored());
    assert_eq!(refused.event, "Enable");
    assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
}

#[test]
fn every_transition_and_refusal_is_covered() {
    let machine = &USER_STATUS_MACHINE;
    assert_eq!(machine.edges.len(), 12);
    assert_eq!(machine.states.len(), 5);
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
