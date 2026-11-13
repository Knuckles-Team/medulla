//! The **feature ledger statechart** (MEDULLA-DECISION-ENGINE-R707.3).
//!
//! One instance per tenant holds every ledger entry of the tenant's feature
//! ledger. The `FeatureLedger` handler is the single writer. Each entry carries
//! its own status, written only through `LedgerStatus::apply` (the explicit
//! `FeatureLedgerMachine`), so this chart is the storage of the entry map, not
//! a second machine: its one `ready` state never changes. Writes on one tenant
//! are serialised by the instance version.
//!
//! One event, a self-loop on `ready`: `record` with `{ entries }` replaces the
//! entry map with the payload's.

use medulla_statechart::{Action, ActionValue, Guard, State, StatechartDef, Transition};

/// Name of the feature ledger chart.
pub const FEATURE_LEDGER_CHART_NAME: &str = "feature_ledger";

/// The event that stores the entry map.
pub const RECORD_EVENT: &str = "record";

/// Build the feature ledger `StatechartDef`. Pure and content-addressed, so
/// defining it twice is idempotent.
pub fn feature_ledger_statechart_def() -> StatechartDef {
    StatechartDef {
        name: FEATURE_LEDGER_CHART_NAME.to_string(),
        schema_version: 1,
        states: vec![State::new("ready")],
        alphabet: vec![RECORD_EVENT.to_string()],
        transitions: vec![Transition::new("ready", RECORD_EVENT, "ready")
            .with_guard(Guard::Always)
            .with_actions(vec![Action::Assign {
                key: "entries".to_string(),
                value: ActionValue::Event {
                    key: "entries".to_string(),
                },
            }])],
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

    // spec: MEDULLA-DECISION-ENGINE-R707.3
    #[test]
    fn ledger_chart_is_valid_and_stores_the_entry_map() {
        let def = feature_ledger_statechart_def();
        assert_eq!(def, feature_ledger_statechart_def(), "deterministic");
        assert!(validate(&def).is_ok());
        let (config, _) = initial_configuration(&def).unwrap();
        let event =
            EventInput::with_payload(RECORD_EVENT, serde_json::json!({"entries": {"f": 1}}));
        let outcome = step(&def, &config, &Context::new(), &event).unwrap();
        assert!(outcome.fired);
        assert_eq!(
            outcome.next_context.get("entries"),
            Some(&serde_json::json!({"f": 1}))
        );
    }
}
