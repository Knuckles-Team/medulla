//! MEDULLA-ENGINE-PARITY-R021.2: how far the engine serves one SQL form, as an explicit state
//! machine.
//!
//! A form starts `Refused` (the compatibility matrix is closed: an absent form is refused).
//! `ServePartially` and `ServeFully` move it up, `Narrow` and `Withdraw` move it down. No state
//! is terminal: a served form can be narrowed or withdrawn when a defect is found.
//! [`SqlFormState::apply`] is the only function that produces a new state, and a refused event is
//! the typed `TRANSITION_REFUSED` naming the machine, the current state and the event. The
//! ontology description is `SqlFormMachine` in `state-machines-v1.ttl`.

use crate::pg_operational_surface::SqlFormState;
use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The closed event set of `SqlFormMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SqlFormEvent {
    /// A refused form is served with a documented gap.
    ServePartially,
    /// A refused or partial form is served in full.
    ServeFully,
    /// A fully served form is narrowed to a partial one.
    Narrow,
    /// A served form is withdrawn and refused again.
    Withdraw,
}

impl MachineState for SqlFormState {
    fn stored(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Partial => "partial",
            Self::Refused => "refused",
        }
    }
}

impl MachineEvent for SqlFormEvent {
    fn name(self) -> &'static str {
        match self {
            Self::ServePartially => "ServePartially",
            Self::ServeFully => "ServeFully",
            Self::Narrow => "Narrow",
            Self::Withdraw => "Withdraw",
        }
    }
}

/// `SqlFormMachine`: the closed edge table. Every other pair is refused.
pub static SQL_FORM_MACHINE: MachineDef<SqlFormState, SqlFormEvent, &'static str> = MachineDef {
    name: "SqlFormMachine",
    states: &[
        SqlFormState::Supported,
        SqlFormState::Partial,
        SqlFormState::Refused,
    ],
    initial: SqlFormState::Refused,
    terminals: &[],
    events: &[
        SqlFormEvent::ServePartially,
        SqlFormEvent::ServeFully,
        SqlFormEvent::Narrow,
        SqlFormEvent::Withdraw,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "S01",
            from: SqlFormState::Refused,
            event: SqlFormEvent::ServePartially,
            guard: None,
            to: SqlFormState::Partial,
        },
        Edge {
            id: "S02",
            from: SqlFormState::Refused,
            event: SqlFormEvent::ServeFully,
            guard: None,
            to: SqlFormState::Supported,
        },
        Edge {
            id: "S03",
            from: SqlFormState::Partial,
            event: SqlFormEvent::ServeFully,
            guard: None,
            to: SqlFormState::Supported,
        },
        Edge {
            id: "S04",
            from: SqlFormState::Partial,
            event: SqlFormEvent::Withdraw,
            guard: None,
            to: SqlFormState::Refused,
        },
        Edge {
            id: "S05",
            from: SqlFormState::Supported,
            event: SqlFormEvent::Narrow,
            guard: None,
            to: SqlFormState::Partial,
        },
        Edge {
            id: "S06",
            from: SqlFormState::Supported,
            event: SqlFormEvent::Withdraw,
            guard: None,
            to: SqlFormState::Refused,
        },
    ],
};

impl SqlFormState {
    /// The one transition function of `SqlFormMachine`.
    pub fn apply(self, event: SqlFormEvent) -> Result<Self, TransitionRefused> {
        transition(&SQL_FORM_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The state a new form record takes before it is served.
    pub fn initial() -> Self {
        SQL_FORM_MACHINE.initial
    }

    /// The state a new form reaches when the engine serves it as `target`: the initial state
    /// with the one event that leads there. The matrix is closed, so a target that cannot be
    /// reached (never the case for the three states) stays `Refused`.
    pub fn serve_as(target: Self) -> Self {
        let event = match target {
            Self::Refused => return Self::initial(),
            Self::Partial => SqlFormEvent::ServePartially,
            Self::Supported => SqlFormEvent::ServeFully,
        };
        Self::initial().apply(event).unwrap_or(Self::initial())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    const EVENTS: [SqlFormEvent; 4] = [
        SqlFormEvent::ServePartially,
        SqlFormEvent::ServeFully,
        SqlFormEvent::Narrow,
        SqlFormEvent::Withdraw,
    ];

    // spec: MEDULLA-ENGINE-PARITY-R021.2
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(SQL_FORM_MACHINE.edges.len(), 6);
        for edge in SQL_FORM_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(SqlFormState::initial(), SqlFormState::Refused);
        for target in [
            SqlFormState::Refused,
            SqlFormState::Partial,
            SqlFormState::Supported,
        ] {
            assert_eq!(SqlFormState::serve_as(target), target);
        }
    }

    // spec: MEDULLA-ENGINE-PARITY-R021.2
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let mut refused_pairs = 0;
        for &state in SQL_FORM_MACHINE.states {
            for event in EVENTS {
                let legal = SQL_FORM_MACHINE
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
                assert_eq!(refused.machine, "SqlFormMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.reason, TransitionRefusalReason::IllegalTransition);
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(refused_pairs, 6);
    }

    /// Lines that assign the state field directly, outside a file's `#[cfg(test)] mod tests`.
    fn direct_writes(source: &str) -> Vec<String> {
        let needles = [
            "state: SqlFormState::Supported",
            "state: SqlFormState::Partial",
            "state: SqlFormState::Refused",
            ".state = SqlFormState::",
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

    // spec: MEDULLA-ENGINE-PARITY-R021.2
    #[test]
    fn no_writer_sets_state_outside_apply() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/pg_operational_surface.rs"
        ))
        .expect("writer source");
        let writes = direct_writes(&source);
        assert!(writes.is_empty(), "direct state writes: {writes:?}");
    }
}
