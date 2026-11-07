//! MEDULLA-PLACEMENT-R783.5 — `PartitionMoveMachine`, the online partition move edge table.
//!
//! The machine runs over the state KIND ([`PartitionKind`], the discriminant of
//! [`PartitionState`]). `Moving` carries its target group as payload, so the target comes in
//! with the event ([`PartitionMove`]) and goes out in the next `PlacementEntry`; no field is
//! added to the entry. [`PlacementEntry::apply_move`] is the one transition function and the
//! only place a partition's state changes.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use super::{PartitionState, PlacementEntry};
use crate::raft::GroupId;

/// The kind (discriminant) of a [`PartitionState`]: the state set of `PartitionMoveMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PartitionKind {
    /// Steady state.
    Active,
    /// The snapshot and catch-up window before the fenced cutover.
    Moving,
}

impl PartitionState {
    /// The kind of this state, with the move target dropped.
    pub fn kind(self) -> PartitionKind {
        match self {
            Self::Active => PartitionKind::Active,
            Self::Moving { .. } => PartitionKind::Moving,
        }
    }
}

impl MachineState for PartitionKind {
    fn stored(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Moving => "moving",
        }
    }
}

/// The kind of a [`PartitionMove`]: the event set of `PartitionMoveMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PartitionMoveEvent {
    /// Mark the partition as moving to a target group.
    StartMove,
    /// Flip the owning group to the target and advance the epoch.
    FenceCutover,
    /// Restore the pre-cutover route.
    AbortMove,
}

impl MachineEvent for PartitionMoveEvent {
    fn name(self) -> &'static str {
        match self {
            Self::StartMove => "StartMove",
            Self::FenceCutover => "FenceCutover",
            Self::AbortMove => "AbortMove",
        }
    }
}

/// An event with its payload: the move target and the facts the guards read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartitionMove {
    /// Start (or replay the start of) a move to `target`.
    StartMove {
        /// The group the partition moves to.
        target: GroupId,
    },
    /// Cut over to `target` at the freshly allocated `epoch`.
    FenceCutover {
        /// The group that becomes the owner.
        target: GroupId,
        /// The new routing epoch.
        epoch: u64,
    },
    /// Roll back a move that has not cut over.
    AbortMove {
        /// The group that must still own the partition.
        source: GroupId,
        /// The move target being abandoned.
        target: GroupId,
        /// The epoch the entry must still have.
        original_epoch: u64,
    },
}

impl PartitionMove {
    /// The payload-free event kind.
    pub fn event(self) -> PartitionMoveEvent {
        match self {
            Self::StartMove { .. } => PartitionMoveEvent::StartMove,
            Self::FenceCutover { .. } => PartitionMoveEvent::FenceCutover,
            Self::AbortMove { .. } => PartitionMoveEvent::AbortMove,
        }
    }
}

/// Guard of `PartitionMoveMachine`: the target is not the owning group.
pub const GUARD_TARGET_NOT_OWNER: &str = "TargetNotOwner";
/// Guard of `PartitionMoveMachine`: the target equals the move target.
pub const GUARD_TARGET_IS_MOVE_TARGET: &str = "TargetIsMoveTarget";
/// Guard of `PartitionMoveMachine`: the partition is moving to the requested target.
pub const GUARD_MOVING_TO_REQUESTED_TARGET: &str = "MovingToRequestedTarget";
/// Guard of `PartitionMoveMachine`: the source still owns the partition at the original epoch
/// and the move target matches.
pub const GUARD_SOURCE_OWNS_AT_ORIGINAL_EPOCH: &str = "SourceOwnsAtOriginalEpoch";

/// `PartitionMoveMachine`: the closed edge table. Every other pair of state and event is refused.
pub static PARTITION_MOVE_MACHINE: MachineDef<PartitionKind, PartitionMoveEvent, &'static str> =
    MachineDef {
        name: "PartitionMoveMachine",
        states: &[PartitionKind::Active, PartitionKind::Moving],
        initial: PartitionKind::Active,
        terminals: &[],
        events: &[
            PartitionMoveEvent::StartMove,
            PartitionMoveEvent::FenceCutover,
            PartitionMoveEvent::AbortMove,
        ],
        guards: &[
            GUARD_TARGET_NOT_OWNER,
            GUARD_TARGET_IS_MOVE_TARGET,
            GUARD_MOVING_TO_REQUESTED_TARGET,
            GUARD_SOURCE_OWNS_AT_ORIGINAL_EPOCH,
        ],
        edges: &[
            Edge {
                id: "P01",
                from: PartitionKind::Active,
                event: PartitionMoveEvent::StartMove,
                guard: Some(GUARD_TARGET_NOT_OWNER),
                to: PartitionKind::Moving,
            },
            Edge {
                id: "P02",
                from: PartitionKind::Moving,
                event: PartitionMoveEvent::StartMove,
                guard: Some(GUARD_TARGET_IS_MOVE_TARGET),
                to: PartitionKind::Moving,
            },
            Edge {
                id: "P03",
                from: PartitionKind::Moving,
                event: PartitionMoveEvent::FenceCutover,
                guard: Some(GUARD_MOVING_TO_REQUESTED_TARGET),
                to: PartitionKind::Active,
            },
            Edge {
                id: "P04",
                from: PartitionKind::Moving,
                event: PartitionMoveEvent::AbortMove,
                guard: Some(GUARD_SOURCE_OWNS_AT_ORIGINAL_EPOCH),
                to: PartitionKind::Active,
            },
        ],
    };

impl PlacementEntry {
    /// The one transition function of `PartitionMoveMachine`: the entry after `mv`, or the typed
    /// refusal. A replayed start (edge P02) returns the entry unchanged, so the caller writes
    /// nothing.
    pub fn apply_move(&self, mv: PartitionMove) -> Result<PlacementEntry, TransitionRefused> {
        let moving_to = |target: GroupId| self.state == PartitionState::Moving { target };
        let edge = transition(
            &PARTITION_MOVE_MACHINE,
            self.state.kind(),
            mv.event(),
            |guard| match (guard, mv) {
                (GUARD_TARGET_NOT_OWNER, PartitionMove::StartMove { target }) => {
                    self.group != target
                }
                (GUARD_TARGET_IS_MOVE_TARGET, PartitionMove::StartMove { target })
                | (GUARD_MOVING_TO_REQUESTED_TARGET, PartitionMove::FenceCutover { target, .. }) => {
                    moving_to(target)
                }
                (
                    GUARD_SOURCE_OWNS_AT_ORIGINAL_EPOCH,
                    PartitionMove::AbortMove {
                        source,
                        target,
                        original_epoch,
                    },
                ) => self.group == source && self.epoch == original_epoch && moving_to(target),
                _ => false,
            },
        )?;
        let mut next = self.clone();
        match (edge.id, mv) {
            ("P01", PartitionMove::StartMove { target }) => {
                next.state = PartitionState::Moving { target };
            }
            ("P03", PartitionMove::FenceCutover { target, epoch }) => {
                next.group = target;
                next.epoch = epoch;
                next.state = PartitionState::Active;
            }
            ("P04", PartitionMove::AbortMove { .. }) => next.state = PartitionState::Active,
            _ => {}
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raft::placement::PartitionKey;
    use medulla_types::transition_refused::TransitionRefusalReason;

    fn entry(group: GroupId, epoch: u64, state: PartitionState) -> PlacementEntry {
        PlacementEntry {
            key: PartitionKey::whole("tenant"),
            group,
            epoch,
            state,
        }
    }

    // spec: MEDULLA-PLACEMENT-R783.5
    #[test]
    fn edge_p01_active_start_move() {
        let active = entry(7, 11, PartitionState::Active);
        let moving = active
            .apply_move(PartitionMove::StartMove { target: 9 })
            .unwrap();
        assert_eq!(moving.state, PartitionState::Moving { target: 9 });
        assert_eq!((moving.group, moving.epoch), (7, 11), "no epoch bump");
        let refused = active
            .apply_move(PartitionMove::StartMove { target: 7 })
            .unwrap_err();
        assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
    }

    // spec: MEDULLA-PLACEMENT-R783.5
    #[test]
    fn edge_p02_moving_start_move_replays() {
        let moving = entry(7, 11, PartitionState::Moving { target: 9 });
        let replay = moving
            .apply_move(PartitionMove::StartMove { target: 9 })
            .unwrap();
        assert_eq!(replay, moving, "the replay writes nothing");
        let refused = moving
            .apply_move(PartitionMove::StartMove { target: 10 })
            .unwrap_err();
        assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
    }

    // spec: MEDULLA-PLACEMENT-R783.5
    #[test]
    fn edge_p03_moving_fence_cutover() {
        let moving = entry(7, 11, PartitionState::Moving { target: 9 });
        let cut = moving
            .apply_move(PartitionMove::FenceCutover {
                target: 9,
                epoch: 12,
            })
            .unwrap();
        assert_eq!(
            (cut.group, cut.epoch, cut.state),
            (9, 12, PartitionState::Active)
        );
        let refused = moving
            .apply_move(PartitionMove::FenceCutover {
                target: 8,
                epoch: 12,
            })
            .unwrap_err();
        assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
    }

    // spec: MEDULLA-PLACEMENT-R783.5
    #[test]
    fn edge_p04_moving_abort_move() {
        let moving = entry(7, 11, PartitionState::Moving { target: 9 });
        let abort = |source, target, original_epoch| PartitionMove::AbortMove {
            source,
            target,
            original_epoch,
        };
        let back = moving.apply_move(abort(7, 9, 11)).unwrap();
        assert_eq!(
            (back.group, back.epoch, back.state),
            (7, 11, PartitionState::Active)
        );
        for bad in [abort(6, 9, 11), abort(7, 9, 10), abort(7, 8, 11)] {
            let refused = moving.apply_move(bad).unwrap_err();
            assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
        }
    }

    // spec: MEDULLA-PLACEMENT-R783.5
    #[test]
    fn partition_move_edge_table_matches_move_plans() {
        let machine = &PARTITION_MOVE_MACHINE;
        assert_eq!(machine.edges.len(), 4);
        assert!(machine.terminals.is_empty());
        let active = entry(7, 11, PartitionState::Active);
        for event in [
            PartitionMove::FenceCutover {
                target: 9,
                epoch: 12,
            },
            PartitionMove::AbortMove {
                source: 7,
                target: 9,
                original_epoch: 11,
            },
        ] {
            let refused = active.apply_move(event).unwrap_err();
            assert_eq!(refused.reason, TransitionRefusalReason::IllegalTransition);
            assert_eq!(refused.machine, "PartitionMoveMachine");
        }
        assert_eq!(
            PartitionState::Moving { target: 3 }.kind(),
            PartitionKind::Moving
        );
    }
}
