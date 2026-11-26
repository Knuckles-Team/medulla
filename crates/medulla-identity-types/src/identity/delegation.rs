//! Delegation records for background work (MEDULLA-IDENTITY-R701): one record
//! per WorkItem naming the tenant, the principal the work runs for, the tool
//! scope it may use, and when the grant expires or was revoked. A second kind,
//! an agent run, names the run and the set of operations it may perform for
//! the principal (MEDULLA-IDENTITY-R742).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Most operations one agent-run delegation may name.
pub const MAX_DELEGATION_OPERATIONS: usize = 64;

/// What a delegation record authorizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DelegationKind {
    /// One tool scope for one WorkItem; the record id is the WorkItem id.
    #[default]
    ToolTask,
    /// An agent run on behalf of the principal, scoped to a set of operations; the record id is
    /// the delegation id a WorkItem names in its `delegation_id` metadata.
    AgentRun,
}

impl DelegationKind {
    fn is_tool_task(&self) -> bool {
        matches!(self, Self::ToolTask)
    }
}

/// The stored delegation of one WorkItem or agent run. Times are engine milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DelegationRecord {
    pub tenant: String,
    pub principal: String,
    /// The tool scope of a `tool_task`; empty for an `agent_run`.
    pub tool: String,
    pub expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<u64>,
    #[serde(default, skip_serializing_if = "DelegationKind::is_tool_task")]
    pub kind: DelegationKind,
    /// The run an `agent_run` delegation is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The operations (scopes) an `agent_run` may perform for the principal.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub operations: BTreeSet<String>,
}

/// Write one WorkItem's delegation record (`identity:admin`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DelegationRecordWrite {
    pub work_item_id: String,
    pub record: DelegationRecord,
}

/// Where a delegation stands at the engine's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DelegationStanding {
    Active,
    Expired,
    Revoked,
}

/// The served read of a delegation record: its current state, and whether the
/// principal is active and holds the tool scope right now (for an agent run: every
/// operation of the set).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DelegationState {
    pub record: DelegationRecord,
    pub standing: DelegationStanding,
    pub principal_active: bool,
    pub holds_tool_scope: bool,
}
