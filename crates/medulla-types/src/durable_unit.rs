//! MEDULLA-ORCH-VOCAB-R009.700 — a durable run unit, mirrored as a node that
//! links to its `:RunTrace` by `run_id` and to its produced artifacts.

use serde::{Deserialize, Serialize};

/// The closed status of a durable unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DurableUnitStatus {
    InFlight,
    Completed,
    Failed,
    Abandoned,
}

/// One durable run unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DurableUnit {
    pub unit_id: String,
    pub run_id: String,
    pub status: DurableUnitStatus,
    #[serde(default)]
    pub artifact_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
}

/// Prior units of a resumed run, in `unit_id` order.
pub fn units_in_resume_order(mut units: Vec<DurableUnit>) -> Vec<DurableUnit> {
    units.sort_by(|a, b| a.unit_id.cmp(&b.unit_id));
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-ORCH-VOCAB-R009.700
    #[test]
    fn status_is_closed_and_round_trips() {
        let unit = DurableUnit {
            unit_id: "unit:2".into(),
            run_id: "run:1".into(),
            status: DurableUnitStatus::InFlight,
            artifact_refs: vec!["cas:a".into()],
            resumed_from: Some("unit:1".into()),
        };
        let json = serde_json::to_value(&unit).expect("encode");
        assert_eq!(json["status"], "in_flight");
        let back: DurableUnit = serde_json::from_value(json.clone()).expect("decode");
        assert_eq!(back, unit);
        let mut bad = json;
        bad["status"] = serde_json::json!("paused");
        assert!(serde_json::from_value::<DurableUnit>(bad).is_err());
        let first = DurableUnit {
            unit_id: "unit:1".into(),
            ..unit.clone()
        };
        let ordered = units_in_resume_order(vec![unit, first]);
        assert_eq!(ordered[0].unit_id, "unit:1");
    }
}
