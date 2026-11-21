//! The readiness registry (MEDULLA-OBSERVE-R007.1): one `ReadinessMachine` per catalog graph.
//!
//! [`ReadinessRegistry::apply`] is the one writer of a graph's readiness state: it takes the
//! state through [`ReadinessState::apply`](crate::machine::readiness::ReadinessState::apply)
//! and records the time of the change and, for `Degrade`, the typed reason. A graph that was
//! never opened through the registry starts at `Opening` on its first event.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use medulla_types::transition_refused::TransitionRefused;
use parking_lot::Mutex;

use crate::machine::readiness::{DegradeReason, ReadinessEvent, ReadinessState};

/// One graph's readiness row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphReadinessRow {
    /// The graph's storage name.
    pub graph: String,
    /// The machine's state.
    pub state: ReadinessState,
    /// Why the graph is `Degraded`; `None` in every other state.
    pub reason: Option<DegradeReason>,
    /// Unix milliseconds of the last state change.
    pub since_unix_ms: u64,
}

/// One `ReadinessMachine` per graph.
#[derive(Default)]
pub struct ReadinessRegistry {
    graphs: Mutex<BTreeMap<String, GraphReadinessRow>>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

impl ReadinessRegistry {
    /// An empty registry. Tests build isolated instances; the process uses
    /// [`readiness`](crate::readiness).
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a graph at `Opening`. A graph the registry already holds keeps its row.
    pub fn open(&self, graph: &str) {
        self.graphs
            .lock()
            .entry(graph.to_string())
            .or_insert_with(|| GraphReadinessRow {
                graph: graph.to_string(),
                state: ReadinessState::initial(),
                reason: None,
                since_unix_ms: now_ms(),
            });
    }

    /// Apply one event to a graph's machine and return the new state, or
    /// `TRANSITION_REFUSED` when no edge lists the pair (the row is unchanged).
    /// `reason` is kept for `Degrade` and dropped for every other event.
    pub fn apply(
        &self,
        graph: &str,
        event: ReadinessEvent,
        reason: Option<DegradeReason>,
    ) -> Result<ReadinessState, TransitionRefused> {
        let mut graphs = self.graphs.lock();
        let row = graphs
            .entry(graph.to_string())
            .or_insert_with(|| GraphReadinessRow {
                graph: graph.to_string(),
                state: ReadinessState::initial(),
                reason: None,
                since_unix_ms: now_ms(),
            });
        let next = row.state.apply(event)?;
        row.state = next;
        row.reason = if event == ReadinessEvent::Degrade {
            reason
        } else {
            None
        };
        row.since_unix_ms = now_ms();
        Ok(next)
    }

    /// Degrade a graph with a typed reason (`Degrade`).
    pub fn degrade(
        &self,
        graph: &str,
        reason: DegradeReason,
    ) -> Result<ReadinessState, TransitionRefused> {
        self.apply(graph, ReadinessEvent::Degrade, Some(reason))
    }

    /// One graph's row.
    pub fn get(&self, graph: &str) -> Option<GraphReadinessRow> {
        self.graphs.lock().get(graph).cloned()
    }

    /// Every graph's row, ascending by graph name.
    pub fn snapshot(&self) -> Vec<GraphReadinessRow> {
        self.graphs.lock().values().cloned().collect()
    }

    /// Forget a graph that left the catalog.
    pub fn remove(&self, graph: &str) {
        self.graphs.lock().remove(graph);
    }
}
