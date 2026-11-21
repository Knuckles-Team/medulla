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

/// The refusal codes of `HarnessEdit`, besides `TRANSITION_REFUSED`.
pub const REFUSALS: &[RefusalCode] = &[HARNESS_EDIT_UNKNOWN, HARNESS_EDIT_STATUS_INVALID];

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
    /// Apply one event to the edit: the only mutation.
    Transition { edit_id: String, event: EditEvent },
    /// Read the edit's recorded status.
    Get { edit_id: String },
}

impl HarnessEditOp {
    /// Whether this operation changes durable state.
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Transition { .. })
    }

    /// The edit the operation names.
    pub fn edit_id(&self) -> &str {
        match self {
            Self::Transition { edit_id, .. } | Self::Get { edit_id } => edit_id,
        }
    }

    /// The edit id is 1 to 256 characters.
    pub fn check(&self) -> Result<(), String> {
        let length = self.edit_id().chars().count();
        if length == 0 || length > MAX_EDIT_ID_CHARS {
            return Err(format!(
                "edit_id must be 1 to {MAX_EDIT_ID_CHARS} characters, got {length}"
            ));
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
