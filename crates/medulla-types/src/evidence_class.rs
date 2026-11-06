//! The weakest class of evidence a conclusion or a cited fact rests on.
//!
//! Lives in the bottom crate so both the decision ladder and the wire facts of
//! `ContextFacts` (MEDULLA-DECISION-ENGINE-R152.1) name one type.

use serde::{Deserialize, Serialize};

/// The weakest class of evidence the conclusion rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum EvidenceClass {
    Proof,
    Observation,
    Claim,
}

impl EvidenceClass {
    /// Parse the stored snake_case label, or `None` for an unknown label.
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "proof" => Some(Self::Proof),
            "observation" => Some(Self::Observation),
            "claim" => Some(Self::Claim),
            _ => None,
        }
    }
}
