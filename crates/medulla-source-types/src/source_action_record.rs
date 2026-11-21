//! The typed items of an `action_records` batch (MEDULLA-DECISION-ENGINE-R869.2).
//!
//! An action record is append-only provenance: who acted, what kind of action,
//! which facts (pinned by node, graph version and digest) and rules it used, and
//! which approval it carried. Tenant identity is not a field; the server derives it.

use medulla_types::contract::BoundedVec;
use serde::{Deserialize, Serialize};

/// Most facts one action record may cite.
pub const MAX_USED_FACTS: usize = 64;
/// Most rules one action record may cite.
pub const MAX_USED_RULES: usize = 16;

/// The closed kind of an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ActionKind {
    /// A tool call.
    ToolCall,
    /// An approval decision.
    Approval,
    /// A filing.
    Filing,
    /// A movement of money.
    MoneyMovement,
    /// A publication.
    Publish,
}

impl ActionKind {
    /// The stored text of the kind: the `:actionKind` value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ToolCall => "tool_call",
            Self::Approval => "approval",
            Self::Filing => "filing",
            Self::MoneyMovement => "money_movement",
            Self::Publish => "publish",
        }
    }

    /// Whether the kind needs an approved gate decision.
    pub const fn needs_approval(self) -> bool {
        matches!(self, Self::MoneyMovement | Self::Filing | Self::Publish)
    }
}

/// One fact an action read: the node, the graph version read and the digest of
/// the node's property blob at that version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FactUseItem {
    /// The node id of the fact.
    pub node_id: String,
    /// The graph version the action read the node at.
    pub revision: u64,
    /// `sha256:` plus 64 lowercase hex of the node's property blob at `revision`.
    pub content_digest: String,
}

/// One rule an action applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RuleUseItem {
    /// The node id of the rule.
    pub rule_id: String,
    /// The rule's version.
    pub version: u64,
    /// `sha256:` plus 64 lowercase hex.
    pub digest: String,
}

/// One action record of an `action_records` batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ActionRecordItem {
    /// The caller's idempotency key; the record's identity derives from it.
    pub idempotency_key: String,
    /// The kind of action.
    pub action_kind: ActionKind,
    /// The acting principal's node id.
    pub actor: String,
    /// The facts the action read.
    #[serde(default)]
    pub used_facts: BoundedVec<FactUseItem, MAX_USED_FACTS>,
    /// The rules the action applied.
    #[serde(default)]
    pub used_rules: BoundedVec<RuleUseItem, MAX_USED_RULES>,
    /// The gate decision node that approved the action.
    #[serde(default)]
    pub approved_by: Option<String>,
    /// The earlier action record this one corrects or follows from.
    #[serde(default)]
    pub caused_by: Option<String>,
    /// The caller's outcome text.
    #[serde(default)]
    pub outcome: Option<String>,
    /// The stated reason the record used no fact.
    #[serde(default)]
    pub no_inputs: Option<String>,
}
