//! The source-reconciliation machine `SemanticSourceReconciliationMachine`
//! (MEDULLA-DURABLE-KERNEL-R874.17, organization constitution Article XXXI).
//!
//! [`SemanticSourceReconciliationPhase`] is the closed state set,
//! [`SemanticSourceReconciliationEvent`] the closed event set,
//! [`SEMANTIC_SOURCE_RECONCILIATION_MACHINE`] the edge table and
//! [`SemanticSourceReconciliationPhase::apply`] the one transition function. A checkpoint
//! is replaced, never mutated: each writer builds a successor and the CAS write checks
//! its phase against the predecessor's through this machine. The machine has no terminal
//! state, because completing the reconciliation deletes the record.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use super::reconciliation::SemanticSourceReconciliationPhase;

/// The closed event set of the source-reconciliation machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticSourceReconciliationEvent {
    /// A source page was admitted and the scan cursor advanced.
    AdvanceScan,
    /// The complete source snapshot was admitted.
    CompleteScan,
    /// A page of deletion tombstones was admitted.
    PageTombstones,
}

impl MachineEvent for SemanticSourceReconciliationEvent {
    fn name(self) -> &'static str {
        match self {
            Self::AdvanceScan => "AdvanceScan",
            Self::CompleteScan => "CompleteScan",
            Self::PageTombstones => "PageTombstones",
        }
    }
}

impl MachineState for SemanticSourceReconciliationPhase {
    fn stored(self) -> &'static str {
        match self {
            Self::Scanning => "scanning",
            Self::FinalizingTombstones => "finalizing_tombstones",
        }
    }
}

/// The source-reconciliation machine has no guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticSourceReconciliationGuard {}

/// The source-reconciliation machine: `Scanning` start; no terminal state.
pub const SEMANTIC_SOURCE_RECONCILIATION_MACHINE: MachineDef<
    SemanticSourceReconciliationPhase,
    SemanticSourceReconciliationEvent,
    SemanticSourceReconciliationGuard,
> = MachineDef {
    name: "SemanticSourceReconciliationMachine",
    states: &[
        SemanticSourceReconciliationPhase::Scanning,
        SemanticSourceReconciliationPhase::FinalizingTombstones,
    ],
    initial: SemanticSourceReconciliationPhase::Scanning,
    terminals: &[],
    events: &[
        SemanticSourceReconciliationEvent::AdvanceScan,
        SemanticSourceReconciliationEvent::CompleteScan,
        SemanticSourceReconciliationEvent::PageTombstones,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "R01",
            from: SemanticSourceReconciliationPhase::Scanning,
            event: SemanticSourceReconciliationEvent::AdvanceScan,
            guard: None,
            to: SemanticSourceReconciliationPhase::Scanning,
        },
        Edge {
            id: "R02",
            from: SemanticSourceReconciliationPhase::Scanning,
            event: SemanticSourceReconciliationEvent::CompleteScan,
            guard: None,
            to: SemanticSourceReconciliationPhase::FinalizingTombstones,
        },
        Edge {
            id: "R03",
            from: SemanticSourceReconciliationPhase::FinalizingTombstones,
            event: SemanticSourceReconciliationEvent::PageTombstones,
            guard: None,
            to: SemanticSourceReconciliationPhase::FinalizingTombstones,
        },
    ],
};

impl SemanticSourceReconciliationPhase {
    /// The one transition function: the next phase, or the typed refusal.
    pub fn apply(self, event: SemanticSourceReconciliationEvent) -> Result<Self, TransitionRefused> {
        transition(
            &SEMANTIC_SOURCE_RECONCILIATION_MACHINE,
            self,
            event,
            |guard| match guard {},
        )
        .map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-DURABLE-KERNEL-R874.17
    #[test]
    fn every_legal_transition_applies() {
        for edge in SEMANTIC_SOURCE_RECONCILIATION_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R874.17
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &SEMANTIC_SOURCE_RECONCILIATION_MACHINE;
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
                assert_eq!(refused.reason, TransitionRefusalReason::IllegalTransition);
                assert_eq!(refused.machine, "SemanticSourceReconciliationMachine");
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

    // spec: MEDULLA-DURABLE-KERNEL-R874.17
    #[test]
    fn no_writer_sets_phase_outside_apply() {
        // The checkpoint is replaced, never mutated: no production file assigns `.phase`,
        // and the one CAS writer checks the pair through the machine.
        for (name, source) in [
            ("reconciliation.rs", include_str!("reconciliation.rs")),
            (
                "source_scan.rs",
                include_str!("../semantic_index_service/source_scan.rs"),
            ),
            (
                "source_turn.rs",
                include_str!("../semantic_index_service/source_turn.rs"),
            ),
        ] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|line| !line.starts_with("//") && line.contains(".phase = "))
                .collect();
            assert!(writes.is_empty(), "{name}: {writes:?}");
        }
        let writer = include_str!("reconciliation.rs");
        assert!(writer.contains(".phase.apply(event)"));
    }
}
