//! `ReadinessMachine` (MEDULLA-OBSERVE-R004.2): the readiness of one graph.
//!
//! States: Opening (initial), Migrating, Materializing, Ready and Degraded. No state is
//! terminal: `Ready` and `Degraded` accept `Reopen` and `Degrade`. [`ReadinessState::apply`]
//! is the one transition function; a pair of state and event that no edge lists is refused
//! with `TRANSITION_REFUSED`. `Degraded` carries a typed [`DegradeReason`] in the graph's
//! record. The edges D01 to D12 are in `specs/engine-observability/data-model.md`.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};

/// The reason code of a graph whose materialization ended with a partial image.
pub const PARTIAL_MATERIALIZATION: &str = "PARTIAL_MATERIALIZATION";
/// The reason code of a graph whose materialization task made no progress.
pub const NO_PROGRESS: &str = "NO_PROGRESS";
/// The reason code of a graph whose namespace migration was refused.
pub const MIGRATION_REFUSED: &str = "MIGRATION_REFUSED";
/// The reason code of a graph whose state lock stayed busy.
pub const STATE_LOCK_BUSY: &str = "STATE_LOCK_BUSY";

/// The closed state set of `ReadinessMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessState {
    /// The graph is being opened.
    Opening,
    /// A migration visits the graph.
    Migrating,
    /// The graph is paged into memory.
    Materializing,
    /// The graph serves complete reads.
    Ready,
    /// A fault left the graph unable to serve complete reads.
    Degraded,
}

/// The closed event set of `ReadinessMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReadinessEvent {
    /// A migration began.
    BeginMigrate,
    /// Paging into memory began.
    BeginMaterialize,
    /// The manifest is complete.
    Materialized,
    /// A fault; carries a [`DegradeReason`] in the record.
    Degrade,
    /// The graph opens again.
    Reopen,
}

/// The guards of `ReadinessMachine`: none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadinessGuard {}

impl MachineState for ReadinessState {
    fn stored(self) -> &'static str {
        match self {
            Self::Opening => "opening",
            Self::Migrating => "migrating",
            Self::Materializing => "materializing",
            Self::Ready => "ready",
            Self::Degraded => "degraded",
        }
    }
}

impl MachineEvent for ReadinessEvent {
    fn name(self) -> &'static str {
        match self {
            Self::BeginMigrate => "BeginMigrate",
            Self::BeginMaterialize => "BeginMaterialize",
            Self::Materialized => "Materialized",
            Self::Degrade => "Degrade",
            Self::Reopen => "Reopen",
        }
    }
}

const fn edge(
    id: &'static str,
    from: ReadinessState,
    event: ReadinessEvent,
    to: ReadinessState,
) -> Edge<ReadinessState, ReadinessEvent, ReadinessGuard> {
    Edge {
        id,
        from,
        event,
        guard: None,
        to,
    }
}

use ReadinessEvent as E;
use ReadinessState as S;

/// `ReadinessMachine`: the closed edge table D01 to D12. Every other pair is refused.
pub static READINESS_MACHINE: MachineDef<ReadinessState, ReadinessEvent, ReadinessGuard> =
    MachineDef {
        name: "ReadinessMachine",
        states: &[
            S::Opening,
            S::Migrating,
            S::Materializing,
            S::Ready,
            S::Degraded,
        ],
        initial: S::Opening,
        terminals: &[],
        events: &[
            E::BeginMigrate,
            E::BeginMaterialize,
            E::Materialized,
            E::Degrade,
            E::Reopen,
        ],
        guards: &[],
        edges: &[
            edge("D01", S::Opening, E::BeginMigrate, S::Migrating),
            edge("D02", S::Opening, E::BeginMaterialize, S::Materializing),
            edge("D03", S::Opening, E::Materialized, S::Ready),
            edge("D04", S::Migrating, E::BeginMaterialize, S::Materializing),
            edge("D05", S::Materializing, E::Materialized, S::Ready),
            edge("D06", S::Opening, E::Degrade, S::Degraded),
            edge("D07", S::Migrating, E::Degrade, S::Degraded),
            edge("D08", S::Materializing, E::Degrade, S::Degraded),
            edge("D09", S::Ready, E::Degrade, S::Degraded),
            edge("D10", S::Degraded, E::Reopen, S::Opening),
            edge("D11", S::Ready, E::Reopen, S::Opening),
            edge("D12", S::Degraded, E::Degrade, S::Degraded),
        ],
    };

impl ReadinessState {
    /// Every state, in declaration order.
    pub const ALL: [ReadinessState; 5] = [
        S::Opening,
        S::Migrating,
        S::Materializing,
        S::Ready,
        S::Degraded,
    ];

    /// The state a graph takes when it opens.
    pub const fn initial() -> Self {
        S::Opening
    }

    /// The one transition function of `ReadinessMachine`: the next state, or
    /// `TRANSITION_REFUSED`.
    pub fn apply(self, event: ReadinessEvent) -> Result<Self, TransitionRefused> {
        transition(&READINESS_MACHINE, self, event, |guard| match guard {}).map(|edge| edge.to)
    }
}

/// Why a graph is `Degraded`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DegradeReason {
    /// A stored code: `PARTIAL_MATERIALIZATION`, `NO_PROGRESS`, `MIGRATION_REFUSED` or
    /// `STATE_LOCK_BUSY`.
    pub code: String,
    /// The name of the task that blocks the graph, when one does.
    pub task: Option<String>,
    /// A line for the operator. Never a payload or a secret.
    pub detail: String,
}
