//! Adjustment records (MEDULLA-DECISION-ENGINE-R158.1).
//!
//! A run's error rate over a window decides whether its model tier and width
//! change. These records carry the window and the adjustment with its reason.

use serde::{Deserialize, Serialize};

use medulla_types::contract::Digest256;

/// The partition of runs one error rate is computed over, with its counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ErrorWindow {
    pub tenant: String,
    pub child_id: String,
    pub operation: String,
    pub runs: u32,
    pub errored_runs: u32,
}

impl ErrorWindow {
    /// Errored runs never exceed runs.
    pub fn checked(self) -> Result<Self, String> {
        if self.errored_runs > self.runs {
            return Err("errored_runs exceeds runs".into());
        }
        Ok(self)
    }
}

/// Why an adjustment was or was not made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum AdjustmentRationale {
    ErrorHigh,
    ErrorLow,
    Unchanged,
    AtTopTier,
    AtHostCeiling,
}

/// A model and width change, with the rationale and its digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ModelWidthAdjustment {
    pub before_model: String,
    pub after_model: String,
    pub before_width: u32,
    pub after_width: u32,
    pub before_budget_tokens: u64,
    pub after_budget_tokens: u64,
    pub rationale: AdjustmentRationale,
    /// SHA-256 of the error rate, the window and the thresholds.
    pub rationale_digest: Digest256,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R158.1
    #[test]
    fn adjustment_types_round_trip() {
        let window = ErrorWindow {
            tenant: "t".into(),
            child_id: "child:1".into(),
            operation: "search".into(),
            runs: 20,
            errored_runs: 7,
        }
        .checked()
        .unwrap();
        let text = serde_json::to_string(&window).unwrap();
        assert_eq!(serde_json::from_str::<ErrorWindow>(&text).unwrap(), window);
        let adjustment = ModelWidthAdjustment {
            before_model: "small".into(),
            after_model: "large".into(),
            before_width: 2,
            after_width: 4,
            before_budget_tokens: 1_000,
            after_budget_tokens: 4_000,
            rationale: AdjustmentRationale::ErrorHigh,
            rationale_digest: Digest256::sha256(text.as_bytes()),
        };
        let text = serde_json::to_string(&adjustment).unwrap();
        assert!(text.contains("\"rationale\":\"error_high\""));
        assert_eq!(
            serde_json::from_str::<ModelWidthAdjustment>(&text).unwrap(),
            adjustment
        );
        for rationale in [
            AdjustmentRationale::ErrorHigh,
            AdjustmentRationale::ErrorLow,
            AdjustmentRationale::Unchanged,
            AdjustmentRationale::AtTopTier,
            AdjustmentRationale::AtHostCeiling,
        ] {
            let text = serde_json::to_string(&rationale).unwrap();
            assert_eq!(
                serde_json::from_str::<AdjustmentRationale>(&text).unwrap(),
                rationale
            );
        }
        assert!(ErrorWindow {
            errored_runs: 21,
            ..window.clone()
        }
        .checked()
        .is_err());
        let mut value = serde_json::to_value(&window).unwrap();
        value["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<ErrorWindow>(value).is_err());
    }
}
