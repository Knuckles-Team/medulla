//! Harness edit lifecycle (MEDULLA-DECISION-ENGINE-R173.7).
//!
//! The engine owns the `core:editStatus` of a harness edit: a caller names an event and
//! the engine applies it with `EditStatus::apply` (`EditMachine`). The tenant of an edit
//! is the verified request tenant, never a request field.
use medulla_types::edit_machine::{EditEvent, EditStatus};
use medulla_types::refusal_code::RefusalCode;
use serde::{Deserialize, Serialize};

/// Longest `edit_id`, in characters.
pub const MAX_EDIT_ID_CHARS: usize = 256;

/// The edit is absent, is not a harness edit, or belongs to another tenant. One envelope
/// for all three, so a caller learns nothing about another tenant's edits.
pub const HARNESS_EDIT_UNKNOWN: RefusalCode = RefusalCode::new("HARNESS_EDIT_UNKNOWN");
/// The stored `editStatus` is not one of the three words of `EditMachine`.
pub const HARNESS_EDIT_STATUS_INVALID: RefusalCode =
    RefusalCode::new("HARNESS_EDIT_STATUS_INVALID");

/// A `Create` names an edit node that already exists (MEDULLA-DECISION-ENGINE-R173.8).
pub const HARNESS_EDIT_EXISTS: RefusalCode = RefusalCode::new("HARNESS_EDIT_EXISTS");

/// The refusal codes of `HarnessEdit`, besides `TRANSITION_REFUSED`.
pub const REFUSALS: &[RefusalCode] = &[
    HARNESS_EDIT_UNKNOWN,
    HARNESS_EDIT_STATUS_INVALID,
    HARNESS_EDIT_EXISTS,
];

/// One harness edit and its recorded status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HarnessEditRecord {
    pub edit_id: String,
    pub status: EditStatus,
}

/// `Method::HarnessEdit` operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum HarnessEditOp {
    /// Create the edit as `proposed`, targeting one dimension: the initial state comes from
    /// `EditMachine`, never from the caller (MEDULLA-DECISION-ENGINE-R173.8).
    Create {
        edit_id: String,
        dimension_iri: String,
        round: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        policy_revision: Option<u64>,
    },
    /// Apply one event to the edit.
    Transition { edit_id: String, event: EditEvent },
    /// Read the edit's recorded status.
    Get { edit_id: String },
}

impl HarnessEditOp {
    /// Whether this operation changes durable state.
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Create { .. } | Self::Transition { .. })
    }

    /// The edit the operation names.
    pub fn edit_id(&self) -> &str {
        match self {
            Self::Create { edit_id, .. }
            | Self::Transition { edit_id, .. }
            | Self::Get { edit_id } => edit_id,
        }
    }

    /// The edit id is 1 to 256 characters; a `Create` also names a dimension.
    pub fn check(&self) -> Result<(), String> {
        let length = self.edit_id().chars().count();
        if length == 0 || length > MAX_EDIT_ID_CHARS {
            return Err(format!(
                "edit_id must be 1 to {MAX_EDIT_ID_CHARS} characters, got {length}"
            ));
        }
        if let Self::Create { dimension_iri, .. } = self {
            if dimension_iri.is_empty() {
                return Err("dimension_iri must not be empty".to_string());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R173.7
    #[test]
    fn ops_round_trip_and_only_a_transition_mutates() {
        let transition = HarnessEditOp::Transition {
            edit_id: "edit:1".into(),
            event: EditEvent::Ship,
        };
        let wire = serde_json::to_value(&transition).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"op": "transition", "edit_id": "edit:1", "event": "ship"})
        );
        assert_eq!(
            serde_json::from_value::<HarnessEditOp>(wire).unwrap(),
            transition
        );
        assert!(transition.is_mutation());
        let create = HarnessEditOp::Create {
            edit_id: "edit:1".into(),
            dimension_iri: "dim:tokens".into(),
            round: 2,
            policy_revision: None,
        };
        assert!(create.is_mutation() && create.check().is_ok());
        assert!(HarnessEditOp::Create {
            edit_id: "edit:1".into(),
            dimension_iri: String::new(),
            round: 0,
            policy_revision: None,
        }
        .check()
        .is_err());
        let get = HarnessEditOp::Get {
            edit_id: "edit:1".into(),
        };
        assert!(!get.is_mutation());
        assert!(get.check().is_ok());
        assert!(HarnessEditOp::Get {
            edit_id: String::new()
        }
        .check()
        .is_err());
        assert!(serde_json::from_value::<HarnessEditOp>(
            serde_json::json!({"op": "get", "edit_id": "e", "tenant_id": "t"})
        )
        .is_err());
    }
}
