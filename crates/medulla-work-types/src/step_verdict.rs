//! The verdict of one verification gate (MEDULLA-SCM-R008).

use serde::{Deserialize, Serialize};

/// What one gate decided about one input: it passed or it failed. A gate that did not run has
/// no verdict at all, so a missing verdict is never read as a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum VerificationVerdict {
    /// The gate accepted the input.
    Pass,
    /// The gate rejected the input.
    Fail,
}

impl VerificationVerdict {
    /// The stable wire name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
        }
    }
}
