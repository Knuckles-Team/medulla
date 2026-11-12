//! Write-back preflight and proposal wire types (MEDULLA-DECISION-ENGINE-R615.2).
//!
//! The `DecisionLog` `Writeback` operation computes, without a durable write,
//! whether one proposal is refused, queued for approval or allowed to proceed,
//! and which creation and ticket proposals a reconciled inventory and a set of
//! risk findings produce. The rules live in
//! `medulla_decision_stats::decision::writeback`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The risk tier a write-back proposal is triaged into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RiskTier {
    Low,
    Medium,
    High,
}

impl RiskTier {
    /// The tier's ordinal: 0 low, 1 medium, 2 high (the `writeback.queue_min_tier` scale).
    pub fn ordinal(self) -> u64 {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
        }
    }
}

/// One proposal to preflight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProposalInput {
    pub id: String,
    pub external_id: String,
    pub system_of_record: String,
    pub risk_tier: RiskTier,
    pub as_of_ms: i64,
}

/// One reconciled inventory item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct InventoryItemInput {
    pub external_id: String,
    /// The systems of record that already hold the external id.
    #[serde(default)]
    pub present_in: Vec<String>,
    pub as_of_ms: i64,
}

/// One risk finding to raise a ticket for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RiskFindingInput {
    pub finding_id: String,
    pub external_id: String,
    pub system_of_record: String,
    pub risk_tier: RiskTier,
    pub as_of_ms: i64,
}

/// One write-back computation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum WritebackOp {
    /// Refuse, queue or pass one proposal. `source_values` maps each source
    /// id to a value digest for the proposal's external id.
    Preflight {
        proposal: ProposalInput,
        #[serde(default)]
        source_values: BTreeMap<String, String>,
    },
    /// Creation proposals for the systems lacking an inventory item, and one
    /// ticket proposal per risk finding.
    Propose {
        #[serde(default)]
        inventory: Vec<InventoryItemInput>,
        #[serde(default)]
        enabled_systems: Vec<String>,
        #[serde(default)]
        findings: Vec<RiskFindingInput>,
    },
}

/// The preflight verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PreflightVerdict {
    Proceed,
    Queue,
    Refuse { code: String, detail: String },
}

/// Which proposal family a row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ProposalKind {
    InventoryCreation,
    FindingTicket,
}

/// One proposal produced by a `Propose` computation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProposalRow {
    pub kind: ProposalKind,
    pub id: String,
    pub external_id: String,
    pub system_of_record: String,
    /// Set for a ticket; an inventory creation carries no tier.
    #[serde(default)]
    pub risk_tier: Option<RiskTier>,
    /// The as-of instant of the source the proposal derives from.
    pub as_of_ms: i64,
}

/// The result of one `Writeback` computation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum WritebackOutcome {
    Preflight { verdict: PreflightVerdict },
    Proposals { proposals: Vec<ProposalRow> },
}
