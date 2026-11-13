//! `ShardPlacementMachine`: the explicit state machine of one shard's placement
//! (MEDULLA-PLACEMENT-R783.11, organization constitution Article XXXI).
//!
//! The closed state set is [`PlacementState`]. The closed event set is
//! [`PlacementEvent`]. The edge table [`SHARD_PLACEMENT_MACHINE`] is the only
//! description of the legal moves, and [`PlacementState::apply`] is the only transition
//! function. The one stored field is [`ShardPlacement::state`]; its only writer is
//! [`ShardPlacement::transition`], which calls `apply`. The machine has no terminal
//! state: a placement lives as long as its shard.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use super::{PlacementState, ShardPlacement};

/// An event of `ShardPlacementMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlacementEvent {
    /// The primary steps down and the shard is served as a follower here.
    Demote,
    /// A follower is promoted to the primary.
    Promote,
    /// A snapshot copy of the shard starts.
    StartSnapshot,
    /// The snapshot copy finished; the delta stream starts catching up.
    SnapshotCopied,
    /// The delta stream caught up; the source is fenced for cutover.
    DeltaCaughtUp,
    /// The fenced cutover committed; the shard is resident on its new primary.
    CutoverCommitted,
    /// An in-flight copy or hydrate is abandoned.
    AbortMove,
    /// The shard starts draining off this node.
    StartDrain,
    /// The shard's data is offloaded to the object tier.
    Offload,
    /// A cold shard starts hydrating from the object tier.
    StartHydrate,
    /// The hydrate finished; the shard is resident.
    Hydrated,
    /// The shard is quarantined after a safety fault.
    Quarantine,
    /// A quarantined shard is released as a follower.
    Release,
}

impl MachineState for PlacementState {
    fn stored(self) -> &'static str {
        match self {
            Self::Resident => "resident",
            Self::Follower => "follower",
            Self::Cold => "cold",
            Self::Hydrating => "hydrating",
            Self::Snapshotting => "snapshotting",
            Self::DeltaCatchUp => "delta_catch_up",
            Self::FencedCutover => "fenced_cutover",
            Self::Draining => "draining",
            Self::Quarantined => "quarantined",
        }
    }
}

impl MachineEvent for PlacementEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Demote => "Demote",
            Self::Promote => "Promote",
            Self::StartSnapshot => "StartSnapshot",
            Self::SnapshotCopied => "SnapshotCopied",
            Self::DeltaCaughtUp => "DeltaCaughtUp",
            Self::CutoverCommitted => "CutoverCommitted",
            Self::AbortMove => "AbortMove",
            Self::StartDrain => "StartDrain",
            Self::Offload => "Offload",
            Self::StartHydrate => "StartHydrate",
            Self::Hydrated => "Hydrated",
            Self::Quarantine => "Quarantine",
            Self::Release => "Release",
        }
    }
}

const fn edge(
    id: &'static str,
    from: PlacementState,
    event: PlacementEvent,
    to: PlacementState,
) -> Edge<PlacementState, PlacementEvent, &'static str> {
    Edge {
        id,
        from,
        event,
        guard: None,
        to,
    }
}

use PlacementEvent as Ev;
use PlacementState as St;

/// `ShardPlacementMachine`: the closed edge table. Every other pair of state and event
/// is refused. The machine names no guard.
pub static SHARD_PLACEMENT_MACHINE: MachineDef<PlacementState, PlacementEvent, &'static str> =
    MachineDef {
        name: "ShardPlacementMachine",
        states: &[
            St::Resident,
            St::Follower,
            St::Cold,
            St::Hydrating,
            St::Snapshotting,
            St::DeltaCatchUp,
            St::FencedCutover,
            St::Draining,
            St::Quarantined,
        ],
        initial: St::Resident,
        terminals: &[],
        events: &[
            Ev::Demote,
            Ev::Promote,
            Ev::StartSnapshot,
            Ev::SnapshotCopied,
            Ev::DeltaCaughtUp,
            Ev::CutoverCommitted,
            Ev::AbortMove,
            Ev::StartDrain,
            Ev::Offload,
            Ev::StartHydrate,
            Ev::Hydrated,
            Ev::Quarantine,
            Ev::Release,
        ],
        guards: &[],
        edges: &[
            edge("SP01", St::Resident, Ev::Demote, St::Follower),
            edge("SP02", St::Follower, Ev::Promote, St::Resident),
            edge("SP03", St::Resident, Ev::StartSnapshot, St::Snapshotting),
            edge("SP04", St::Follower, Ev::StartSnapshot, St::Snapshotting),
            edge(
                "SP05",
                St::Snapshotting,
                Ev::SnapshotCopied,
                St::DeltaCatchUp,
            ),
            edge(
                "SP06",
                St::DeltaCatchUp,
                Ev::DeltaCaughtUp,
                St::FencedCutover,
            ),
            edge(
                "SP07",
                St::FencedCutover,
                Ev::CutoverCommitted,
                St::Resident,
            ),
            edge("SP08", St::Snapshotting, Ev::AbortMove, St::Resident),
            edge("SP09", St::DeltaCatchUp, Ev::AbortMove, St::Resident),
            edge("SP10", St::FencedCutover, Ev::AbortMove, St::Resident),
            edge("SP11", St::Hydrating, Ev::AbortMove, St::Cold),
            edge("SP12", St::Resident, Ev::StartDrain, St::Draining),
            edge("SP13", St::Follower, Ev::StartDrain, St::Draining),
            edge("SP14", St::Draining, Ev::Offload, St::Cold),
            edge("SP15", St::Resident, Ev::Offload, St::Cold),
            edge("SP16", St::Cold, Ev::StartHydrate, St::Hydrating),
            edge("SP17", St::Hydrating, Ev::Hydrated, St::Resident),
            edge("SP18", St::Resident, Ev::Quarantine, St::Quarantined),
            edge("SP19", St::Follower, Ev::Quarantine, St::Quarantined),
            edge("SP20", St::Draining, Ev::Quarantine, St::Quarantined),
            edge("SP21", St::Quarantined, Ev::Release, St::Follower),
        ],
    };

impl PlacementState {
    /// The one transition function of `ShardPlacementMachine`.
    pub fn apply(self, event: PlacementEvent) -> Result<Self, TransitionRefused> {
        transition(&SHARD_PLACEMENT_MACHINE, self, event, |_| true).map(|edge| edge.to)
    }
}

impl ShardPlacement {
    /// The only writer of [`ShardPlacement::state`]: applies `event` through
    /// `ShardPlacementMachine` and records the tick. An illegal event changes nothing and
    /// returns the typed refusal.
    pub fn transition(
        &mut self,
        event: PlacementEvent,
        now_tick: u64,
    ) -> Result<(), TransitionRefused> {
        let next = self.state.apply(event)?;
        self.state = next;
        self.last_transition_tick = now_tick;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-PLACEMENT-R783.11
    #[test]
    fn every_legal_transition_applies() {
        for edge in SHARD_PLACEMENT_MACHINE.edges {
            assert_eq!(
                edge.from.apply(edge.event).expect("legal edge applies"),
                edge.to,
                "edge {}",
                edge.id
            );
        }
        // Every state is reachable from the initial state.
        let mut seen = vec![SHARD_PLACEMENT_MACHINE.initial];
        let mut frontier = seen.clone();
        while let Some(state) = frontier.pop() {
            for edge in SHARD_PLACEMENT_MACHINE.edges_from(state) {
                if !seen.contains(&edge.to) {
                    seen.push(edge.to);
                    frontier.push(edge.to);
                }
            }
        }
        for state in SHARD_PLACEMENT_MACHINE.states {
            assert!(seen.contains(state), "{state:?} is unreachable");
        }
    }

    // spec: MEDULLA-PLACEMENT-R783.11
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        for &from in SHARD_PLACEMENT_MACHINE.states {
            for &event in SHARD_PLACEMENT_MACHINE.events {
                if SHARD_PLACEMENT_MACHINE
                    .edges
                    .iter()
                    .any(|e| e.from == from && e.event == event)
                {
                    continue;
                }
                let refused = from.apply(event).expect_err("illegal pair is refused");
                assert_eq!(refused.machine, "ShardPlacementMachine");
                assert_eq!(refused.from, from.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.reason, TransitionRefusalReason::IllegalTransition);
            }
        }
    }

    // spec: MEDULLA-PLACEMENT-R783.11
    #[test]
    fn no_writer_sets_state_outside_apply() {
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
            let whole = path.file_name().is_some_and(|n| n == "mod.rs");
            // In mod.rs the test module builds fixtures too, and must use the machine.
            let scanned = if whole {
                text.as_str()
            } else {
                text.split("#[cfg(test)]").next().unwrap_or("")
            };
            for line in scanned.lines() {
                let trimmed = line.trim();
                if !trimmed.starts_with("//") && trimmed.contains(".state =") {
                    assignments.push(format!("{}: {trimmed}", path.display()));
                }
            }
        }
        assert_eq!(
            assignments,
            vec![format!(
                "{}: self.state = next;",
                dir.join("shard_placement_machine.rs").display()
            )],
            "ShardPlacement.state has a writer outside ShardPlacement::transition"
        );
    }
}
