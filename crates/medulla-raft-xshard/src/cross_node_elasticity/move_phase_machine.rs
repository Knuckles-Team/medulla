//! `MovePhaseMachine`: the explicit state machine of a cross-node move checkpoint
//! (MEDULLA-PLACEMENT-R783.7, organization constitution Article XXXI).
//!
//! The closed state set is [`MovePhase`]. The closed event set is [`MoveEvent`]. The
//! edge table [`MOVE_PHASE_MACHINE`] is the only description of the legal moves, and
//! [`MovePhase::apply`] is the only transition function. The one stored field is
//! [`MoveCheckpoint::phase`]; its only writer after construction is
//! [`MoveCheckpoint::advance`], which calls `apply`.
//!
//! A new move starts in the entry state its kind names: a hydrate move starts in
//! `Hydrate`, every other move in `Snapshot` (the declared initial state).

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use super::{MoveCheckpoint, MoveKind, MovePhase};

/// An event of `MovePhaseMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MoveEvent {
    /// The snapshot copy finished; the delta stream starts catching up.
    SnapshotCopied,
    /// The delta stream caught up; the source is fenced for cutover.
    DeltaCaughtUp,
    /// The fenced cutover committed on the target.
    CutoverCommitted,
    /// The object-tier hydrate finished on the target.
    Hydrated,
    /// The move was abandoned before it completed.
    Abort,
}

impl MachineState for MovePhase {
    fn stored(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::DeltaCatchUp => "delta_catch_up",
            Self::FencedCutover => "fenced_cutover",
            Self::Hydrate => "hydrate",
            Self::Complete => "complete",
            Self::Aborted => "aborted",
        }
    }
}

impl MachineEvent for MoveEvent {
    fn name(self) -> &'static str {
        match self {
            Self::SnapshotCopied => "SnapshotCopied",
            Self::DeltaCaughtUp => "DeltaCaughtUp",
            Self::CutoverCommitted => "CutoverCommitted",
            Self::Hydrated => "Hydrated",
            Self::Abort => "Abort",
        }
    }
}

/// `MovePhaseMachine`: the closed edge table. Every other pair of state and event is
/// refused. The machine names no guard.
pub static MOVE_PHASE_MACHINE: MachineDef<MovePhase, MoveEvent, &'static str> = MachineDef {
    name: "MovePhaseMachine",
    states: &[
        MovePhase::Snapshot,
        MovePhase::DeltaCatchUp,
        MovePhase::FencedCutover,
        MovePhase::Hydrate,
        MovePhase::Complete,
        MovePhase::Aborted,
    ],
    initial: MovePhase::Snapshot,
    terminals: &[MovePhase::Complete, MovePhase::Aborted],
    events: &[
        MoveEvent::SnapshotCopied,
        MoveEvent::DeltaCaughtUp,
        MoveEvent::CutoverCommitted,
        MoveEvent::Hydrated,
        MoveEvent::Abort,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "MP01",
            from: MovePhase::Snapshot,
            event: MoveEvent::SnapshotCopied,
            guard: None,
            to: MovePhase::DeltaCatchUp,
        },
        Edge {
            id: "MP02",
            from: MovePhase::DeltaCatchUp,
            event: MoveEvent::DeltaCaughtUp,
            guard: None,
            to: MovePhase::FencedCutover,
        },
        Edge {
            id: "MP03",
            from: MovePhase::FencedCutover,
            event: MoveEvent::CutoverCommitted,
            guard: None,
            to: MovePhase::Complete,
        },
        Edge {
            id: "MP04",
            from: MovePhase::Hydrate,
            event: MoveEvent::Hydrated,
            guard: None,
            to: MovePhase::Complete,
        },
        Edge {
            id: "MP05",
            from: MovePhase::Snapshot,
            event: MoveEvent::Abort,
            guard: None,
            to: MovePhase::Aborted,
        },
        Edge {
            id: "MP06",
            from: MovePhase::DeltaCatchUp,
            event: MoveEvent::Abort,
            guard: None,
            to: MovePhase::Aborted,
        },
        Edge {
            id: "MP07",
            from: MovePhase::FencedCutover,
            event: MoveEvent::Abort,
            guard: None,
            to: MovePhase::Aborted,
        },
        Edge {
            id: "MP08",
            from: MovePhase::Hydrate,
            event: MoveEvent::Abort,
            guard: None,
            to: MovePhase::Aborted,
        },
    ],
};

impl MovePhase {
    /// The one transition function of `MovePhaseMachine`.
    pub fn apply(self, event: MoveEvent) -> Result<Self, TransitionRefused> {
        transition(&MOVE_PHASE_MACHINE, self, event, |_| true).map(|edge| edge.to)
    }

    /// The state a new move of `kind` starts in.
    pub fn entry_for(kind: MoveKind) -> Self {
        match kind {
            MoveKind::Hydrate => Self::Hydrate,
            MoveKind::MovePrimary | MoveKind::AddFollower => MOVE_PHASE_MACHINE.initial,
        }
    }
}

impl MoveCheckpoint {
    /// The only writer of [`MoveCheckpoint::phase`] after construction: applies `event`
    /// through `MovePhaseMachine` and records the tick of the transition. An illegal
    /// event changes nothing and returns the typed refusal.
    pub fn advance(&mut self, event: MoveEvent, now_tick: u64) -> Result<(), TransitionRefused> {
        let next = self.phase.apply(event)?;
        self.phase = next;
        self.last_transition_tick = now_tick;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-PLACEMENT-R783.7
    #[test]
    fn every_legal_transition_applies() {
        for edge in MOVE_PHASE_MACHINE.edges {
            assert_eq!(
                edge.from.apply(edge.event).expect("legal edge applies"),
                edge.to,
                "edge {}",
                edge.id
            );
        }
        assert_eq!(MovePhase::entry_for(MoveKind::Hydrate), MovePhase::Hydrate);
        assert_eq!(
            MovePhase::entry_for(MoveKind::MovePrimary),
            MovePhase::Snapshot
        );
        assert_eq!(
            MovePhase::entry_for(MoveKind::AddFollower),
            MovePhase::Snapshot
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.7
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        for &from in MOVE_PHASE_MACHINE.states {
            for &event in MOVE_PHASE_MACHINE.events {
                if MOVE_PHASE_MACHINE
                    .edges
                    .iter()
                    .any(|e| e.from == from && e.event == event)
                {
                    continue;
                }
                let refused = from.apply(event).expect_err("illegal pair is refused");
                assert_eq!(refused.machine, "MovePhaseMachine");
                assert_eq!(refused.from, from.stored());
                assert_eq!(refused.event, event.name());
                let want = if MOVE_PHASE_MACHINE.is_terminal(from) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, want);
            }
        }
    }

    // spec: MEDULLA-PLACEMENT-R783.7
    #[test]
    fn no_writer_sets_phase_outside_apply() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("cross_node_elasticity");
        let mut assignments = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("read module dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read source");
            // Test modules build fixtures; only production code is scanned.
            let production = text.split("#[cfg(test)]").next().unwrap_or("");
            for line in production.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("//") {
                    continue;
                }
                if trimmed.contains(".phase =") {
                    assignments.push(format!("{}: {trimmed}", path.display()));
                }
            }
        }
        assert_eq!(
            assignments,
            vec![format!(
                "{}: self.phase = next;",
                dir.join("move_phase_machine.rs").display()
            )],
            "MoveCheckpoint.phase has a writer outside MoveCheckpoint::advance"
        );
    }
}
