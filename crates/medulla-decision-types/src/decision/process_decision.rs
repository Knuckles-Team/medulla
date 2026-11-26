//! MEDULLA-DECISION-ENGINE-R180.1: the process decision model -- the questions
//! a stored DMN decision table and a process branch step put to `Decide`, and
//! the verdicts that answer them.
//!
//! The engine evaluates and the harness applies the verdict; nothing here
//! decides anything. Every type refuses unknown fields. Input values are one of
//! string, integer, fixed-point decimal, boolean or date; there is no float.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use medulla_types::refusal_code::RefusalCode;

/// One typed value of a decision input or output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DmnValue {
    String(String),
    Integer(i64),
    /// Fixed point: `units` scaled down by `10^scale` (`1.25` is units 125,
    /// scale 2). Never a float.
    Decimal(FixedDecimal),
    Boolean(bool),
    /// An ISO-8601 calendar date, `YYYY-MM-DD`.
    Date(String),
}

/// A fixed-point decimal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FixedDecimal {
    pub units: i64,
    pub scale: u8,
}

/// Evaluate the stored decision table `decision_iri` over typed inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DecisionTableRequest {
    pub decision_iri: String,
    pub inputs: BTreeMap<String, DmnValue>,
}

/// Select the taken arms of the branch step `branch_task_id` of the sealed plan
/// `plan_digest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BranchSelectRequest {
    pub plan_digest: String,
    pub branch_task_id: String,
    pub inputs: BTreeMap<String, DmnValue>,
}

/// The outputs of a table evaluation, one row per output the hit policy
/// returns, and the ids of the rules that matched in declared order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TableVerdict {
    pub outputs: Vec<BTreeMap<String, DmnValue>>,
    pub matched_rule_ids: Vec<String>,
}

/// The arms a branch step takes. `matched_conditions` names each arm whose
/// condition held; `used_default` is true when the default arm was taken
/// because no condition held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BranchVerdict {
    pub taken_task_ids: Vec<String>,
    pub matched_conditions: Vec<String>,
    pub used_default: bool,
}

/// Abstention: UNIQUE, ANY or FIRST matched no rule (MEDULLA-DECISION-ENGINE-R180.5).
pub const DMN_NO_RULE_MATCHED: RefusalCode = RefusalCode::new("DMN_NO_RULE_MATCHED");
/// Abstention: UNIQUE matched more than one rule; the detail names the rule ids.
pub const DMN_UNIQUE_VIOLATED: RefusalCode = RefusalCode::new("DMN_UNIQUE_VIOLATED");
/// Abstention: ANY matched rules with different outputs; the detail names the rule ids.
pub const DMN_ANY_CONFLICT: RefusalCode = RefusalCode::new("DMN_ANY_CONFLICT");
/// Abstention: an entry or a hit policy outside the supported subset; the detail names it.
pub const DMN_EXPRESSION_UNSUPPORTED: RefusalCode = RefusalCode::new("DMN_EXPRESSION_UNSUPPORTED");
/// Abstention: a table input the request did not supply; the detail names it.
pub const DMN_INPUT_MISSING: RefusalCode = RefusalCode::new("DMN_INPUT_MISSING");
/// Abstention: no arm held and the branch has no default arm.
pub const BRANCH_NO_ARM: RefusalCode = RefusalCode::new("BRANCH_NO_ARM");
/// Abstention: no sealed plan carries the requested plan digest.
pub const PLAN_UNKNOWN: RefusalCode = RefusalCode::new("PLAN_UNKNOWN");

/// The refusal table of the `decision_table_eval` and `branch_select` families.
pub const REFUSALS: &[RefusalCode] = &[
    DMN_NO_RULE_MATCHED,
    DMN_UNIQUE_VIOLATED,
    DMN_ANY_CONFLICT,
    DMN_EXPRESSION_UNSUPPORTED,
    DMN_INPUT_MISSING,
    BRANCH_NO_ARM,
    PLAN_UNKNOWN,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> BTreeMap<String, DmnValue> {
        BTreeMap::from([
            ("age".to_string(), DmnValue::Integer(42)),
            (
                "rate".to_string(),
                DmnValue::Decimal(FixedDecimal {
                    units: 125,
                    scale: 2,
                }),
            ),
            ("vip".to_string(), DmnValue::Boolean(true)),
            ("tier".to_string(), DmnValue::String("gold".into())),
            ("since".to_string(), DmnValue::Date("2026-11-12".into())),
        ])
    }

    // spec: MEDULLA-DECISION-ENGINE-R180.1
    #[test]
    fn requests_round_trip() {
        let table = DecisionTableRequest {
            decision_iri: "https://example.org/dmn#discount".into(),
            inputs: inputs(),
        };
        let json = serde_json::to_string(&table).unwrap();
        assert_eq!(
            serde_json::from_str::<DecisionTableRequest>(&json).unwrap(),
            table
        );
        assert!(
            json.contains(r#""age":{"type":"integer","value":42}"#),
            "{json}"
        );

        let branch = BranchSelectRequest {
            plan_digest: "sha256:ab".into(),
            branch_task_id: "t1".into(),
            inputs: inputs(),
        };
        let json = serde_json::to_string(&branch).unwrap();
        assert_eq!(
            serde_json::from_str::<BranchSelectRequest>(&json).unwrap(),
            branch
        );

        let verdict = TableVerdict {
            outputs: vec![BTreeMap::from([(
                "discount".to_string(),
                DmnValue::Integer(10),
            )])],
            matched_rule_ids: vec!["r1".into()],
        };
        let json = serde_json::to_string(&verdict).unwrap();
        assert_eq!(
            serde_json::from_str::<TableVerdict>(&json).unwrap(),
            verdict
        );

        let arms = BranchVerdict {
            taken_task_ids: vec!["a".into(), "b".into()],
            matched_conditions: vec!["a".into()],
            used_default: false,
        };
        let json = serde_json::to_string(&arms).unwrap();
        assert_eq!(serde_json::from_str::<BranchVerdict>(&json).unwrap(), arms);

        // Unknown fields and unknown value types refuse.
        assert!(serde_json::from_str::<DecisionTableRequest>(
            r#"{"decision_iri":"x","inputs":{},"extra":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<BranchSelectRequest>(
            r#"{"plan_digest":"d","branch_task_id":"t","inputs":{},"extra":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<BranchVerdict>(
            r#"{"taken_task_ids":[],"matched_conditions":[],"used_default":false,"x":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<DmnValue>(r#"{"type":"float","value":1.5}"#).is_err());
        assert!(serde_json::from_str::<FixedDecimal>(r#"{"units":1,"scale":0,"x":1}"#).is_err());
    }
}
