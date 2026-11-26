//! The readiness report of the process (MEDULLA-OBSERVE-R007.2).
//!
//! [`readiness_report`] reads the process-wide readiness registry and builds the
//! [`ReadinessReport`] the `Health` call and the `/readyz` probe serve. The registry is read on
//! its own: no server state lock is taken, so the report answers while the state lock is held.

use medulla_types::readiness_report::{
    Blocker, GraphReadiness, ReadinessReport, MAX_HEALTH_BLOCKERS,
};
use medulla_types::transition_refused::MachineState;

use crate::machine::readiness::{ReadinessState, STATE_LOCK_BUSY};
use crate::registry::readiness::GraphReadinessRow;
use crate::watchdog::PAGE_IN_TASK_PREFIX;

/// The report of the readiness registry `rows`. `lock_busy` is `Some(holder)` when the server
/// state lock did not answer in time: the report is then not ready and its first blocker is the
/// process blocker `STATE_LOCK_BUSY`, which names the holder of the lock when one is known.
pub fn report_of(rows: Vec<GraphReadinessRow>, lock_busy: Option<Option<String>>) -> ReadinessReport {
    let mut graphs = Vec::with_capacity(rows.len());
    let mut blockers = Vec::new();
    if let Some(holder) = lock_busy {
        blockers.push(Blocker {
            graph: String::new(),
            state: ReadinessState::Degraded.stored().to_string(),
            reason_code: STATE_LOCK_BUSY.to_string(),
            since_unix_ms: 0,
            task: holder,
        });
    }
    for row in rows {
        let reason_code = row.reason.as_ref().map(|reason| reason.code.clone());
        if row.state != ReadinessState::Ready && blockers.len() < MAX_HEALTH_BLOCKERS {
            let task = row
                .reason
                .as_ref()
                .and_then(|reason| reason.task.clone())
                .or_else(|| {
                    (row.state == ReadinessState::Materializing)
                        .then(|| format!("{PAGE_IN_TASK_PREFIX}{}", row.graph))
                });
            blockers.push(Blocker {
                graph: row.graph.clone(),
                state: row.state.stored().to_string(),
                reason_code: reason_code
                    .clone()
                    .unwrap_or_else(|| row.state.stored().to_ascii_uppercase()),
                since_unix_ms: row.since_unix_ms,
                task,
            });
        }
        graphs.push(GraphReadiness {
            graph: row.graph,
            state: row.state.stored().to_string(),
            reason_code,
            since_unix_ms: row.since_unix_ms,
        });
    }
    ReadinessReport {
        ready: blockers.is_empty(),
        graphs,
        blockers,
    }
}

/// The report of the process-wide readiness registry.
pub fn readiness_report(lock_busy: Option<Option<String>>) -> ReadinessReport {
    report_of(crate::readiness().snapshot(), lock_busy)
}
