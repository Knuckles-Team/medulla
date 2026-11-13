//! Trust ledger and corrigibility verdict: the wire model (MEDULLA-DECISION-ENGINE-R752.750).
//!
//! `Decide` answers whether an action class may run without a person's
//! confirmation. A reversible class earns that by a run of clean outcomes; an
//! irreversible class never does. Nothing here reads a clock or a store.

use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;
use super::record::outcome::RefusalCode;

/// Whether an action can be undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum IrreversibilityClass {
    Reversible,
    Irreversible,
}

/// The consecutive clean outcomes of one action class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TrustLedger {
    pub action_class: String,
    /// Independent successes since the last failure.
    pub clean_outcomes: u32,
    /// The evaluation id of the last failure; `None` when there was none.
    #[serde(default)]
    pub last_failure_evaluation: Option<String>,
}

/// What the corrigibility rule says about one action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CorrigibilityVerdict {
    Allow,
    RequireConfirmation,
}

/// An irreversible action, or any question of safety class `Security`,
/// `Policy`, `WriteBack` or `Irreversible`, needs a person's approval.
pub const ACTION_NEEDS_APPROVAL: RefusalCode = RefusalCode::new("ACTION_NEEDS_APPROVAL");

/// The `Autonomy` family's refusal table.
pub const REFUSALS: &[RefusalCode] = &[ACTION_NEEDS_APPROVAL];

/// The `autonomy.*` knobs (MEDULLA-DECISION-ENGINE-R752.755): the clean run a
/// reversible class needs, and the fixed rule that an irreversible class never
/// clears.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "autonomy.clean_outcomes",
        unit: "outcomes",
        default: KnobValue::Integer(20),
        min: KnobValue::Integer(5),
        max: KnobValue::Integer(1000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "autonomy.irreversible_needs_confirmation",
        unit: "flag",
        default: KnobValue::Integer(1),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(1),
        safety: KnobSafety::Fixed,
    },
];

/// The clean run a reversible class needs, from the policy in force.
pub fn clean_outcomes_needed(policy: &DecisionPolicy) -> u32 {
    match policy.knob(&KNOBS[0]) {
        KnobValue::Integer(value) => u32::try_from(value).unwrap_or(20).clamp(5, 1000),
        _ => 20,
    }
}

/// The body of an autonomy question. The action class is the question's one
/// declared option; its outcomes are read from the decision log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AutonomyRequest {
    pub irreversibility: IrreversibilityClass,
}

/// The verdict, the clean run behind it and the refusal code it carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AutonomyReading {
    pub verdict: CorrigibilityVerdict,
    pub clean_outcomes: u32,
    /// `ACTION_NEEDS_APPROVAL` when the rule, not the ledger, requires approval.
    #[serde(default)]
    pub code: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R752.750
    #[test]
    fn autonomy_types_round_trip() {
        for class in [
            IrreversibilityClass::Reversible,
            IrreversibilityClass::Irreversible,
        ] {
            let text = serde_json::to_string(&class).unwrap();
            assert_eq!(
                serde_json::from_str::<IrreversibilityClass>(&text).unwrap(),
                class
            );
        }
        for verdict in [
            CorrigibilityVerdict::Allow,
            CorrigibilityVerdict::RequireConfirmation,
        ] {
            let text = serde_json::to_string(&verdict).unwrap();
            assert_eq!(
                serde_json::from_str::<CorrigibilityVerdict>(&text).unwrap(),
                verdict
            );
        }
        assert_eq!(
            serde_json::to_string(&CorrigibilityVerdict::RequireConfirmation).unwrap(),
            "\"require_confirmation\""
        );
        let ledger = TrustLedger {
            action_class: "file.write".into(),
            clean_outcomes: 7,
            last_failure_evaluation: Some("eval-3".into()),
        };
        let text = serde_json::to_string(&ledger).unwrap();
        assert_eq!(serde_json::from_str::<TrustLedger>(&text).unwrap(), ledger);
        let fresh: TrustLedger =
            serde_json::from_str(r#"{"action_class":"a","clean_outcomes":0}"#).unwrap();
        assert_eq!(fresh.last_failure_evaluation, None);
        assert_eq!(
            serde_json::to_string(&crate::decision::QuestionKind::AutonomyVerdict).unwrap(),
            "\"autonomy_verdict\""
        );
    }
}
