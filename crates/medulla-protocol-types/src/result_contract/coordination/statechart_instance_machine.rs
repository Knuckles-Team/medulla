//! MEDULLA-ENGINE-PARITY-R021.3: the status marker of one statechart instance on the wire as an explicit state machine.
//!
//! `Active` is the initial state. The marker follows the configuration: a firing transition
//! either keeps it (`Remain`, `RemainFinal`) or moves it (`ReachFinal`, `LeaveFinal`) as the
//! configuration gains or loses a final state, and a parallel chart can leave a final state
//! while another region fires, so no state is terminal. [`StatechartInstanceStatus::apply`] is the only
//! function that produces a new state, and a refused event is the typed `TRANSITION_REFUSED`
//! naming the machine, the current state and the event. The ontology description is
//! `StatechartInstanceMachine` in `state-machines-v1.ttl`.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use super::statechart::StatechartInstanceStatus;

/// The closed event set of `StatechartInstanceMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StatechartInstanceEvent {
    /// A firing transition leaves the instance non-final.
    Remain,
    /// A firing transition enters a final state.
    ReachFinal,
    /// A firing transition leaves every final state.
    LeaveFinal,
    /// A firing transition keeps the instance final.
    RemainFinal,
}

impl MachineState for StatechartInstanceStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Final => "final",
        }
    }
}

impl MachineEvent for StatechartInstanceEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Remain => "Remain",
            Self::ReachFinal => "ReachFinal",
            Self::LeaveFinal => "LeaveFinal",
            Self::RemainFinal => "RemainFinal",
        }
    }
}

/// `StatechartInstanceMachine`: the closed edge table. Every other pair is refused.
pub static STATECHART_INSTANCE_MACHINE: MachineDef<
    StatechartInstanceStatus,
    StatechartInstanceEvent,
    &'static str,
> = MachineDef {
    name: "StatechartInstanceMachine",
    states: &[
        StatechartInstanceStatus::Active,
        StatechartInstanceStatus::Final,
    ],
    initial: StatechartInstanceStatus::Active,
    terminals: &[],
    events: &[
        StatechartInstanceEvent::Remain,
        StatechartInstanceEvent::ReachFinal,
        StatechartInstanceEvent::LeaveFinal,
        StatechartInstanceEvent::RemainFinal,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "I01",
            from: StatechartInstanceStatus::Active,
            event: StatechartInstanceEvent::Remain,
            guard: None,
            to: StatechartInstanceStatus::Active,
        },
        Edge {
            id: "I02",
            from: StatechartInstanceStatus::Active,
            event: StatechartInstanceEvent::ReachFinal,
            guard: None,
            to: StatechartInstanceStatus::Final,
        },
        Edge {
            id: "I03",
            from: StatechartInstanceStatus::Final,
            event: StatechartInstanceEvent::LeaveFinal,
            guard: None,
            to: StatechartInstanceStatus::Active,
        },
        Edge {
            id: "I04",
            from: StatechartInstanceStatus::Final,
            event: StatechartInstanceEvent::RemainFinal,
            guard: None,
            to: StatechartInstanceStatus::Final,
        },
    ],
};

impl StatechartInstanceStatus {
    /// The one transition function of `StatechartInstanceMachine`.
    pub fn apply(self, event: StatechartInstanceEvent) -> Result<Self, TransitionRefused> {
        transition(&STATECHART_INSTANCE_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The state a new record takes before its first configuration is applied.
    pub fn initial() -> Self {
        STATECHART_INSTANCE_MACHINE.initial
    }

    /// Move to the state the configuration implies: the event is picked from the current state
    /// and whether the configuration holds a final state, then applied.
    pub fn settle(self, configuration_is_final: bool) -> Result<Self, TransitionRefused> {
        let event = match (self, configuration_is_final) {
            (Self::Active, false) => StatechartInstanceEvent::Remain,
            (Self::Active, true) => StatechartInstanceEvent::ReachFinal,
            (Self::Final, false) => StatechartInstanceEvent::LeaveFinal,
            (Self::Final, true) => StatechartInstanceEvent::RemainFinal,
        };
        self.apply(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    const EVENTS: [StatechartInstanceEvent; 4] = [
        StatechartInstanceEvent::Remain,
        StatechartInstanceEvent::ReachFinal,
        StatechartInstanceEvent::LeaveFinal,
        StatechartInstanceEvent::RemainFinal,
    ];

    // spec: MEDULLA-ENGINE-PARITY-R021.3
    #[test]
    fn every_legal_transition_applies() {
        let machine = &STATECHART_INSTANCE_MACHINE;
        assert_eq!(machine.edges.len(), 4);
        for edge in machine.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(
            StatechartInstanceStatus::initial(),
            StatechartInstanceStatus::Active
        );
        for (from, is_final, to) in [
            (
                StatechartInstanceStatus::Active,
                false,
                StatechartInstanceStatus::Active,
            ),
            (
                StatechartInstanceStatus::Active,
                true,
                StatechartInstanceStatus::Final,
            ),
            (
                StatechartInstanceStatus::Final,
                false,
                StatechartInstanceStatus::Active,
            ),
            (
                StatechartInstanceStatus::Final,
                true,
                StatechartInstanceStatus::Final,
            ),
        ] {
            assert_eq!(from.settle(is_final), Ok(to));
        }
    }

    // spec: MEDULLA-ENGINE-PARITY-R021.3
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &STATECHART_INSTANCE_MACHINE;
        let mut refused_pairs = 0;
        for &state in machine.states {
            for event in EVENTS {
                let legal = machine
                    .edges
                    .iter()
                    .any(|edge| edge.from == state && edge.event == event);
                let result = state.apply(event);
                if legal {
                    assert!(result.is_ok(), "{state:?} {event:?}");
                    continue;
                }
                refused_pairs += 1;
                let refused = result.unwrap_err();
                assert_eq!(refused.machine, "StatechartInstanceMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.reason, TransitionRefusalReason::IllegalTransition);
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(refused_pairs, 4);
    }

    /// Lines that assign the state field directly (`<field>: <Type>::` in a literal or
    /// `.<field> = <Type>::`), outside a file's `#[cfg(test)] mod tests`.
    fn direct_writes(source: &str) -> Vec<String> {
        let needles = [
            "status: StatechartInstanceStatus::",
            ".status = StatechartInstanceStatus::",
        ];
        let mut out = Vec::new();
        let mut previous = "";
        for line in source.lines().map(str::trim) {
            if previous == "#[cfg(test)]" && line.starts_with("mod tests") {
                break;
            }
            previous = line;
            if !line.starts_with("//") && needles.iter().any(|n| line.contains(n)) {
                out.push(line.to_string());
            }
        }
        out
    }

    // spec: MEDULLA-ENGINE-PARITY-R021.3
    #[test]
    fn no_writer_sets_status_outside_apply() {
        let root = env!("CARGO_MANIFEST_DIR");
        for relative in [
            "src/result_contract/coordination/statechart.rs",
            "src/result_contract/coordination/statechart_instance_machine.rs",
        ] {
            let source =
                std::fs::read_to_string(format!("{root}/{relative}")).expect("writer source");
            let writes = direct_writes(&source);
            assert!(
                writes.is_empty(),
                "direct state writes in {relative}: {writes:?}"
            );
        }
    }
}
