//! The run context of one requirement row (MEDULLA-SDLC-R015.2, R015.3).
//!
//! The engine assembles the context behind the caller's read ACL and seals it as the body
//! of the `TaskPlan` record it answers, so a dispatched run receives it and the decision
//! log holds it.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One section of a run context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContextSection {
    pub node_id: String,
    pub title: String,
    pub text: String,
}

/// The run context of one requirement row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TaskContext {
    pub row_id: String,
    pub row_text: String,
    pub plan_sections: Vec<ContextSection>,
    pub contracts: Vec<ContextSection>,
    pub data_model: Option<ContextSection>,
    pub neighbourhood: Vec<String>,
    pub truncated: bool,
}

impl TaskContext {
    /// The lowercase hex SHA-256 of the context's canonical JSON.
    pub fn digest(&self) -> String {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        hex::encode(Sha256::digest(bytes))
    }
}
