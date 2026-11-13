//! MEDULLA-FEDERATED-QUERY-R738.16: the outcome status of one trace as an explicit state
//! machine (`TraceOutcomeMachine`).
//!
//! `Pending` is the initial state: a draft record that no event has concluded. Exactly one
//! event concludes it: `Succeed`, `Fail`, `Cancel` or `Deny`, and each resulting state is
//! terminal. [`TraceOutcomeStatus::apply`] is the only function that produces a new status; a
//! refused event is the typed `TRANSITION_REFUSED` naming the machine, the current state and
//! the event. The status lives in `TraceOutcome.status`; `Pending` has no wire text, so a
//! stored or decoded record is always concluded. The ontology description is
//! `TraceOutcomeMachine` in `state-machines-v1.ttl`.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use super::TraceOutcomeStatus;

/// The closed event set of `TraceOutcomeMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TraceOutcomeEvent {
    /// The traced operation completed.
    Succeed,
    /// The traced operation failed.
    Fail,
    /// The traced operation was cancelled before it completed.
    Cancel,
    /// Policy denied the traced operation.
    Deny,
}

impl MachineState for TraceOutcomeStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Denied => "denied",
        }
    }
}

impl MachineEvent for TraceOutcomeEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Succeed => "Succeed",
            Self::Fail => "Fail",
            Self::Cancel => "Cancel",
            Self::Deny => "Deny",
        }
    }
}

/// `TraceOutcomeMachine`: the closed edge table. Every other pair is refused.
pub static TRACE_OUTCOME_MACHINE: MachineDef<TraceOutcomeStatus, TraceOutcomeEvent, &'static str> =
    MachineDef {
        name: "TraceOutcomeMachine",
        states: &[
            TraceOutcomeStatus::Pending,
            TraceOutcomeStatus::Succeeded,
            TraceOutcomeStatus::Failed,
            TraceOutcomeStatus::Cancelled,
            TraceOutcomeStatus::Denied,
        ],
        initial: TraceOutcomeStatus::Pending,
        terminals: &[
            TraceOutcomeStatus::Succeeded,
            TraceOutcomeStatus::Failed,
            TraceOutcomeStatus::Cancelled,
            TraceOutcomeStatus::Denied,
        ],
        events: &[
            TraceOutcomeEvent::Succeed,
            TraceOutcomeEvent::Fail,
            TraceOutcomeEvent::Cancel,
            TraceOutcomeEvent::Deny,
        ],
        guards: &[],
        edges: &[
            Edge {
                id: "T01",
                from: TraceOutcomeStatus::Pending,
                event: TraceOutcomeEvent::Succeed,
                guard: None,
                to: TraceOutcomeStatus::Succeeded,
            },
            Edge {
                id: "T02",
                from: TraceOutcomeStatus::Pending,
                event: TraceOutcomeEvent::Fail,
                guard: None,
                to: TraceOutcomeStatus::Failed,
            },
            Edge {
                id: "T03",
                from: TraceOutcomeStatus::Pending,
                event: TraceOutcomeEvent::Cancel,
                guard: None,
                to: TraceOutcomeStatus::Cancelled,
            },
            Edge {
                id: "T04",
                from: TraceOutcomeStatus::Pending,
                event: TraceOutcomeEvent::Deny,
                guard: None,
                to: TraceOutcomeStatus::Denied,
            },
        ],
    };

impl TraceOutcomeStatus {
    /// The one transition function of `TraceOutcomeMachine`.
    pub fn apply(self, event: TraceOutcomeEvent) -> Result<Self, TransitionRefused> {
        transition(&TRACE_OUTCOME_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epistemic_operations::{
        TraceOutcome, TraceOutcomePolicyDecision, TraceOutcomeSchemaVersion,
    };
    use medulla_types::transition_refused::TransitionRefusalReason;
    use std::collections::BTreeMap;

    fn draft() -> TraceOutcome {
        TraceOutcome {
            schema_version: TraceOutcomeSchemaVersion::V1,
            outcome_id: "o1".into(),
            trace_id: "t1".into(),
            context_id: "c1".into(),
            operation: "op".into(),
            status: TraceOutcomeStatus::Pending,
            started_at_ms: 1,
            ended_at_ms: 2,
            input_artifact_refs: vec![],
            output_artifact_refs: vec![],
            metrics: BTreeMap::new(),
            policy_decision: TraceOutcomePolicyDecision::NotApplicable,
            error_code: None,
            error_detail_ref: None,
            evaluation_scores: BTreeMap::new(),
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R738.16
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(TRACE_OUTCOME_MACHINE.initial, TraceOutcomeStatus::Pending);
        assert_eq!(
            TRACE_OUTCOME_MACHINE.edges.len(),
            TRACE_OUTCOME_MACHINE.events.len()
        );
        for edge in TRACE_OUTCOME_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
            let concluded = draft().conclude(edge.event).unwrap();
            assert_eq!(concluded.status, edge.to, "{}", edge.id);
            let wire = serde_json::to_string(&concluded.status).unwrap();
            assert_eq!(wire, format!("\"{}\"", edge.to.stored()));
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R738.16
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let refused = TraceOutcomeStatus::Succeeded
            .apply(TraceOutcomeEvent::Fail)
            .unwrap_err();
        assert_eq!(refused.machine, "TraceOutcomeMachine");
        assert_eq!(refused.from, "succeeded");
        assert_eq!(refused.event, "Fail");
        assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
        assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
        for state in TRACE_OUTCOME_MACHINE.states {
            for event in TRACE_OUTCOME_MACHINE.events {
                let listed = TRACE_OUTCOME_MACHINE
                    .edges
                    .iter()
                    .any(|edge| edge.from == *state && edge.event == *event);
                assert_eq!(state.apply(*event).is_ok(), listed, "{state:?} {event:?}");
            }
        }
        let concluded = draft().conclude(TraceOutcomeEvent::Succeed).unwrap();
        let again = concluded.clone().conclude(TraceOutcomeEvent::Deny);
        assert!(again.is_err(), "a concluded record is not concluded twice");
        assert_eq!(concluded.status, TraceOutcomeStatus::Succeeded);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R738.16
    #[test]
    fn pending_has_no_wire_text() {
        assert!(serde_json::to_string(&TraceOutcomeStatus::Pending).is_err());
        assert!(serde_json::from_str::<TraceOutcomeStatus>("\"pending\"").is_err());
    }

    /// No production file assigns `.status = TraceOutcomeStatus::...` or builds a record in a
    /// concluded state: the status changes only through `TraceOutcomeStatus::apply`.
    // spec: MEDULLA-FEDERATED-QUERY-R738.16
    #[test]
    fn no_writer_sets_status_outside_apply() {
        fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if path.is_dir() {
                    if name != "target" {
                        walk(&path, hits);
                    }
                } else if name.ends_with(".rs") && name != "trace_outcome_machine.rs" {
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    for (number, line) in text.lines().enumerate() {
                        let line = line.trim();
                        if line.starts_with("//") {
                            continue;
                        }
                        let direct = line.contains(".status = TraceOutcomeStatus::");
                        let concluded = line.starts_with("status: TraceOutcomeStatus::")
                            && !line.starts_with("status: TraceOutcomeStatus::Pending");
                        if direct || concluded {
                            hits.push(format!("{}:{}", path.display(), number + 1));
                        }
                    }
                }
            }
        }
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut hits = Vec::new();
        walk(&crates, &mut hits);
        assert!(
            hits.is_empty(),
            "direct TraceOutcomeStatus writes: {hits:?}"
        );
    }
}
