//! Delegation records for background work (MEDULLA-IDENTITY-R701): one record
//! per WorkItem naming the tenant, the principal the work runs for, the tool
//! scope it may use, and when the grant expires or was revoked.

use serde::{Deserialize, Serialize};

/// The stored delegation of one WorkItem. Times are engine milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DelegationRecord {
    pub tenant: String,
    pub principal: String,
    pub tool: String,
    pub expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<u64>,
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
/// principal is active and holds the tool scope right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DelegationState {
    pub record: DelegationRecord,
    pub standing: DelegationStanding,
    pub principal_active: bool,
    pub holds_tool_scope: bool,
}
