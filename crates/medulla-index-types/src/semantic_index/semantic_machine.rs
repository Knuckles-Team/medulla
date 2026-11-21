//! The semantic-stage machine `SemanticMachine` (MEDULLA-DURABLE-KERNEL-R874.18,
//! organization constitution Article XXXI).
//!
//! [`SemanticStage`] is the closed state set, [`SemanticEvent`] the closed event set,
//! [`SEMANTIC_MACHINE`] the edge table and [`SemanticStage::apply`] the one transition
//! function. One stage intent is immutable; the machine orders the intents of one
//! generation, each naming its predecessor. `S6` (`ReconcileAndActivate`) is terminal.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use super::stage::SemanticStage;

/// The closed event set of the semantic-stage machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticEvent {
    /// S1 completed; the graph projection follows.
    ProjectGraph,
    /// S2 completed; the lexical index follows.
    IndexLexical,
    /// S3 completed; the vector stage follows.
    EmbedVectors,
    /// S4 completed; the ANN index follows.
    BuildAnn,
    /// S5 completed; the activation stage follows.
    ReconcileAndActivate,
}

impl SemanticEvent {
    /// The event that advances a completed `stage` to its successor, or `None` for the
    /// terminal stage.
    pub fn advancing(stage: SemanticStage) -> Option<Self> {
        match stage {
            SemanticStage::SourceCommit => Some(Self::ProjectGraph),
            SemanticStage::GraphProjection => Some(Self::IndexLexical),
            SemanticStage::LexicalIndex => Some(Self::EmbedVectors),
            SemanticStage::Vector => Some(Self::BuildAnn),
            SemanticStage::AnnIndex => Some(Self::ReconcileAndActivate),
            SemanticStage::ReconcileAndActivate => None,
        }
    }
}

impl MachineEvent for SemanticEvent {
    fn name(self) -> &'static str {
        match self {
            Self::ProjectGraph => "ProjectGraph",
            Self::IndexLexical => "IndexLexical",
            Self::EmbedVectors => "EmbedVectors",
            Self::BuildAnn => "BuildAnn",
            Self::ReconcileAndActivate => "ReconcileAndActivate",
        }
    }
}

impl MachineState for SemanticStage {
    fn stored(self) -> &'static str {
        self.as_str()
    }
}

/// The semantic-stage machine has no guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticGuard {}

/// The semantic-stage machine: `SourceCommit` start; `ReconcileAndActivate` is terminal.
pub const SEMANTIC_MACHINE: MachineDef<SemanticStage, SemanticEvent, SemanticGuard> = MachineDef {
    name: "SemanticMachine",
    states: &[
        SemanticStage::SourceCommit,
        SemanticStage::GraphProjection,
        SemanticStage::LexicalIndex,
        SemanticStage::Vector,
        SemanticStage::AnnIndex,
        SemanticStage::ReconcileAndActivate,
    ],
    initial: SemanticStage::SourceCommit,
    terminals: &[SemanticStage::ReconcileAndActivate],
    events: &[
        SemanticEvent::ProjectGraph,
        SemanticEvent::IndexLexical,
        SemanticEvent::EmbedVectors,
        SemanticEvent::BuildAnn,
        SemanticEvent::ReconcileAndActivate,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "S01",
            from: SemanticStage::SourceCommit,
            event: SemanticEvent::ProjectGraph,
            guard: None,
            to: SemanticStage::GraphProjection,
        },
        Edge {
            id: "S02",
            from: SemanticStage::GraphProjection,
            event: SemanticEvent::IndexLexical,
            guard: None,
            to: SemanticStage::LexicalIndex,
        },
        Edge {
            id: "S03",
            from: SemanticStage::LexicalIndex,
            event: SemanticEvent::EmbedVectors,
            guard: None,
            to: SemanticStage::Vector,
        },
        Edge {
            id: "S04",
            from: SemanticStage::Vector,
            event: SemanticEvent::BuildAnn,
            guard: None,
            to: SemanticStage::AnnIndex,
        },
        Edge {
            id: "S05",
            from: SemanticStage::AnnIndex,
            event: SemanticEvent::ReconcileAndActivate,
            guard: None,
            to: SemanticStage::ReconcileAndActivate,
        },
    ],
};

impl SemanticStage {
    /// The one transition function: the next stage, or the typed refusal.
    pub fn apply(self, event: SemanticEvent) -> Result<Self, TransitionRefused> {
        transition(&SEMANTIC_MACHINE, self, event, |guard| match guard {}).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-DURABLE-KERNEL-R874.18
    #[test]
    fn every_legal_transition_applies() {
        for edge in SEMANTIC_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
            assert_eq!(SemanticEvent::advancing(edge.from), Some(edge.event));
            assert_eq!(edge.to.predecessor(), Some(edge.from));
        }
        assert_eq!(SemanticStage::SourceCommit.predecessor(), None);
        assert_eq!(
            SemanticEvent::advancing(SemanticStage::ReconcileAndActivate),
            None
        );
    }

    // spec: MEDULLA-DURABLE-KERNEL-R874.18
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &SEMANTIC_MACHINE;
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
                let expected = if machine.terminals.contains(&state) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, expected);
                assert_eq!(refused.machine, "SemanticMachine");
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

    // spec: MEDULLA-DURABLE-KERNEL-R874.18
    #[test]
    fn no_writer_sets_stage_outside_apply() {
        let source = include_str!("stage.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let writes: Vec<&str> = production
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//") && line.contains(".stage = "))
            .collect();
        assert!(writes.is_empty(), "{writes:?}");
    }
}
