//! The shard drift monitor machine (MEDULLA-DECISION-ENGINE-R174.8.3,
//! MEDULLA-FEDERATED-QUERY-R739.1, organization constitution Article XXXI).
//!
//! A shard monitor is `Warming` while it collects its reference window and `Monitoring`
//! once the reference is sealed. The edge table below is the machine; the ontology
//! twin is `:ShardMonitorMachine` in `crates/medulla-core/ontology/state-machines-v1.ttl`.
//! [`ShardMonitorState::apply`] runs the shared transition function over it, and an
//! illegal event is refused with the shared [`TransitionRefused`].

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The closed states of a shard monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShardMonitorState {
    /// Collecting the reference window.
    Warming,
    /// Comparing writes against the sealed reference.
    Monitoring,
}

impl MachineState for ShardMonitorState {
    fn stored(self) -> &'static str {
        match self {
            Self::Warming => "warming",
            Self::Monitoring => "monitoring",
        }
    }
}

/// The closed events of a shard monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShardMonitorEvent {
    /// A written vector reached the shard.
    Write,
    /// The reference window is complete.
    SealReference,
}

impl MachineEvent for ShardMonitorEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Write => "Write",
            Self::SealReference => "SealReference",
        }
    }
}

/// The guards of a shard monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShardMonitorGuard {
    /// The reference window holds at least the configured number of vectors.
    ReferenceComplete,
}

/// The facts the guards read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardMonitorFacts {
    /// Vectors collected into the reference window so far.
    pub reference_count: u64,
    /// Vectors the reference window needs (`drift.reference_vectors`).
    pub reference_vectors: u64,
}

/// `ShardMonitorMachine`: the closed edge table. Every other pair of state and event is
/// refused.
pub static SHARD_MONITOR_MACHINE: MachineDef<
    ShardMonitorState,
    ShardMonitorEvent,
    ShardMonitorGuard,
> = MachineDef {
    name: "ShardMonitorMachine",
    states: &[ShardMonitorState::Warming, ShardMonitorState::Monitoring],
    initial: ShardMonitorState::Warming,
    terminals: &[],
    events: &[ShardMonitorEvent::Write, ShardMonitorEvent::SealReference],
    guards: &[ShardMonitorGuard::ReferenceComplete],
    edges: &[
        Edge {
            id: "S01",
            from: ShardMonitorState::Warming,
            event: ShardMonitorEvent::Write,
            guard: None,
            to: ShardMonitorState::Warming,
        },
        Edge {
            id: "S02",
            from: ShardMonitorState::Warming,
            event: ShardMonitorEvent::SealReference,
            guard: Some(ShardMonitorGuard::ReferenceComplete),
            to: ShardMonitorState::Monitoring,
        },
        Edge {
            id: "S03",
            from: ShardMonitorState::Monitoring,
            event: ShardMonitorEvent::Write,
            guard: None,
            to: ShardMonitorState::Monitoring,
        },
    ],
};

impl ShardMonitorState {
    /// Apply `event` under `facts`: the next state, or the shared typed refusal.
    pub fn apply(
        self,
        event: ShardMonitorEvent,
        facts: &ShardMonitorFacts,
    ) -> Result<Self, TransitionRefused> {
        transition(&SHARD_MONITOR_MACHINE, self, event, |guard| match guard {
            ShardMonitorGuard::ReferenceComplete => {
                facts.reference_count >= facts.reference_vectors
            }
        })
        .map(|edge| edge.to)
    }
}
