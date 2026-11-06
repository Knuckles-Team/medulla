//! The **claim statechart** (MEDULLA-DECISION-ENGINE-R652.3).
//!
//! A research claim is one instance of this chart in the `medulla-statechart`
//! store: its state is the `ClaimState`, its context carries the transition
//! log, the evidence, the log-odds, the stances and the contradiction pairs.
//! The `Claims` handler is the single writer. As in `loop_statechart`, the
//! handler computes the guard booleans (the log-odds comparisons of R652.2)
//! and passes them in the event payload; the chart never compares numbers.
//!
//! Events:
//! * `advance` with payload `{ to, guard_met, transitions }`: one edge of the
//!   lifecycle table, enabled when `guard_met` is true; assigns `transitions`.
//! * `record_evidence` with payload `{ evidence, log_odds_micro, stances,
//!   contradiction_pairs, feeds }`: a self-loop on every non-final state that
//!   assigns those keys.
//!
//! A sealed research artifact is an instance of the one-state
//! `research_artifact` chart; its context holds the artifact.

use medulla_statechart::{Action, ActionValue, Guard, State, StatechartDef, Transition};

/// Name of the claim chart.
pub const CLAIM_CHART_NAME: &str = "claim";
/// Name of the artifact chart.
pub const ARTIFACT_CHART_NAME: &str = "research_artifact";

/// The five claim states, in lifecycle-table order.
const STATES: &[&str] = &["proposed", "supported", "contested", "accepted", "retired"];

/// The legal edges of the lifecycle table (R652.2).
const EDGES: &[(&str, &str)] = &[
    ("proposed", "supported"),
    ("proposed", "contested"),
    ("proposed", "retired"),
    ("supported", "accepted"),
    ("supported", "contested"),
    ("supported", "retired"),
    ("contested", "supported"),
    ("contested", "retired"),
    ("accepted", "contested"),
    ("accepted", "retired"),
];

/// The context keys `record_evidence` assigns from the event payload.
const EVIDENCE_KEYS: &[&str] = &[
    "evidence",
    "log_odds_micro",
    "stances",
    "contradiction_pairs",
    "feeds",
];

fn event_eq(key: &str, value: serde_json::Value) -> Guard {
    Guard::EventEq {
        key: key.to_string(),
        value,
    }
}

fn assign_from_event(key: &str) -> Action {
    Action::Assign {
        key: key.to_string(),
        value: ActionValue::Event {
            key: key.to_string(),
        },
    }
}

/// Build the claim `StatechartDef`. Pure and content-addressed, so defining
/// it twice is idempotent.
pub fn claim_statechart_def() -> StatechartDef {
    let mut transitions: Vec<Transition> = EDGES
        .iter()
        .map(|(from, to)| {
            Transition::new(*from, "advance", *to)
                .with_guard(Guard::All {
                    guards: vec![
                        event_eq("to", serde_json::json!(*to)),
                        event_eq("guard_met", serde_json::json!(true)),
                    ],
                })
                .with_actions(vec![assign_from_event("transitions")])
        })
        .collect();
    transitions.extend(
        STATES
            .iter()
            .filter(|state| **state != "retired")
            .map(|state| {
                Transition::new(*state, "record_evidence", *state)
                    .with_guard(Guard::Always)
                    .with_actions(
                        EVIDENCE_KEYS
                            .iter()
                            .map(|key| assign_from_event(key))
                            .collect(),
                    )
            }),
    );
    StatechartDef {
        name: CLAIM_CHART_NAME.to_string(),
        schema_version: 1,
        states: STATES.iter().map(|id| State::new(*id)).collect(),
        alphabet: vec!["advance".to_string(), "record_evidence".to_string()],
        transitions,
        initial: "proposed".to_string(),
        finals: vec!["retired".to_string()],
        meta: Default::default(),
    }
}

/// Build the one-state chart that holds a sealed research artifact.
pub fn research_artifact_statechart_def() -> StatechartDef {
    StatechartDef {
        name: ARTIFACT_CHART_NAME.to_string(),
        schema_version: 1,
        states: vec![State::new("sealed")],
        alphabet: Vec::new(),
        transitions: Vec::new(),
        initial: "sealed".to_string(),
        finals: Vec::new(),
        meta: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_statechart::context::{Context, EventInput};
    use medulla_statechart::{initial_configuration, step, validate};

    // spec: MEDULLA-DECISION-ENGINE-R652.3
    #[test]
    fn claim_chart_is_valid_and_follows_the_lifecycle_table() {
        let def = claim_statechart_def();
        assert_eq!(def, claim_statechart_def(), "deterministic");
        assert!(validate(&def).is_ok());
        assert!(validate(&research_artifact_statechart_def()).is_ok());

        let (config, _) = initial_configuration(&def).unwrap();
        let ctx = Context::new();
        let go = |to: &str, guard_met: bool| {
            EventInput::with_payload(
                "advance",
                serde_json::json!({"to": to, "guard_met": guard_met, "transitions": []}),
            )
        };
        // The guard boolean gates the edge.
        let blocked = step(&def, &config, &ctx, &go("supported", false)).unwrap();
        assert!(!blocked.fired);
        let fired = step(&def, &config, &ctx, &go("supported", true)).unwrap();
        assert!(fired.fired);
        assert!(fired.next.contains("supported"));
        // An edge outside the table never fires.
        let illegal = step(&def, &config, &ctx, &go("accepted", true)).unwrap();
        assert!(!illegal.fired);
        // Evidence is a self-loop that assigns the payload keys.
        let record = EventInput::with_payload(
            "record_evidence",
            serde_json::json!({
                "evidence": [1], "log_odds_micro": 5, "stances": {},
                "contradiction_pairs": [], "feeds": [],
            }),
        );
        let recorded = step(&def, &config, &ctx, &record).unwrap();
        assert!(recorded.fired);
        assert!(recorded.next.contains("proposed"));
        assert_eq!(
            recorded.next_context.get("log_odds_micro"),
            Some(&serde_json::json!(5))
        );
    }
}
