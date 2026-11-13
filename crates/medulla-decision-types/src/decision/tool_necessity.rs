//! Tool necessity records (MEDULLA-DECISION-ENGINE-R165.1).
//!
//! How much a tool matters to a model's success is a Q32 difference between
//! runs with and without it. The records here carry that difference, compare it
//! with what the model did and name the mismatches.

use serde::{Deserialize, Serialize};

use medulla_types::contract::Digest256;

/// Format identity of [`ToolNecessity`].
pub const TOOL_NECESSITY_SCHEMA_VERSION: u16 = 1;
/// `2^32`, the bound of a Q32 difference.
pub const Q32_ONE: i64 = 1 << 32;

/// The measured necessity of one tool for one model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ToolNecessity {
    pub schema_version: u16,
    pub tool_id: String,
    pub model_id: String,
    pub necessity_q32: i64,
    pub samples_with: u32,
    pub samples_without: u32,
    pub newest_run_ms: u64,
    pub provenance_digest: Digest256,
}

impl ToolNecessity {
    /// `necessity_q32` lies in `[-2^32, 2^32]`.
    pub fn checked(self) -> Result<Self, String> {
        if !(-Q32_ONE..=Q32_ONE).contains(&self.necessity_q32) {
            return Err("necessity_q32 is outside [-2^32, 2^32]".into());
        }
        Ok(self)
    }
}

/// A tool's necessity against what the model did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ToolComparison {
    pub tool_id: String,
    pub necessity_q32: i64,
    pub model_called: bool,
    pub agreement: bool,
}

/// Why a trace disagrees with the measured necessity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum MismatchReason {
    MissedTool,
    UnneededTool,
}

/// One disagreement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TraceMismatch {
    pub tool_id: String,
    pub necessity_q32: i64,
    pub model_call_id: String,
    pub reason: MismatchReason,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn necessity(value: i64) -> ToolNecessity {
        ToolNecessity {
            schema_version: TOOL_NECESSITY_SCHEMA_VERSION,
            tool_id: "tool:search".into(),
            model_id: "model:m".into(),
            necessity_q32: value,
            samples_with: 30,
            samples_without: 28,
            newest_run_ms: 1_700_000_000_000,
            provenance_digest: Digest256::sha256(b"runs"),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R165.1
    #[test]
    fn necessity_types_round_trip() {
        let record = necessity(Q32_ONE / 2).checked().unwrap();
        let text = serde_json::to_string(&record).unwrap();
        assert_eq!(
            serde_json::from_str::<ToolNecessity>(&text).unwrap(),
            record
        );
        let comparison = ToolComparison {
            tool_id: "tool:search".into(),
            necessity_q32: record.necessity_q32,
            model_called: false,
            agreement: false,
        };
        let text = serde_json::to_string(&comparison).unwrap();
        assert_eq!(
            serde_json::from_str::<ToolComparison>(&text).unwrap(),
            comparison
        );
        let mismatch = TraceMismatch {
            tool_id: "tool:search".into(),
            necessity_q32: record.necessity_q32,
            model_call_id: "call:1".into(),
            reason: MismatchReason::MissedTool,
        };
        let text = serde_json::to_string(&mismatch).unwrap();
        assert!(text.contains("\"reason\":\"missed_tool\""));
        assert_eq!(
            serde_json::from_str::<TraceMismatch>(&text).unwrap(),
            mismatch
        );
        assert!(necessity(Q32_ONE).checked().is_ok());
        assert!(necessity(-Q32_ONE).checked().is_ok());
        assert!(necessity(Q32_ONE + 1).checked().is_err());
        assert!(necessity(-Q32_ONE - 1).checked().is_err());
    }
}
