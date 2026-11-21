//! MEDULLA-DATAPLAT-VOCAB-R009.5: the admission state of one induced ontology proposal as one explicit state machine
//! (organization constitution Article XXXI).
//!
//! `Draft` is the initial state; the closed event set is `Admit`. [`ProposalState::apply`] is the only function that
//! produces a new state, and a refused pair is the typed `TRANSITION_REFUSED` naming the machine,
//! the current state and the event. The ontology description is `OntologyProposalMachine` in `state-machines-v1.ttl`.

use serde::{Deserialize, Serialize};

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// Admission state of an induced proposal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ProposalState {
    #[default]
    /// Induced and not yet admitted; the initial state.
    Draft,
    /// Passed the admission checks; terminal.
    Admitted,
}

/// The closed event set of `OntologyProposalMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProposalEvent {
    /// The proposal passes property coverage and class redundancy.
    Admit,
}

impl MachineState for ProposalState {
    fn stored(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Admitted => "admitted",
        }
    }
}

impl MachineEvent for ProposalEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Admit => "Admit",
        }
    }
}

/// `OntologyProposalMachine`: the closed edge table. Every other pair is refused.
pub static ONTOLOGY_PROPOSAL_MACHINE: MachineDef<ProposalState, ProposalEvent, &'static str> =
    MachineDef {
        name: "OntologyProposalMachine",
        states: &[ProposalState::Draft, ProposalState::Admitted],
        initial: ProposalState::Draft,
        terminals: &[ProposalState::Admitted],
        events: &[ProposalEvent::Admit],
        guards: &[],
        edges: &[Edge {
            id: "OP01",
            from: ProposalState::Draft,
            event: ProposalEvent::Admit,
            guard: None,
            to: ProposalState::Admitted,
        }],
    };

impl ProposalState {
    /// The one transition function of `OntologyProposalMachine`: the next state, or the typed refusal.
    pub fn apply(self, event: ProposalEvent) -> Result<Self, TransitionRefused> {
        transition(&ONTOLOGY_PROPOSAL_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    const MACHINES: &str = include_str!("../../medulla-core/ontology/state-machines-v1.ttl");

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.5
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(ONTOLOGY_PROPOSAL_MACHINE.edges.len(), 1);
        for edge in ONTOLOGY_PROPOSAL_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(
            ProposalState::Draft.apply(ProposalEvent::Admit),
            Ok(ProposalState::Admitted)
        );
    }

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.5
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &ONTOLOGY_PROPOSAL_MACHINE;
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
                assert_eq!(refused.machine, "OntologyProposalMachine");
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

    // spec: MEDULLA-DATAPLAT-VOCAB-R009.5
    #[test]
    fn the_ontology_describes_the_same_machine() {
        assert!(MACHINES.contains(":OntologyProposalMachine a :StateMachine"));
        assert!(MACHINES.contains(":OntologyProposalMachine_Draft a"));
        assert!(MACHINES.contains(":storedForm \"draft\""));
        assert!(MACHINES.contains(":OntologyProposalMachine_Admitted a"));
        assert!(MACHINES.contains(":storedForm \"admitted\""));
    }

    /// Lines that assign the `state` field outside the transition function.
    // spec: MEDULLA-DATAPLAT-VOCAB-R009.5
    #[test]
    fn no_writer_sets_state_outside_apply() {
        let sources = [
            include_str!("../../medulla-ingestion-types/src/ontology_proposal.rs"),
            include_str!("../../medulla-ingestion-types/src/ingest_rules.rs"),
        ];
        for source in sources {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//") && l.contains(".state = "))
                .collect();
            assert!(writes.is_empty(), "{writes:?}");
        }
    }
}
