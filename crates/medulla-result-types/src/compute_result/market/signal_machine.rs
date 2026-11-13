//! MEDULLA-FINANCE-PRIMITIVES-R905.3: the data status of one signal projection as an explicit
//! state machine (`SignalDataMachine`).
//!
//! `Unavailable` is the initial state: no final bar has been consumed. A consumed bar either
//! leaves the kernel still warming (`Warm`) or produces a value (`Confirm`); a valid signal
//! whose last final bar is older than the caller's bound takes `Expire` to `Stale`, and a new
//! confirmed bar brings a stale signal back to `Valid`. No state is terminal.
//! [`DataStatus::apply`] is the only function that produces a new status, and a refused event
//! is the typed `TRANSITION_REFUSED` naming the machine, the current state and the event. The
//! status lives in `SignalState.data_status`; the ontology description is `SignalDataMachine` in
//! `state-machines-v1.ttl`.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use super::DataStatus;

/// The closed event set of `SignalDataMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SignalEvent {
    /// A final bar was consumed and the kernel is still short of its warm-up.
    Warm,
    /// A final bar was consumed and the kernel produced a value.
    Confirm,
    /// The last final bar is older than the staleness bound.
    Expire,
}

impl MachineState for DataStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Warming => "warming",
            Self::Valid => "valid",
            Self::Stale => "stale",
            Self::Unavailable => "unavailable",
        }
    }
}

impl MachineEvent for SignalEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Warm => "Warm",
            Self::Confirm => "Confirm",
            Self::Expire => "Expire",
        }
    }
}

/// `SignalDataMachine`: the closed edge table. Every other pair is refused.
pub static SIGNAL_MACHINE: MachineDef<DataStatus, SignalEvent, &'static str> = MachineDef {
    name: "SignalDataMachine",
    states: &[
        DataStatus::Unavailable,
        DataStatus::Warming,
        DataStatus::Valid,
        DataStatus::Stale,
    ],
    initial: DataStatus::Unavailable,
    terminals: &[],
    events: &[SignalEvent::Warm, SignalEvent::Confirm, SignalEvent::Expire],
    guards: &[],
    edges: &[
        Edge {
            id: "S01",
            from: DataStatus::Unavailable,
            event: SignalEvent::Warm,
            guard: None,
            to: DataStatus::Warming,
        },
        Edge {
            id: "S02",
            from: DataStatus::Warming,
            event: SignalEvent::Warm,
            guard: None,
            to: DataStatus::Warming,
        },
        Edge {
            id: "S03",
            from: DataStatus::Unavailable,
            event: SignalEvent::Confirm,
            guard: None,
            to: DataStatus::Valid,
        },
        Edge {
            id: "S04",
            from: DataStatus::Warming,
            event: SignalEvent::Confirm,
            guard: None,
            to: DataStatus::Valid,
        },
        Edge {
            id: "S05",
            from: DataStatus::Valid,
            event: SignalEvent::Confirm,
            guard: None,
            to: DataStatus::Valid,
        },
        Edge {
            id: "S06",
            from: DataStatus::Stale,
            event: SignalEvent::Confirm,
            guard: None,
            to: DataStatus::Valid,
        },
        Edge {
            id: "S07",
            from: DataStatus::Valid,
            event: SignalEvent::Expire,
            guard: None,
            to: DataStatus::Stale,
        },
    ],
};

impl DataStatus {
    /// The status of a signal before any bar.
    pub const INITIAL: DataStatus = DataStatus::Unavailable;

    /// The one transition function of `SignalDataMachine`.
    pub fn apply(self, event: SignalEvent) -> Result<Self, TransitionRefused> {
        transition(&SIGNAL_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-FINANCE-PRIMITIVES-R905.3
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(SIGNAL_MACHINE.initial, DataStatus::INITIAL);
        for edge in SIGNAL_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(
            SIGNAL_MACHINE.edges.len(),
            7,
            "seven legal edges are declared"
        );
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R905.3
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let refused = DataStatus::Stale.apply(SignalEvent::Expire).unwrap_err();
        assert_eq!(refused.machine, "SignalDataMachine");
        assert_eq!(refused.from, "stale");
        assert_eq!(refused.event, "Expire");
        assert_eq!(refused.reason, TransitionRefusalReason::IllegalTransition);
        for state in SIGNAL_MACHINE.states {
            for event in SIGNAL_MACHINE.events {
                let listed = SIGNAL_MACHINE
                    .edges
                    .iter()
                    .any(|edge| edge.from == *state && edge.event == *event);
                assert_eq!(state.apply(*event).is_ok(), listed, "{state:?} {event:?}");
            }
        }
    }

    /// No production file assigns `.data_status = DataStatus::...` after construction: the
    /// status changes only through `DataStatus::apply`.
    // spec: MEDULLA-FINANCE-PRIMITIVES-R905.3
    #[test]
    fn no_writer_sets_state_outside_apply() {
        fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if path.is_dir() {
                    if name != "target" {
                        walk(&path, hits);
                    }
                } else if name.ends_with(".rs") {
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    for (number, line) in text.lines().enumerate() {
                        let line = line.trim();
                        if !line.starts_with("//") && line.contains(".data_status = DataStatus::") {
                            hits.push(format!("{}:{}", path.display(), number + 1));
                        }
                    }
                }
            }
        }
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut hits = Vec::new();
        walk(&crates, &mut hits);
        assert!(hits.is_empty(), "direct DataStatus writes: {hits:?}");
    }
}
