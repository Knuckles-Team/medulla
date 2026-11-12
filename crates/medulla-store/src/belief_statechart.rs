//! The **belief ledger statechart** (MEDULLA-DECISION-ENGINE-R700.3).
//!
//! One instance per `(tenant, graph)` holds the durable output of belief
//! revision: the claim texts the opposition check indexes, the latest revision
//! of each claim, the contradiction records, the retraction proposals and the
//! queue of claims whose evidence changed after a committed batch. The
//! `BeliefRevise` handler is the single writer. Passes on different graphs are
//! different instances, so they do not contend; passes on one graph are
//! serialised by the instance version.
//!
//! Events, each a self-loop on the one `ready` state that assigns the payload
//! keys of the same name:
//! * `enqueue` with `{ pending }`: the claims awaiting a pass.
//! * `record_pass` with `{ pending, texts, revisions, contradictions,
//!   retractions, passes }`: the result of one pass.

use medulla_statechart::{Action, ActionValue, Guard, State, StatechartDef, Transition};

/// Name of the belief ledger chart.
pub const BELIEF_CHART_NAME: &str = "belief_ledger";

const ENQUEUE_KEYS: &[&str] = &["pending"];
const PASS_KEYS: &[&str] = &[
    "pending",
    "texts",
    "revisions",
    "contradictions",
    "retractions",
    "passes",
];

fn assign_from_event(key: &str) -> Action {
    Action::Assign {
        key: key.to_string(),
        value: ActionValue::Event {
            key: key.to_string(),
        },
    }
}

fn self_loop(event: &str, keys: &[&str]) -> Transition {
    Transition::new("ready", event, "ready")
        .with_guard(Guard::Always)
        .with_actions(keys.iter().map(|key| assign_from_event(key)).collect())
}

/// Build the belief ledger `StatechartDef`. Pure and content-addressed, so
/// defining it twice is idempotent.
pub fn belief_statechart_def() -> StatechartDef {
    StatechartDef {
        name: BELIEF_CHART_NAME.to_string(),
        schema_version: 1,
        states: vec![State::new("ready")],
        alphabet: vec!["enqueue".to_string(), "record_pass".to_string()],
        transitions: vec![
            self_loop("enqueue", ENQUEUE_KEYS),
            self_loop("record_pass", PASS_KEYS),
        ],
        initial: "ready".to_string(),
        finals: Vec::new(),
        meta: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_statechart::context::{Context, EventInput};
    use medulla_statechart::{initial_configuration, step, validate};

    // spec: MEDULLA-DECISION-ENGINE-R700.3
    #[test]
    fn belief_chart_is_valid_and_assigns_the_pass_keys() {
        let def = belief_statechart_def();
        assert_eq!(def, belief_statechart_def(), "deterministic");
        assert!(validate(&def).is_ok());
        let (config, _) = initial_configuration(&def).unwrap();
        let queued = EventInput::with_payload("enqueue", serde_json::json!({"pending": ["c1"]}));
        let outcome = step(&def, &config, &Context::new(), &queued).unwrap();
        assert!(outcome.fired);
        assert_eq!(
            outcome.next_context.get("pending"),
            Some(&serde_json::json!(["c1"]))
        );
    }
}
