//! Refusal codes of the participant memory family
//! (MEDULLA-DECISION-ENGINE-R010.100.4).
//!
//! Collected by `medulla_decision_types::decision::codes::module_tables` and the
//! uniqueness test, so no code here repeats a shared or foreign code.

use crate::refusal_code::RefusalCode;

/// The longest participant slug, in bytes.
pub const MAX_PARTICIPANT_SLUG_BYTES: usize = 64;

/// A slug that is not `[a-z0-9-]{1,64}`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("PARTICIPANT_ID_INVALID: participant id must match [a-z0-9-]{{1,{MAX_PARTICIPANT_SLUG_BYTES}}}: {0:?}")]
pub struct ParticipantIdInvalid(pub String);

/// A participant slug: `[a-z0-9-]{1,64}` (MEDULLA-DECISION-ENGINE-R617.7). It lives
/// here so the source and work types can both name a participant without
/// depending on each other.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ParticipantId(String);

impl ParticipantId {
    /// Validate a slug.
    pub fn new(value: impl Into<String>) -> Result<Self, ParticipantIdInvalid> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= MAX_PARTICIPANT_SLUG_BYTES
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if valid {
            Ok(Self(value))
        } else {
            Err(ParticipantIdInvalid(value))
        }
    }

    /// The slug.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ParticipantId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(serde::de::Error::custom)
    }
}

/// The refusal code of a read that the reader's view excludes
/// (MEDULLA-DECISION-ENGINE-R652.9); it is also an entry of [`REFUSALS`].
pub const PARTICIPANT_NOT_VISIBLE: RefusalCode = RefusalCode::new("PARTICIPANT_NOT_VISIBLE");

/// The refusal code of a reader that asks for a view its principal does not
/// hold; it is also an entry of [`REFUSALS`].
pub const OBSERVER_NOT_PERMITTED: RefusalCode = RefusalCode::new("OBSERVER_NOT_PERMITTED");

/// Every refusal code of the participant memory family.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("PARTICIPANT_KIND_INVALID"),
    RefusalCode::new("MEMBERSHIP_SESSION_UNKNOWN"),
    RefusalCode::new("PARTICIPANT_NOT_VISIBLE"),
    RefusalCode::new("OBSERVER_NOT_PERMITTED"),
    RefusalCode::new("SCOPE_NOT_FOUND"),
    RefusalCode::new("SCOPE_NOT_VISIBLE"),
    RefusalCode::new("SCOPE_SESSION_LIMIT"),
    RefusalCode::new("NO_GROUNDED_CLAIMS"),
    RefusalCode::new("CARD_ABSENT"),
    RefusalCode::new("DERIVE_BACKLOG_FULL"),
    RefusalCode::new("DERIVE_ORDER_HELD"),
    RefusalCode::new("PROFILE_NOT_FOUND"),
    RefusalCode::new("PROFILE_NOT_VISIBLE"),
    RefusalCode::new("PROFILE_REVISION_CONFLICT"),
    RefusalCode::new("PROFILE_TOOL_NOT_ALLOWED"),
    RefusalCode::new("DOCUMENT_SET_NOT_FOUND"),
    RefusalCode::new("DOCUMENT_SET_QUERY_NOT_READ_ONLY"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_names_every_participant_code_once() {
        assert_eq!(REFUSALS.len(), 17);
        let mut codes: Vec<&str> = REFUSALS.iter().map(|c| c.as_str()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), 17);
    }
}
