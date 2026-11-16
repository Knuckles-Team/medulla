//! MEDULLA-INGEST-WORKER-R911.3 -- the ingest stage of a paper revision as one explicit
//! state machine (organization constitution Article XXXI).
//!
//! A paper revision moves through four stages in pipeline order. [`IngestStage::apply`] is
//! the one transition function: it is the only code that moves a stage, it returns the
//! typed [`TransitionRefused`] for every pair the edge table does not list, and the last
//! stage accepts no event.

use crate::ingest_throughput::IngestStage;
use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// An event of `IngestMachine`: the stage that just completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IngestEvent {
    /// The paper revision was fetched.
    Fetched,
    /// Text was extracted from the fetched revision.
    Extracted,
    /// The extracted text was embedded.
    Embedded,
}

impl MachineState for IngestStage {
    fn stored(self) -> &'static str {
        match self {
            Self::Fetch => "fetch",
            Self::Extract => "extract",
            Self::Embed => "embed",
            Self::Index => "index",
        }
    }
}

impl MachineEvent for IngestEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Fetched => "Fetched",
            Self::Extracted => "Extracted",
            Self::Embedded => "Embedded",
        }
    }
}

/// `IngestMachine`: the closed edge table. Every other pair of state and event is refused.
pub static INGEST_MACHINE: MachineDef<IngestStage, IngestEvent, &'static str> = MachineDef {
    name: "IngestMachine",
    states: &[
        IngestStage::Fetch,
        IngestStage::Extract,
        IngestStage::Embed,
        IngestStage::Index,
    ],
    initial: IngestStage::Fetch,
    terminals: &[IngestStage::Index],
    events: &[
        IngestEvent::Fetched,
        IngestEvent::Extracted,
        IngestEvent::Embedded,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "I01",
            from: IngestStage::Fetch,
            event: IngestEvent::Fetched,
            guard: None,
            to: IngestStage::Extract,
        },
        Edge {
            id: "I02",
            from: IngestStage::Extract,
            event: IngestEvent::Extracted,
            guard: None,
            to: IngestStage::Embed,
        },
        Edge {
            id: "I03",
            from: IngestStage::Embed,
            event: IngestEvent::Embedded,
            guard: None,
            to: IngestStage::Index,
        },
    ],
};

impl IngestStage {
    /// The one transition function of `IngestMachine`.
    pub fn apply(self, event: IngestEvent) -> Result<Self, TransitionRefused> {
        transition(&INGEST_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The stage a paper revision is in after `completed` stages finished, reached by
    /// applying the pipeline events from the initial stage.
    pub fn after_completed(completed: usize) -> Result<Self, TransitionRefused> {
        const PIPELINE: [IngestEvent; 3] = [
            IngestEvent::Fetched,
            IngestEvent::Extracted,
            IngestEvent::Embedded,
        ];
        let mut stage = INGEST_MACHINE.initial;
        for event in PIPELINE.iter().copied().cycle().take(completed) {
            stage = stage.apply(event)?;
        }
        Ok(stage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-INGEST-WORKER-R911.3
    #[test]
    fn every_legal_transition_applies() {
        for edge in INGEST_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(IngestStage::after_completed(0), Ok(IngestStage::Fetch));
        assert_eq!(IngestStage::after_completed(3), Ok(IngestStage::Index));
    }

    // spec: MEDULLA-INGEST-WORKER-R911.3
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &INGEST_MACHINE;
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
                assert_eq!(refused.machine, "IngestMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(
            illegal,
            machine.states.len() * machine.events.len() - machine.edges.len()
        );
        assert!(IngestStage::after_completed(4).is_err());
    }

    /// Lines that assign a `stage` field outside the transition function.
    // spec: MEDULLA-INGEST-WORKER-R911.3
    #[test]
    fn no_writer_sets_stage_outside_apply() {
        for source in [
            include_str!("ingest_throughput.rs"),
            include_str!("ingest_drain.rs"),
        ] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//") && l.contains(".stage = "))
                .collect();
            assert!(writes.is_empty(), "{writes:?}");
        }
        let own = include_str!("ingest_machine.rs");
        let production = own.split("#[cfg(test)]").next().unwrap();
        let writes: Vec<&str> = production
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("//") && l.starts_with("stage = "))
            .collect();
        assert_eq!(writes, vec!["stage = stage.apply(event)?;"]);
    }
}
