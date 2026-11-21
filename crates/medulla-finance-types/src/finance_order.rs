//! Order proposal wire types (MEDULLA-FINANCE-PRIMITIVES-R015.1.1 to R015.1.3, R010.2).
//!
//! An [`OrderProposalSnapshot`] is one sealed, content-addressed revision of an
//! order proposal. `Method::FinanceOrderStage` creates it, `Method::FinanceOrderLimitCheck`
//! reads the limit verdict for a proposal, and `Method::FinanceOrderPromote` admits
//! a staged revision for host review with an [`OrderAdmissionReceipt`]. Nothing here
//! is an order or a provider effect.

use serde::{Deserialize, Serialize};

/// The head moved: another revision or digest is current.
pub const PROPOSAL_HEAD_STALE: &str = "PROPOSAL_HEAD_STALE";
/// Other bytes already exist at that revision.
pub const PROPOSAL_REVISION_CONFLICT: &str = "PROPOSAL_REVISION_CONFLICT";
/// The payload hash, or canonical form for its declared schema, does not match.
pub const PROPOSAL_HASH_MISMATCH: &str = "PROPOSAL_HASH_MISMATCH";
/// An evidence reference does not resolve to a record of the caller's tenant.
pub const PROPOSAL_EVIDENCE_MISSING: &str = "PROPOSAL_EVIDENCE_MISSING";
/// The order breaches a pinned risk limit.
pub const ORDER_LIMIT_BREACH: &str = "ORDER_LIMIT_BREACH";
/// The tenant has no `TradingPolicy` record at the pinned id.
pub const TRADING_POLICY_MISSING: &str = "TRADING_POLICY_MISSING";
/// A manifest gate has no receipt or the manifest digest is unavailable.
pub const PROMOTE_GATE_UNAVAILABLE: &str = "PROMOTE_GATE_UNAVAILABLE";
/// A gate receipt did not pass.
pub const PROMOTE_GATE_FAILED: &str = "PROMOTE_GATE_FAILED";
/// The expected promoted head is not the stored one.
pub const PROMOTE_HEAD_STALE: &str = "PROMOTE_HEAD_STALE";
/// The staged digest, payload hash, decision or policy does not match the request.
pub const PROMOTE_EVIDENCE_MISMATCH: &str = "PROMOTE_EVIDENCE_MISMATCH";
/// A gate receipt is incomplete or bound to another candidate.
pub const PROMOTE_RECEIPT_INCOMPLETE: &str = "PROMOTE_RECEIPT_INCOMPLETE";
/// A promotion hook vetoed the admission.
pub const ORDER_PROMOTION_VETOED: &str = "ORDER_PROMOTION_VETOED";
/// The one promotion transaction failed and rolled back.
pub const PROMOTE_WRITE_FAILED: &str = "PROMOTE_WRITE_FAILED";

/// Every refusal code of the order proposal methods, for registry checks.
pub const ORDER_REFUSALS: &[&str] = &[
    PROPOSAL_HEAD_STALE,
    PROPOSAL_REVISION_CONFLICT,
    PROPOSAL_HASH_MISMATCH,
    PROPOSAL_EVIDENCE_MISSING,
    ORDER_LIMIT_BREACH,
    TRADING_POLICY_MISSING,
    PROMOTE_GATE_UNAVAILABLE,
    PROMOTE_GATE_FAILED,
    PROMOTE_HEAD_STALE,
    PROMOTE_EVIDENCE_MISMATCH,
    PROMOTE_RECEIPT_INCOMPLETE,
    ORDER_PROMOTION_VETOED,
    PROMOTE_WRITE_FAILED,
];

/// The payload schema id medulla validates itself ([`FinanceOrder`]).
pub const ORDER_PAYLOAD_SCHEMA_ID: &str = "finance.order";
/// The payload schema revision of [`FinanceOrder`].
pub const ORDER_PAYLOAD_SCHEMA_REVISION: &str = "1";

/// The order the limit guard measures: instrument, signed quantity (negative is a
/// sale) and notional, both in integer units of the tenant's risk limits. Its
/// canonical bytes are `serde_json::to_vec` of this struct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FinanceOrder {
    pub instrument_id: String,
    pub signed_quantity: i64,
    pub notional: i64,
}

/// One evidence record a proposal cites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EvidenceRef {
    pub owner_requirement_id: String,
    pub record_id: String,
    pub revision: u64,
    pub digest: String,
}

/// One sealed revision of an order proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OrderProposalSnapshot {
    pub tenant_id: String,
    pub proposal_id: String,
    /// Starts at 1.
    pub revision: u64,
    /// `None` for revision 1; the digest of revision - 1 otherwise.
    #[serde(default)]
    pub predecessor_digest: Option<String>,
    pub decision_id: String,
    pub decision_revision: u64,
    pub decision_digest: String,
    pub payload_schema_id: String,
    pub payload_schema_revision: String,
    pub canonical_payload_bytes: Vec<u8>,
    /// `sha256:<hex>` over `canonical_payload_bytes` as stored.
    pub payload_sha256: String,
    pub policy_id: String,
    pub policy_revision: u64,
    pub policy_digest: String,
    pub evidence_refs: Vec<EvidenceRef>,
}

/// The answer of `FinanceOrderStage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OrderStageResult {
    pub proposal_id: String,
    pub revision: u64,
    pub predecessor_digest: Option<String>,
    pub snapshot_digest: String,
    /// Always `staged`.
    pub state: String,
}

/// One limit breach, as `AbstainReason::LimitBreach` names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LimitBreachReason {
    pub rule_id: String,
    pub observed: i64,
    pub bound: i64,
    pub signal_iri: String,
}

/// The answer of `FinanceOrderLimitCheck`: `Passed`, or `Abstained` with one reason
/// per breach in rule order (MEDULLA-DECISION-ENGINE-R131.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OrderLimitCheckResult {
    /// `Passed` or `Abstained`.
    pub outcome: String,
    pub reasons: Vec<LimitBreachReason>,
}

/// The tenant's `TradingPolicy` record: the leverage and risk policy and the
/// designated approvers. `leverage_limits` is a JSON array of per-instrument
/// leverage policies and `risk_limits` a JSON array of limit rules, typed at the
/// handler by the kernel types that consume them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TradingPolicy {
    pub tenant_id: String,
    pub revision: u64,
    pub leverage_limits: serde_json::Value,
    pub risk_limits: serde_json::Value,
    pub designated_approvers: Vec<String>,
}

/// One gate receipt supplied to a promotion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GateReceipt {
    pub gate_id: String,
    pub gate_revision: u64,
    pub candidate_digest: String,
    pub pass_fail: bool,
    pub evidence_digest: String,
}

/// The request of `FinanceOrderPromote`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OrderPromoteRequest {
    pub tenant_id: String,
    pub proposal_id: String,
    pub candidate_revision: u64,
    pub snapshot_digest: String,
    /// `None` when nothing is promoted yet.
    #[serde(default)]
    pub expected_promoted_revision: Option<u64>,
    /// The digest of the gate manifest the receipts answer.
    pub required_gate_manifest_digest: String,
    /// Every gate id of the manifest, in manifest order.
    pub manifest_gate_ids: Vec<String>,
    pub gate_receipts: Vec<GateReceipt>,
    /// The approver acting on the promotion, checked against the tenant policy. A live order
    /// without one is refused `ORDER_PROMOTION_VETOED`.
    #[serde(default)]
    pub approver: Option<String>,
    /// A hook that vetoed the promotion before this call; refuses `ORDER_PROMOTION_VETOED`.
    #[serde(default)]
    pub veto_reason: Option<String>,
}

/// The sealed record a promotion writes. Host-review admission only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OrderAdmissionReceipt {
    pub proposal_id: String,
    pub revision: u64,
    pub snapshot_digest: String,
    pub decision_digest: String,
    pub policy_id: String,
    pub policy_revision: u64,
    pub policy_digest: String,
    pub gate_manifest_digest: String,
    pub gate_receipt_digests: Vec<String>,
    pub transaction_revision: u64,
}

/// The answer of `FinanceOrderPromote`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OrderPromoteResult {
    pub receipt_id: String,
    pub receipt_digest: String,
    pub receipt: OrderAdmissionReceipt,
}
