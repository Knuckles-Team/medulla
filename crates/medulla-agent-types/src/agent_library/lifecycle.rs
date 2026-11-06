//! The lifecycle one Agent Library revision is retained under, and the one
//! rule the three layers above components enforce about it.

use serde::{Deserialize, Serialize};

/// The retained lifecycle of one Agent Library definition.
///
/// `Retired` is a durable tombstone.  It remains in the revision stream and
/// cannot be replaced by a later publish, preserving the definition's
/// provenance and preventing silent resurrection of an agent identity.
///
/// `Withdrawn` is the reversible counterpart, and it exists only for connector
/// pack members: a pack that stops serving an entry has not decided the entry
/// was wrong, only that it is no longer offered, and a later import that
/// offers it again must be able to republish it. Collapsing the two would make
/// every transient publisher outage a permanent tombstone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum AgentLibraryLifecycle {
    Published,
    Retired,
    Withdrawn,
}

/// Refuse [`AgentLibraryLifecycle::Withdrawn`] on a layer that has no
/// withdrawal authority.
///
/// Withdrawal belongs to a connector pack importer saying an entry is no
/// longer served. Agents, graphs and templates have no importer, so the state
/// is unreachable for them and is refused by name rather than left to surface
/// later as an unexplained digest or routing mismatch.
pub fn refuse_withdrawn(layer: &str, lifecycle: AgentLibraryLifecycle) -> Result<(), String> {
    match lifecycle {
        AgentLibraryLifecycle::Published | AgentLibraryLifecycle::Retired => Ok(()),
        AgentLibraryLifecycle::Withdrawn => Err(format!(
            "WITHDRAWN_NOT_ALLOWED: {layer} entries have no withdrawal lifecycle"
        )),
    }
}

/// A lifecycle change outside the allowed transition table
/// (MEDULLA-DECISION-ENGINE-R168.4).
pub const LIFECYCLE_TRANSITION_INVALID: &str = "LIFECYCLE_TRANSITION_INVALID";
/// Any change away from `Retired`, which is terminal (MEDULLA-DECISION-ENGINE-R168.4).
pub const LIFECYCLE_RETIRED_TERMINAL: &str = "LIFECYCLE_RETIRED_TERMINAL";
/// A stored or submitted lifecycle record that fails validation
/// (MEDULLA-DECISION-ENGINE-R168.4).
pub const LIFECYCLE_STATE_MALFORMED: &str = "LIFECYCLE_STATE_MALFORMED";
/// Withdrawal on a layer or member without withdrawal authority.
pub const WITHDRAWN_NOT_ALLOWED: &str = "WITHDRAWN_NOT_ALLOWED";

/// The lifecycle family's stable refusal codes (MEDULLA-DECISION-ENGINE-R168.4).
pub const REFUSALS: &[&str] = &[
    LIFECYCLE_TRANSITION_INVALID,
    LIFECYCLE_RETIRED_TERMINAL,
    LIFECYCLE_STATE_MALFORMED,
    WITHDRAWN_NOT_ALLOWED,
];
