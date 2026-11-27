//! MEDULLA-DATAPLAT-VOCAB-R009.4: the review lifecycle of one glossary term as one explicit state machine
//! (organization constitution Article XXXI).
//!
//! `Draft` is the initial state; the closed event set is `Submit`, `Approve`, `Deprecate`. [`GlossaryTermStatus::apply`] is the only function that
//! produces a new state, and a refused pair is the typed `TRANSITION_REFUSED` naming the machine,
//! the current state and the event. The ontology description is `GlossaryTermMachine` in `state-machines-v1.ttl`.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// Where a glossary term is in its review lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GlossaryTermStatus {
    /// Being written; the initial state.
    Draft,
    /// Submitted for review.
    InReview,
    /// Approved for use.
    Approved,
    /// Replaced or withdrawn; terminal.
    Deprecated,
}

/// The closed event set of `GlossaryTermMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GlossaryTermEvent {
    /// The draft is submitted for review.
    Submit,
    /// A reviewer approves the term.
    Approve,
    /// The approved term is retired.
    Deprecate,
}

impl MachineState for GlossaryTermStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::InReview => "in_review",
            Self::Approved => "approved",
            Self::Deprecated => "deprecated",
        }
    }
}

impl MachineEvent for GlossaryTermEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Submit => "Submit",
            Self::Approve => "Approve",
            Self::Deprecate => "Deprecate",
        }
    }
}

/// `GlossaryTermMachine`: the closed edge table. Every other pair is refused.
pub static GLOSSARY_TERM_MACHINE: MachineDef<GlossaryTermStatus, GlossaryTermEvent, &'static str> =
    MachineDef {
        name: "GlossaryTermMachine",
        states: &[
            GlossaryTermStatus::Draft,
            GlossaryTermStatus::InReview,
            GlossaryTermStatus::Approved,
            GlossaryTermStatus::Deprecated,
        ],
        initial: GlossaryTermStatus::Draft,
        terminals: &[GlossaryTermStatus::Deprecated],
        events: &[
            GlossaryTermEvent::Submit,
            GlossaryTermEvent::Approve,
            GlossaryTermEvent::Deprecate,
        ],
        guards: &[],
        edges: &[
            Edge {
                id: "GL01",
                from: GlossaryTermStatus::Draft,
                event: GlossaryTermEvent::Submit,
                guard: None,
                to: GlossaryTermStatus::InReview,
            },
            Edge {
                id: "GL02",
                from: GlossaryTermStatus::InReview,
                event: GlossaryTermEvent::Approve,
                guard: None,
                to: GlossaryTermStatus::Approved,
            },
            Edge {
                id: "GL03",
                from: GlossaryTermStatus::Approved,
                event: GlossaryTermEvent::Deprecate,
                guard: None,
                to: GlossaryTermStatus::Deprecated,
            },
        ],
    };

impl GlossaryTermStatus {
    /// The one transition function of `GlossaryTermMachine`: the next state, or the typed refusal.
    pub fn apply(self, event: GlossaryTermEvent) -> Result<Self, TransitionRefused> {
        transition(&GLOSSARY_TERM_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    const MACHINES: &str = include_str!("../../medulla-core/ontology/state-machines-v1.ttl");

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.4
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(GLOSSARY_TERM_MACHINE.edges.len(), 3);
        for edge in GLOSSARY_TERM_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(
            GlossaryTermStatus::Draft.apply(GlossaryTermEvent::Submit),
            Ok(GlossaryTermStatus::InReview)
        );
        assert_eq!(
            GlossaryTermStatus::InReview.apply(GlossaryTermEvent::Approve),
            Ok(GlossaryTermStatus::Approved)
        );
        assert_eq!(
            GlossaryTermStatus::Approved.apply(GlossaryTermEvent::Deprecate),
            Ok(GlossaryTermStatus::Deprecated)
        );
    }

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.4
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &GLOSSARY_TERM_MACHINE;
        for &state in machine.states {
            for &event in machine.events {
                if machine
                    .edges
                    .iter()
                    .any(|e| e.from == state && e.event == event)
                {
                    continue;
                }
                let refused = state.apply(event).unwrap_err();
                assert_eq!(refused.machine, "GlossaryTermMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                let expected = if machine.is_terminal(state) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, expected);
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
    }

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.4
    #[test]
    fn the_ontology_describes_the_same_machine() {
        assert!(MACHINES.contains(":GlossaryTermMachine a :StateMachine"));
        assert!(MACHINES.contains(":GlossaryTermMachine_Draft a"));
        assert!(MACHINES.contains(":storedForm \"draft\""));
        assert!(MACHINES.contains(":GlossaryTermMachine_InReview a"));
        assert!(MACHINES.contains(":storedForm \"in_review\""));
        assert!(MACHINES.contains(":GlossaryTermMachine_Approved a"));
        assert!(MACHINES.contains(":storedForm \"approved\""));
        assert!(MACHINES.contains(":GlossaryTermMachine_Deprecated a"));
        assert!(MACHINES.contains(":storedForm \"deprecated\""));
    }

    /// Lines that assign the `status` field outside the transition function.
    // spec: MEDULLA-DATAPLAT-VOCAB-R009.4
    #[test]
    fn no_writer_sets_status_outside_apply() {
        let sources = [include_str!(
            "../../medulla-catalog-source-types/src/glossary.rs"
        )];
        for source in sources {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//") && l.contains(".status = "))
                .collect();
            assert!(writes.is_empty(), "{writes:?}");
        }
    }
}
