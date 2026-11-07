//! MEDULLA-SDLC-R030.1 — the defect lifecycle as one explicit state machine.
//!
//! `DefectStatus::apply` is the one transition function. The machine has no terminal
//! state: Closed and Rejected leave only by Reopen. New to Verified is refused.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};

use crate::lifecycle::DefectStatus;

/// The facts the defect guards read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DefectFacts {
    /// A fix reference is recorded (guard of D03).
    pub fix_reference_recorded: bool,
    /// Verification evidence is recorded (guard of D04).
    pub verification_evidence_recorded: bool,
    /// A reason is recorded (guard of D06 to D09).
    pub reason_recorded: bool,
}

// DefectMachine: the row's edge table. The ontology test compares it with state-machines-v1.ttl.
/// An event of `DefectMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DefectEvent {
    /// The `Triage` event.
    Triage,
    /// The `StartFix` event.
    StartFix,
    /// The `Fix` event.
    Fix,
    /// The `Verify` event.
    Verify,
    /// The `Close` event.
    Close,
    /// The `Reject` event.
    Reject,
    /// The `Reopen` event.
    Reopen,
}

impl MachineState for DefectStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Triaged => "triaged",
            Self::Fixing => "fixing",
            Self::Fixed => "fixed",
            Self::Verified => "verified",
            Self::Closed => "closed",
            Self::Rejected => "rejected",
        }
    }
}

impl MachineEvent for DefectEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Triage => "Triage",
            Self::StartFix => "StartFix",
            Self::Fix => "Fix",
            Self::Verify => "Verify",
            Self::Close => "Close",
            Self::Reject => "Reject",
            Self::Reopen => "Reopen",
        }
    }
}

/// Guard of `DefectMachine`: a fix reference is recorded.
pub const DEFECT_GUARD_FIX_REFERENCE_RECORDED: &str = "FixReferenceRecorded";
/// Guard of `DefectMachine`: verification evidence is recorded.
pub const DEFECT_GUARD_VERIFICATION_EVIDENCE_RECORDED: &str = "VerificationEvidenceRecorded";
/// Guard of `DefectMachine`: a reason is recorded.
pub const DEFECT_GUARD_REASON_RECORDED: &str = "ReasonRecorded";

/// `DefectMachine`: the closed edge table. Every other pair of state and event is refused.
pub static DEFECT_MACHINE: MachineDef<DefectStatus, DefectEvent, &'static str> = MachineDef {
    name: "DefectMachine",
    states: &[
        DefectStatus::New,
        DefectStatus::Triaged,
        DefectStatus::Fixing,
        DefectStatus::Fixed,
        DefectStatus::Verified,
        DefectStatus::Closed,
        DefectStatus::Rejected,
    ],
    initial: DefectStatus::New,
    terminals: &[],
    events: &[
        DefectEvent::Triage,
        DefectEvent::StartFix,
        DefectEvent::Fix,
        DefectEvent::Verify,
        DefectEvent::Close,
        DefectEvent::Reject,
        DefectEvent::Reopen,
    ],
    guards: &[
        DEFECT_GUARD_FIX_REFERENCE_RECORDED,
        DEFECT_GUARD_VERIFICATION_EVIDENCE_RECORDED,
        DEFECT_GUARD_REASON_RECORDED,
    ],
    edges: &[
        Edge {
            id: "D01",
            from: DefectStatus::New,
            event: DefectEvent::Triage,
            guard: None,
            to: DefectStatus::Triaged,
        },
        Edge {
            id: "D02",
            from: DefectStatus::Triaged,
            event: DefectEvent::StartFix,
            guard: None,
            to: DefectStatus::Fixing,
        },
        Edge {
            id: "D03",
            from: DefectStatus::Fixing,
            event: DefectEvent::Fix,
            guard: Some(DEFECT_GUARD_FIX_REFERENCE_RECORDED),
            to: DefectStatus::Fixed,
        },
        Edge {
            id: "D04",
            from: DefectStatus::Fixed,
            event: DefectEvent::Verify,
            guard: Some(DEFECT_GUARD_VERIFICATION_EVIDENCE_RECORDED),
            to: DefectStatus::Verified,
        },
        Edge {
            id: "D05",
            from: DefectStatus::Verified,
            event: DefectEvent::Close,
            guard: None,
            to: DefectStatus::Closed,
        },
        Edge {
            id: "D06",
            from: DefectStatus::New,
            event: DefectEvent::Reject,
            guard: Some(DEFECT_GUARD_REASON_RECORDED),
            to: DefectStatus::Rejected,
        },
        Edge {
            id: "D07",
            from: DefectStatus::Triaged,
            event: DefectEvent::Reject,
            guard: Some(DEFECT_GUARD_REASON_RECORDED),
            to: DefectStatus::Rejected,
        },
        Edge {
            id: "D08",
            from: DefectStatus::Closed,
            event: DefectEvent::Reopen,
            guard: Some(DEFECT_GUARD_REASON_RECORDED),
            to: DefectStatus::New,
        },
        Edge {
            id: "D09",
            from: DefectStatus::Rejected,
            event: DefectEvent::Reopen,
            guard: Some(DEFECT_GUARD_REASON_RECORDED),
            to: DefectStatus::New,
        },
    ],
};

impl DefectStatus {
    /// The one transition function of `DefectMachine`. The guards read `facts`.
    pub fn apply(self, event: DefectEvent, facts: DefectFacts) -> Result<Self, TransitionRefused> {
        transition(&DEFECT_MACHINE, self, event, |guard| match guard {
            DEFECT_GUARD_FIX_REFERENCE_RECORDED => facts.fix_reference_recorded,
            DEFECT_GUARD_VERIFICATION_EVIDENCE_RECORDED => facts.verification_evidence_recorded,
            DEFECT_GUARD_REASON_RECORDED => facts.reason_recorded,
            _ => false,
        })
        .map(|edge| edge.to)
    }
}

impl StateMachineApply for DefectStatus {
    type Event = DefectEvent;
    type Facts = DefectFacts;
    fn apply(&self, event: DefectEvent, facts: &DefectFacts) -> Result<Self, TransitionRefused> {
        DefectStatus::apply(*self, event, *facts)
    }
}

#[cfg(test)]
mod tests {
    use super::{DefectEvent, DefectFacts, DefectStatus, DEFECT_MACHINE};
    use medulla_types::transition_refused::{MachineState, TransitionRefusalReason};

    const ALL: DefectFacts = DefectFacts {
        fix_reference_recorded: true,
        verification_evidence_recorded: true,
        reason_recorded: true,
    };

    fn edge_case(id: &str, from: DefectStatus, event: DefectEvent, to: DefectStatus) {
        let edge = DEFECT_MACHINE.edges.iter().find(|e| e.id == id).unwrap();
        assert_eq!((edge.from, edge.event, edge.to), (from, event, to));
        assert_eq!(from.apply(event, ALL), Ok(to));
        if edge.guard.is_some() {
            let refused = from.apply(event, DefectFacts::default()).unwrap_err();
            assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
        }
    }

    /// Every (state, event) pair with no edge.
    fn illegal_pairs() -> Vec<(DefectStatus, DefectEvent)> {
        let machine = &DEFECT_MACHINE;
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

    // spec: MEDULLA-SDLC-R030.1

    #[test]
    fn edge_d01_new_triage() {
        edge_case(
            "D01",
            DefectStatus::New,
            DefectEvent::Triage,
            DefectStatus::Triaged,
        );
    }

    // spec: MEDULLA-SDLC-R030.1

    #[test]
    fn edge_d02_triaged_start_fix() {
        edge_case(
            "D02",
            DefectStatus::Triaged,
            DefectEvent::StartFix,
            DefectStatus::Fixing,
        );
    }

    // spec: MEDULLA-SDLC-R030.1

    #[test]
    fn edge_d03_fixing_fix() {
        edge_case(
            "D03",
            DefectStatus::Fixing,
            DefectEvent::Fix,
            DefectStatus::Fixed,
        );
    }

    // spec: MEDULLA-SDLC-R030.1

    #[test]
    fn edge_d04_fixed_verify() {
        edge_case(
            "D04",
            DefectStatus::Fixed,
            DefectEvent::Verify,
            DefectStatus::Verified,
        );
    }

    // spec: MEDULLA-SDLC-R030.1

    #[test]
    fn edge_d05_verified_close() {
        edge_case(
            "D05",
            DefectStatus::Verified,
            DefectEvent::Close,
            DefectStatus::Closed,
        );
    }

    // spec: MEDULLA-SDLC-R030.1

    #[test]
    fn edge_d06_new_reject() {
        edge_case(
            "D06",
            DefectStatus::New,
            DefectEvent::Reject,
            DefectStatus::Rejected,
        );
    }

    // spec: MEDULLA-SDLC-R030.1

    #[test]
    fn edge_d07_triaged_reject() {
        edge_case(
            "D07",
            DefectStatus::Triaged,
            DefectEvent::Reject,
            DefectStatus::Rejected,
        );
    }

    // spec: MEDULLA-SDLC-R030.1

    #[test]
    fn edge_d08_closed_reopen() {
        edge_case(
            "D08",
            DefectStatus::Closed,
            DefectEvent::Reopen,
            DefectStatus::New,
        );
    }

    // spec: MEDULLA-SDLC-R030.1

    #[test]
    fn edge_d09_rejected_reopen() {
        edge_case(
            "D09",
            DefectStatus::Rejected,
            DefectEvent::Reopen,
            DefectStatus::New,
        );
    }

    // spec: MEDULLA-SDLC-R030.1

    #[test]
    fn every_illegal_pair_is_refused() {
        let machine = &DEFECT_MACHINE;
        let pairs = illegal_pairs();
        assert!(!pairs.is_empty());
        for (state, event) in pairs {
            let refused = state.apply(event, ALL).unwrap_err();
            let expected = if machine.is_terminal(state) {
                TransitionRefusalReason::TerminalState
            } else {
                TransitionRefusalReason::IllegalTransition
            };
            assert_eq!(refused.reason, expected, "{state:?} {event:?}");
        }
        for &terminal in machine.terminals {
            for &event in machine.events {
                assert!(terminal.apply(event, ALL).is_err());
            }
        }
    }

    // spec: MEDULLA-SDLC-R030.1

    #[test]
    fn refusal_carries_machine_from_event() {
        let refused = DefectStatus::New
            .apply(DefectEvent::StartFix, ALL)
            .unwrap_err();
        assert_eq!(refused.machine, "DefectMachine");
        assert_eq!(refused.from, DefectStatus::New.stored());
        assert_eq!(refused.event, "StartFix");
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

    // spec: MEDULLA-SDLC-R030.1

    #[test]
    fn no_writer_sets_state_outside_apply() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../medulla-core/src/lifecycle/defect.rs"
        );
        let source = std::fs::read_to_string(path).expect("writer source");
        let writes = direct_state_writes(&source, "status", "DefectStatus");
        assert!(
            writes.is_empty(),
            "direct state writes in ../medulla-core/src/lifecycle/defect.rs: {writes:?}"
        );
    }

    // spec: MEDULLA-SDLC-R030.1

    #[test]
    fn every_transition_and_refusal_is_covered() {
        let machine = &DEFECT_MACHINE;
        assert_eq!(machine.edges.len(), 9);
        assert_eq!(machine.states.len(), 7);
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
                let legal = state.apply(event, ALL).is_ok();
                let has_edge = machine.edges_from(state).any(|e| e.event == event);
                assert_eq!(legal, has_edge, "{state:?} {event:?}");
            }
        }
    }
}
