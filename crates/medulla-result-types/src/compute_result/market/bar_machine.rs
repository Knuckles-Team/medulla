//! MEDULLA-FINANCE-PRIMITIVES-R905.1: the finality of one bar version as an explicit state
//! machine (`BarMachine`).
//!
//! `Provisional` is the initial state: the period is still open or its data incomplete. The
//! closed event set is `Settle`, which enters the terminal `Final`; a final bar is never
//! reopened (a correction is a new version with a higher revision, never a status change).
//! [`BarStatus::apply`] is the only function that produces a new status, and a refused event
//! is the typed `TRANSITION_REFUSED` naming the machine, the current state and the event. The
//! ontology description is `BarMachine` in `state-machines-v1.ttl`.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use super::BarStatus;

/// The closed event set of `BarMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BarEvent {
    /// The bar's period closed and its values are settled.
    Settle,
}

impl MachineState for BarStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Provisional => "provisional",
            Self::Final => "final",
        }
    }
}

impl MachineEvent for BarEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Settle => "Settle",
        }
    }
}

/// `BarMachine`: the closed edge table. Every other pair is refused.
pub static BAR_MACHINE: MachineDef<BarStatus, BarEvent, &'static str> = MachineDef {
    name: "BarMachine",
    states: &[BarStatus::Provisional, BarStatus::Final],
    initial: BarStatus::Provisional,
    terminals: &[BarStatus::Final],
    events: &[BarEvent::Settle],
    guards: &[],
    edges: &[Edge {
        id: "B01",
        from: BarStatus::Provisional,
        event: BarEvent::Settle,
        guard: None,
        to: BarStatus::Final,
    }],
};

impl BarStatus {
    /// The state a new bar version takes.
    pub const INITIAL: BarStatus = BarStatus::Provisional;

    /// The one transition function of `BarMachine`.
    pub fn apply(self, event: BarEvent) -> Result<Self, TransitionRefused> {
        transition(&BAR_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The status of a bar built from a period that is `settled` or still open: a new
    /// record starts `Provisional` and a settled one takes the `Settle` edge.
    pub fn of_settled(settled: bool) -> Result<Self, TransitionRefused> {
        if settled {
            Self::INITIAL.apply(BarEvent::Settle)
        } else {
            Ok(Self::INITIAL)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-FINANCE-PRIMITIVES-R905.1
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(BAR_MACHINE.initial, BarStatus::Provisional);
        assert_eq!(
            BarStatus::Provisional.apply(BarEvent::Settle),
            Ok(BarStatus::Final)
        );
        assert_eq!(BarStatus::of_settled(true), Ok(BarStatus::Final));
        assert_eq!(BarStatus::of_settled(false), Ok(BarStatus::Provisional));
        assert_eq!(BAR_MACHINE.edges.len(), 1, "one legal edge is declared");
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R905.1
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        // The only pair the edge table does not list: a final bar never takes an event.
        let refused = BarStatus::Final.apply(BarEvent::Settle).unwrap_err();
        assert_eq!(refused.machine, "BarMachine");
        assert_eq!(refused.from, "final");
        assert_eq!(refused.event, "Settle");
        assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
        for state in BAR_MACHINE.states {
            for event in BAR_MACHINE.events {
                let listed = BAR_MACHINE
                    .edges
                    .iter()
                    .any(|edge| edge.from == *state && edge.event == *event);
                assert_eq!(state.apply(*event).is_ok(), listed, "{state:?} {event:?}");
            }
        }
    }

    /// No production file assigns `.status = BarStatus::...` after construction: the status
    /// changes only inside `BarStatus::apply`.
    // spec: MEDULLA-FINANCE-PRIMITIVES-R905.1
    #[test]
    fn no_writer_sets_status_outside_apply() {
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
                        if !line.starts_with("//") && line.contains(".status = BarStatus::") {
                            hits.push(format!("{}:{}", path.display(), number + 1));
                        }
                    }
                }
            }
        }
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut hits = Vec::new();
        walk(&crates, &mut hits);
        assert!(hits.is_empty(), "direct BarStatus writes: {hits:?}");
    }
}
