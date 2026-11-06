//! Append-only ontology edit chain records (MEDULLA-TYPED-PACKS-R651.1).
//!
//! One [`OntologyEdit`] names one change to one ontology subject. Each edit
//! links to the SHA-256 of the previous edit, so the chain is tamper-evident.
//! The records are pure data; the chain rules live in `medulla-core` and the
//! commit in the `GraphSchema` handler.

use crate::contract::Digest256;
use serde::{Deserialize, Serialize};

/// What one edit did to its subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditKind {
    /// The subject came into being.
    Add,
    /// The subject's content changed.
    Change,
    /// The subject was withdrawn.
    Remove,
}

impl EditKind {
    /// The kind an inversion of this edit carries: `add` and `remove` swap.
    pub fn inverse(self) -> Self {
        match self {
            Self::Add => Self::Remove,
            Self::Change => Self::Change,
            Self::Remove => Self::Add,
        }
    }
}

/// One link of the ontology edit chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OntologyEdit {
    /// Position-ordered identifier, unique inside one chain.
    pub edit_id: String,
    /// What the edit did.
    pub kind: EditKind,
    /// The ontology subject the edit changed.
    pub subject_iri: String,
    /// Content digest before the edit; absent for an `add`.
    pub before_digest: Option<Digest256>,
    /// Content digest after the edit; absent for a `remove`.
    pub after_digest: Option<Digest256>,
    /// SHA-256 of the previous edit; absent for the first edit of a chain.
    pub prev_digest: Option<Digest256>,
    /// The consumed governed change that sealed the edit, if any.
    pub sealed_by: Option<String>,
}

/// How serious one naming finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamingSeverity {
    /// Reported, the write goes on.
    Warn,
    /// Refuses the write.
    Error,
}

/// One naming-convention finding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamingFinding {
    /// The class or property the finding is about.
    pub subject_iri: String,
    /// The convention the name breaks.
    pub rule: String,
    /// Warn or error.
    pub severity: NamingSeverity,
}

/// The `ontology.naming_mode` knob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamingMode {
    /// Findings are reported.
    Warn,
    /// Findings refuse the write.
    Refuse,
}

impl NamingMode {
    /// The mode a knob value selects: `0` is `warn`, anything else `refuse`.
    pub fn from_knob(value: u64) -> Self {
        if value == 0 {
            Self::Warn
        } else {
            Self::Refuse
        }
    }

    /// The severity a finding carries under this mode.
    pub fn severity(self) -> NamingSeverity {
        match self {
            Self::Warn => NamingSeverity::Warn,
            Self::Refuse => NamingSeverity::Error,
        }
    }
}
