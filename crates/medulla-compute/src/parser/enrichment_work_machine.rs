//! MEDULLA-INGEST-WORKER-R911.1 -- the stage of an eligible enrichment unit as one explicit state machine (organization constitution
//! Article XXXI).
//!
//! A unit climbs the enrichment ladder one rung at a time: classical, then embedding, then
//! the language model. [`EnrichmentWorkStage::apply`] is the one transition function. It is the only
//! code that moves a stage, it returns the typed [`TransitionRefused`] for every pair the
//! edge table does not list, and the last rung accepts no event.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use super::enrichment_snapshot::EnrichmentWorkStage;

/// An event of `EnrichmentWorkMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EnrichmentWorkEvent {
    /// The unit moved past its current rung to the next one.
    Escalated,
}

impl MachineState for EnrichmentWorkStage {
    fn stored(self) -> &'static str {
        match self {
            Self::Classical => "classical",
            Self::Embedding => "embedding",
            Self::Llm => "llm",
        }
    }
}

impl MachineEvent for EnrichmentWorkEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Escalated => "Escalated",
        }
    }
}

/// `EnrichmentWorkMachine`: the closed edge table. Every other pair of state and event is refused.
pub static ENRICHMENT_WORK_MACHINE: MachineDef<
    EnrichmentWorkStage,
    EnrichmentWorkEvent,
    &'static str,
> = MachineDef {
    name: "EnrichmentWorkMachine",
    states: &[
        EnrichmentWorkStage::Classical,
        EnrichmentWorkStage::Embedding,
        EnrichmentWorkStage::Llm,
    ],
    initial: EnrichmentWorkStage::Classical,
    terminals: &[EnrichmentWorkStage::Llm],
    events: &[EnrichmentWorkEvent::Escalated],
    guards: &[],
    edges: &[
        Edge {
            id: "W01",
            from: EnrichmentWorkStage::Classical,
            event: EnrichmentWorkEvent::Escalated,
            guard: None,
            to: EnrichmentWorkStage::Embedding,
        },
        Edge {
            id: "W02",
            from: EnrichmentWorkStage::Embedding,
            event: EnrichmentWorkEvent::Escalated,
            guard: None,
            to: EnrichmentWorkStage::Llm,
        },
    ],
};

impl EnrichmentWorkStage {
    /// The one transition function of `EnrichmentWorkMachine`.
    pub fn apply(self, event: EnrichmentWorkEvent) -> Result<Self, TransitionRefused> {
        transition(&ENRICHMENT_WORK_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-INGEST-WORKER-R911.1
    #[test]
    fn every_legal_transition_applies() {
        for edge in ENRICHMENT_WORK_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R911.1
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &ENRICHMENT_WORK_MACHINE;
        let mut illegal = 0;
        for &state in machine.states {
            for &event in machine.events {
                if machine
                    .edges
                    .iter()
                    .any(|e| e.from == state && e.event == event)
                {
                    continue;
                }
                illegal += 1;
                let refused = state.apply(event).unwrap_err();
                let expected = if machine.is_terminal(state) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, expected);
                assert_eq!(refused.machine, "EnrichmentWorkMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(
            illegal,
            machine.states.len() * machine.events.len() - machine.edges.len()
        );
    }

    /// The stage of a record is written only by its constructors and by `apply`.
    // spec: MEDULLA-INGEST-WORKER-R911.1
    #[test]
    fn no_writer_sets_stage_outside_apply() {
        let source = include_str!("enrichment_snapshot.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let writes: Vec<&str> = production
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("//") && l.contains(".stage = "))
            .collect();
        assert!(writes.is_empty(), "{writes:?}");
    }
}
