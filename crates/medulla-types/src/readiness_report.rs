//! The readiness report of the engine (MEDULLA-OBSERVE-R007.2).
//!
//! One report answers whether every graph serves complete reads and, when it does not, which
//! graph blocks and why. The `Health` call carries it as `HealthReport.readiness` and the
//! observability listener serves it as `GET /readyz`. A report holds graph names, state names and
//! reason codes only: never a token, a payload or graph content.

use serde::{Deserialize, Serialize};

/// The most blockers one report lists.
pub const MAX_HEALTH_BLOCKERS: usize = 100;

/// The readiness of one graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GraphReadiness {
    /// The graph's storage name.
    pub graph: String,
    /// The stored text of the graph's `ReadinessMachine` state.
    pub state: String,
    /// The reason code of a degraded graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    /// Unix milliseconds of the graph's last state change.
    pub since_unix_ms: u64,
}

/// One reason a graph, or the process, is not ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Blocker {
    /// The graph the blocker belongs to; empty when it belongs to the process.
    pub graph: String,
    /// The stored text of the graph's state.
    pub state: String,
    /// The reason code: the degrade reason, or the upper-case state name of a graph still
    /// opening, migrating or materializing, or `STATE_LOCK_BUSY`.
    pub reason_code: String,
    /// Unix milliseconds since the blocker began; 0 for a process blocker.
    pub since_unix_ms: u64,
    /// The task that blocks the graph, or the holder of the busy state lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// Whether the engine is ready, with every graph's state and at most
/// [`MAX_HEALTH_BLOCKERS`] blockers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReadinessReport {
    /// True when no graph blocks and the state lock answered in time.
    pub ready: bool,
    /// Every catalog graph's readiness, ascending by graph name.
    pub graphs: Vec<GraphReadiness>,
    /// The blockers, ascending by graph name; a process blocker comes first.
    pub blockers: Vec<Blocker>,
}
