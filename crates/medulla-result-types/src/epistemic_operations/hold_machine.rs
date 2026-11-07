//! MEDULLA-PLACEMENT-R780.1 — the development lane hold machine (`LaneHoldMachine`).
//!
//! A state belongs in the machine when it is persisted and observable with distinct edges;
//! anything else is folded into an existing state. The machine has five states: Absent (the
//! start, no record), Active, Expired, CleanupPending and Cleaned. The table below is the
//! only one: the spec row and `state-machines-v1.ttl` carry the same edges.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use super::development_state::DevelopmentLaneHoldState as HoldState;

/// An event of `LaneHoldMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HoldEvent {
    /// A reservation is admitted and the hold is written.
    Reserve,
    /// The lifecycle work item reached a terminal state: the hold is released and its
    /// cleanup is owed.
    Finish,
    /// The hold time limit passed before the hold was finished.
    Expire,
    /// The hold is renewed in place.
    Renew,
    /// The cleanup work item removed the hold's resources.
    Cleanup,
}

impl MachineState for HoldState {
    fn stored(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Active => "active",
            Self::Expired => "expired",
            Self::CleanupPending => "cleanup_pending",
            Self::Cleaned => "cleaned",
        }
    }
}

impl MachineEvent for HoldEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Reserve => "Reserve",
            Self::Finish => "Finish",
            Self::Expire => "Expire",
            Self::Renew => "Renew",
            Self::Cleanup => "Cleanup",
        }
    }
}

/// Guard of `LaneHoldMachine`: the reservation request is admitted.
pub const HOLD_GUARD_RESERVATION_ADMITTED: &str = "ReservationAdmitted";
/// Guard of `LaneHoldMachine`: the work item machine reached a terminal state.
pub const HOLD_GUARD_WORK_ITEM_TERMINAL: &str = "WorkItemTerminal";
/// Guard of `LaneHoldMachine`: the hold time limit has passed.
pub const HOLD_GUARD_TIME_LIMIT_PASSED: &str = "TimeLimitPassed";
/// Guard of `LaneHoldMachine`: the observation is fresh and the time limit has not passed.
pub const HOLD_GUARD_LIVE_AND_FRESH: &str = "LiveAndFresh";
/// Guard of `LaneHoldMachine`: the cleanup request names this hold and its expected revision.
pub const HOLD_GUARD_CLEANUP_HOLD_MATCHES: &str = "CleanupHoldMatches";

/// `LaneHoldMachine`: the closed edge table. Every other pair of state and event is refused.
pub static LANE_HOLD_MACHINE: MachineDef<HoldState, HoldEvent, &'static str> = MachineDef {
    name: "LaneHoldMachine",
    states: &[
        HoldState::Absent,
        HoldState::Active,
        HoldState::Expired,
        HoldState::CleanupPending,
        HoldState::Cleaned,
    ],
    initial: HoldState::Absent,
    terminals: &[HoldState::Cleaned],
    events: &[
        HoldEvent::Reserve,
        HoldEvent::Finish,
        HoldEvent::Expire,
        HoldEvent::Renew,
        HoldEvent::Cleanup,
    ],
    guards: &[
        HOLD_GUARD_RESERVATION_ADMITTED,
        HOLD_GUARD_WORK_ITEM_TERMINAL,
        HOLD_GUARD_TIME_LIMIT_PASSED,
        HOLD_GUARD_LIVE_AND_FRESH,
        HOLD_GUARD_CLEANUP_HOLD_MATCHES,
    ],
    edges: &[
        Edge {
            id: "H01",
            from: HoldState::Absent,
            event: HoldEvent::Reserve,
            guard: Some(HOLD_GUARD_RESERVATION_ADMITTED),
            to: HoldState::Active,
        },
        Edge {
            id: "H02",
            from: HoldState::Active,
            event: HoldEvent::Finish,
            guard: Some(HOLD_GUARD_WORK_ITEM_TERMINAL),
            to: HoldState::CleanupPending,
        },
        Edge {
            id: "H03",
            from: HoldState::Active,
            event: HoldEvent::Expire,
            guard: Some(HOLD_GUARD_TIME_LIMIT_PASSED),
            to: HoldState::Expired,
        },
        Edge {
            id: "H04",
            from: HoldState::CleanupPending,
            event: HoldEvent::Cleanup,
            guard: Some(HOLD_GUARD_CLEANUP_HOLD_MATCHES),
            to: HoldState::Cleaned,
        },
        Edge {
            id: "H05",
            from: HoldState::Expired,
            event: HoldEvent::Cleanup,
            guard: Some(HOLD_GUARD_CLEANUP_HOLD_MATCHES),
            to: HoldState::Cleaned,
        },
        Edge {
            id: "H06",
            from: HoldState::Active,
            event: HoldEvent::Renew,
            guard: Some(HOLD_GUARD_LIVE_AND_FRESH),
            to: HoldState::Active,
        },
    ],
};

impl HoldState {
    /// The one transition function of `LaneHoldMachine`. `guard_holds` answers each guard by
    /// name; the caller has already evaluated the condition the guard names.
    pub fn apply(
        self,
        event: HoldEvent,
        guard_holds: impl Fn(&'static str) -> bool,
    ) -> Result<Self, TransitionRefused> {
        transition(&LANE_HOLD_MACHINE, self, event, guard_holds).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    fn edge_case(id: &str, from: HoldState, event: HoldEvent, to: HoldState) {
        let edge = LANE_HOLD_MACHINE.edges.iter().find(|e| e.id == id).unwrap();
        assert_eq!((edge.from, edge.event, edge.to), (from, event, to));
        assert_eq!(from.apply(event, |_| true), Ok(to));
        let refused = from.apply(event, |_| false).unwrap_err();
        assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
    }

    // spec: MEDULLA-PLACEMENT-R780.1
    #[test]
    fn edge_h01_absent_reserve() {
        edge_case(
            "H01",
            HoldState::Absent,
            HoldEvent::Reserve,
            HoldState::Active,
        );
    }

    // spec: MEDULLA-PLACEMENT-R780.1
    #[test]
    fn edge_h02_active_finish() {
        edge_case(
            "H02",
            HoldState::Active,
            HoldEvent::Finish,
            HoldState::CleanupPending,
        );
    }

    // spec: MEDULLA-PLACEMENT-R780.1
    #[test]
    fn edge_h03_active_expire() {
        edge_case(
            "H03",
            HoldState::Active,
            HoldEvent::Expire,
            HoldState::Expired,
        );
    }

    // spec: MEDULLA-PLACEMENT-R780.1
    #[test]
    fn edge_h04_cleanup_pending_cleanup() {
        edge_case(
            "H04",
            HoldState::CleanupPending,
            HoldEvent::Cleanup,
            HoldState::Cleaned,
        );
    }

    // spec: MEDULLA-PLACEMENT-R780.1
    #[test]
    fn edge_h05_expired_cleanup() {
        edge_case(
            "H05",
            HoldState::Expired,
            HoldEvent::Cleanup,
            HoldState::Cleaned,
        );
    }

    // spec: MEDULLA-PLACEMENT-R780.1
    #[test]
    fn edge_h06_active_renew() {
        edge_case(
            "H06",
            HoldState::Active,
            HoldEvent::Renew,
            HoldState::Active,
        );
    }

    // spec: MEDULLA-PLACEMENT-R780.1
    #[test]
    fn every_illegal_pair_is_refused() {
        let machine = &LANE_HOLD_MACHINE;
        for &state in machine.states {
            for &event in machine.events {
                let has_edge = machine
                    .edges
                    .iter()
                    .any(|e| e.from == state && e.event == event);
                let result = state.apply(event, |_| true);
                assert_eq!(result.is_ok(), has_edge, "{state:?} {event:?}");
                if !has_edge {
                    let expected = if machine.is_terminal(state) {
                        TransitionRefusalReason::TerminalState
                    } else {
                        TransitionRefusalReason::IllegalTransition
                    };
                    assert_eq!(result.unwrap_err().reason, expected, "{state:?} {event:?}");
                }
            }
        }
        // A finish of a Cleaned hold is refused: Cleaned is terminal.
        assert!(HoldState::Cleaned
            .apply(HoldEvent::Finish, |_| true)
            .is_err());
    }

    // spec: MEDULLA-PLACEMENT-R780.1
    #[test]
    fn refusal_carries_machine_from_event() {
        let refused = HoldState::Cleaned
            .apply(HoldEvent::Finish, |_| true)
            .unwrap_err();
        assert_eq!(refused.machine, "LaneHoldMachine");
        assert_eq!(refused.from, "cleaned");
        assert_eq!(refused.event, "Finish");
        assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
    }

    // spec: MEDULLA-PLACEMENT-R780
    #[test]
    fn every_transition_and_refusal_is_covered() {
        let machine = &LANE_HOLD_MACHINE;
        assert_eq!(machine.states.len(), 5);
        assert_eq!(machine.edges.len(), 6);
        for &state in machine.states {
            assert_eq!(machine.state_from_stored(state.stored()), Some(state));
        }
    }

    // spec: MEDULLA-PLACEMENT-R780.1
    #[test]
    fn stored_text_is_the_wire_text() {
        for &state in LANE_HOLD_MACHINE.states {
            let wire = serde_json::to_string(&state).unwrap();
            assert_eq!(wire, format!("\"{}\"", state.stored()));
        }
    }
}
