//! MEDULLA-PLACEMENT-R783.8 — `MoveStageMachine`, the move journal stage edge table.
//!
//! The machine runs over [`MoveStage`], the stage a [`PartitionMoveJournal`] records. It is
//! separate from `PartitionMoveMachine` (R783.5), which runs over the placement entry's
//! `PartitionState`: the journal stage and the placement state are two persisted fields with two
//! edge tables, and `PlacementEntry::apply_move` never writes a `MoveStage`.
//! [`MoveStage::apply`] is the one transition function and
//! [`PartitionMoveJournal::apply_stage`] the only code that assigns the journal's `stage` after
//! construction. The fence observations that choose an event stay at the call sites in
//! `reshard.rs`.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use super::{MoveStage, PartitionMoveJournal};

impl MachineState for MoveStage {
    fn stored(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Moving => "moving",
            Self::Transferring => "transferring",
            Self::ReadyForCutover => "ready_for_cutover",
            Self::CutoverCommitted => "cutover_committed",
            Self::Aborting => "aborting",
            Self::Completed => "completed",
            Self::Aborted => "aborted",
        }
    }
}

/// The event set of `MoveStageMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MoveStageEvent {
    /// The source partition is marked as moving.
    StartMove,
    /// The graph transfer begins, or resumes.
    BeginTransfer,
    /// Every graph of the inventory is transferred.
    FinishTransfer,
    /// The fenced cutover committed.
    CommitCutover,
    /// Every graph is routed to the target and the move is retained as completed.
    Complete,
    /// An abort is requested.
    BeginAbort,
    /// The source route is restored and the abort is retained.
    FinishAbort,
}

impl MachineEvent for MoveStageEvent {
    fn name(self) -> &'static str {
        match self {
            Self::StartMove => "StartMove",
            Self::BeginTransfer => "BeginTransfer",
            Self::FinishTransfer => "FinishTransfer",
            Self::CommitCutover => "CommitCutover",
            Self::Complete => "Complete",
            Self::BeginAbort => "BeginAbort",
            Self::FinishAbort => "FinishAbort",
        }
    }
}

/// `MoveStageMachine`: the closed edge table, all edges unguarded. Every other pair of stage and
/// event is refused.
pub static MOVE_STAGE_MACHINE: MachineDef<MoveStage, MoveStageEvent, &'static str> = MachineDef {
    name: "MoveStageMachine",
    states: &[
        MoveStage::Planned,
        MoveStage::Moving,
        MoveStage::Transferring,
        MoveStage::ReadyForCutover,
        MoveStage::CutoverCommitted,
        MoveStage::Aborting,
        MoveStage::Completed,
        MoveStage::Aborted,
    ],
    initial: MoveStage::Planned,
    terminals: &[MoveStage::Completed, MoveStage::Aborted],
    events: &[
        MoveStageEvent::StartMove,
        MoveStageEvent::BeginTransfer,
        MoveStageEvent::FinishTransfer,
        MoveStageEvent::CommitCutover,
        MoveStageEvent::Complete,
        MoveStageEvent::BeginAbort,
        MoveStageEvent::FinishAbort,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "S01",
            from: MoveStage::Planned,
            event: MoveStageEvent::StartMove,
            guard: None,
            to: MoveStage::Moving,
        },
        Edge {
            id: "S02",
            from: MoveStage::Planned,
            event: MoveStageEvent::BeginTransfer,
            guard: None,
            to: MoveStage::Transferring,
        },
        Edge {
            id: "S03",
            from: MoveStage::Moving,
            event: MoveStageEvent::BeginTransfer,
            guard: None,
            to: MoveStage::Transferring,
        },
        // A resumed transfer.
        Edge {
            id: "S04",
            from: MoveStage::Transferring,
            event: MoveStageEvent::BeginTransfer,
            guard: None,
            to: MoveStage::Transferring,
        },
        Edge {
            id: "S05",
            from: MoveStage::Transferring,
            event: MoveStageEvent::FinishTransfer,
            guard: None,
            to: MoveStage::ReadyForCutover,
        },
        Edge {
            id: "S06",
            from: MoveStage::ReadyForCutover,
            event: MoveStageEvent::CommitCutover,
            guard: None,
            to: MoveStage::CutoverCommitted,
        },
        // The cutover won the race with the abort.
        Edge {
            id: "S07",
            from: MoveStage::Aborting,
            event: MoveStageEvent::CommitCutover,
            guard: None,
            to: MoveStage::CutoverCommitted,
        },
        // Recovery replay.
        Edge {
            id: "S08",
            from: MoveStage::CutoverCommitted,
            event: MoveStageEvent::CommitCutover,
            guard: None,
            to: MoveStage::CutoverCommitted,
        },
        Edge {
            id: "S09",
            from: MoveStage::CutoverCommitted,
            event: MoveStageEvent::Complete,
            guard: None,
            to: MoveStage::Completed,
        },
        Edge {
            id: "S10",
            from: MoveStage::Planned,
            event: MoveStageEvent::BeginAbort,
            guard: None,
            to: MoveStage::Aborting,
        },
        Edge {
            id: "S11",
            from: MoveStage::Moving,
            event: MoveStageEvent::BeginAbort,
            guard: None,
            to: MoveStage::Aborting,
        },
        Edge {
            id: "S12",
            from: MoveStage::Transferring,
            event: MoveStageEvent::BeginAbort,
            guard: None,
            to: MoveStage::Aborting,
        },
        Edge {
            id: "S13",
            from: MoveStage::ReadyForCutover,
            event: MoveStageEvent::BeginAbort,
            guard: None,
            to: MoveStage::Aborting,
        },
        Edge {
            id: "S14",
            from: MoveStage::Aborting,
            event: MoveStageEvent::FinishAbort,
            guard: None,
            to: MoveStage::Aborted,
        },
    ],
};

impl MoveStage {
    /// The one transition function of `MoveStageMachine`: the next stage, or the typed refusal.
    pub fn apply(self, event: MoveStageEvent) -> Result<MoveStage, TransitionRefused> {
        transition(&MOVE_STAGE_MACHINE, self, event, |_| true).map(|edge| edge.to)
    }
}

impl PartitionMoveJournal {
    /// Apply `event` to the journal's stage. The only code that assigns `stage` after
    /// construction; an illegal pair changes nothing.
    pub fn apply_stage(&mut self, event: MoveStageEvent) -> Result<(), TransitionRefused> {
        self.stage = self.stage.apply(event)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{PartitionKey, PartitionState, PlacementEntry};

    fn journal() -> PartitionMoveJournal {
        let entry = PlacementEntry {
            key: PartitionKey {
                tenant: "tenant-a".to_string(),
                range_start: 0,
                range_end: u64::MAX,
            },
            group: 1,
            epoch: 1,
            state: PartitionState::Active,
        };
        PartitionMoveJournal::new(&entry, 2, vec!["graph-a".to_string()]).unwrap()
    }

    // proves R783.8: BUILD: the move journal stage MoveStage is its own explicit state machine, ...
    // spec: MEDULLA-PLACEMENT-R783.8
    #[test]
    fn every_legal_transition_applies() {
        use MoveStage::*;
        use MoveStageEvent::*;
        let legal = [
            ("S01", Planned, StartMove, Moving),
            ("S02", Planned, BeginTransfer, Transferring),
            ("S03", Moving, BeginTransfer, Transferring),
            ("S04", Transferring, BeginTransfer, Transferring),
            ("S05", Transferring, FinishTransfer, ReadyForCutover),
            ("S06", ReadyForCutover, CommitCutover, CutoverCommitted),
            ("S07", Aborting, CommitCutover, CutoverCommitted),
            ("S08", CutoverCommitted, CommitCutover, CutoverCommitted),
            ("S09", CutoverCommitted, Complete, Completed),
            ("S10", Planned, BeginAbort, Aborting),
            ("S11", Moving, BeginAbort, Aborting),
            ("S12", Transferring, BeginAbort, Aborting),
            ("S13", ReadyForCutover, BeginAbort, Aborting),
            ("S14", Aborting, FinishAbort, Aborted),
        ];
        assert_eq!(legal.len(), MOVE_STAGE_MACHINE.edges.len());
        for (id, from, event, to) in legal {
            assert_eq!(from.apply(event), Ok(to), "{id}");
            let edge = MOVE_STAGE_MACHINE
                .edges
                .iter()
                .find(|edge| edge.id == id)
                .unwrap_or_else(|| panic!("{id} is in the edge table"));
            assert_eq!((edge.from, edge.event, edge.to), (from, event, to), "{id}");
            let mut moved = journal();
            moved.stage = from;
            moved.apply_stage(event).unwrap();
            assert_eq!(moved.stage, to, "{id}");
        }
        // The terminal set the machine declares is the one `MoveStage::terminal` answers.
        for stage in MOVE_STAGE_MACHINE.states {
            assert_eq!(
                stage.terminal(),
                MOVE_STAGE_MACHINE.terminals.contains(stage),
                "{stage:?}"
            );
        }
        assert_eq!(MOVE_STAGE_MACHINE.initial, journal().stage);
    }

    // spec: MEDULLA-PLACEMENT-R783.8
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        for &stage in MOVE_STAGE_MACHINE.states {
            for &event in MOVE_STAGE_MACHINE.events {
                let legal = MOVE_STAGE_MACHINE
                    .edges
                    .iter()
                    .any(|edge| edge.from == stage && edge.event == event);
                if legal {
                    continue;
                }
                let refused = stage
                    .apply(event)
                    .expect_err(&format!("{stage:?} on {event:?} must be refused"));
                assert_eq!(refused.machine, "MoveStageMachine");
                assert_eq!(refused.from, stage.stored());
                assert_eq!(refused.event, event.name());
                // An illegal pair changes nothing.
                let mut unchanged = journal();
                unchanged.stage = stage;
                let before = unchanged.clone();
                assert!(unchanged.apply_stage(event).is_err());
                assert_eq!(unchanged, before);
            }
        }
    }

    // spec: MEDULLA-PLACEMENT-R783.8
    #[test]
    fn no_writer_sets_stage_outside_apply() {
        fn scan(dir: &std::path::Path, hits: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    scan(&path, hits);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(&path).unwrap();
                    // Everything before the first test module is production code.
                    let production = source.split("#[cfg(test)]").next().unwrap_or("");
                    for (number, line) in production.lines().enumerate() {
                        if line.contains(".stage = MoveStage::") {
                            hits.push(format!("{}:{}", path.display(), number + 1));
                        }
                    }
                }
            }
        }
        let mut hits = Vec::new();
        scan(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut hits,
        );
        assert!(
            hits.is_empty(),
            "a stage assignment outside `apply_stage`: {hits:?}"
        );
    }
}
