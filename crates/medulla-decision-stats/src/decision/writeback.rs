//! Write-back proposal records: the approval queue and the inventory and
//! ticket proposals it releases (MEDULLA-DECISION-ENGINE-R615.1).
//!
//! The model layer of the write-back family: every proposal is typed,
//! carries the risk tier it was triaged into and the as-of instant it
//! derives from, and is deduplicated by its external id. The queue/approve/
//! execute behaviour (R615.2), the vendor execution wiring (R615.3) and the
//! `PROPOSAL_UNAPPROVED` refusal (R615.4) build on these records; nothing
//! here executes a vendor call or reads a clock.

use medulla_types::contract::{ResourceId, TenantId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The stable code of a refused write-back identifier.
pub const INVALID_WRITEBACK_IDENTIFIER: &str = "INVALID_WRITEBACK_IDENTIFIER";
/// The stable code of a refused as-of instant.
pub const INVALID_WRITEBACK_AS_OF: &str = "INVALID_WRITEBACK_AS_OF";
/// The stable code of a queue holding two proposals with one external id.
pub const WRITEBACK_DUPLICATE_EXTERNAL_ID: &str = "WRITEBACK_DUPLICATE_EXTERNAL_ID";
/// The stable code of a queue holding another tenant's proposal.
pub const WRITEBACK_TENANT_MISMATCH: &str = "WRITEBACK_TENANT_MISMATCH";
/// The stable code of a proposal whose sources disagree on its value.
pub const WRITEBACK_SOURCE_CONFLICT: &str = "WRITEBACK_SOURCE_CONFLICT";

/// A refused write-back model value, with its stable code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritebackError {
    pub code: &'static str,
    pub detail: String,
}

impl WritebackError {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for WritebackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for WritebackError {}

fn resource_id<T: Into<String>>(value: T, field: &str) -> Result<ResourceId, WritebackError> {
    ResourceId::new(value).map_err(|detail| {
        WritebackError::new(INVALID_WRITEBACK_IDENTIFIER, format!("{field}: {detail}"))
    })
}

fn tenant_id<T: Into<String>>(value: T, field: &str) -> Result<TenantId, WritebackError> {
    TenantId::new(value).map_err(|detail| {
        WritebackError::new(INVALID_WRITEBACK_IDENTIFIER, format!("{field}: {detail}"))
    })
}

fn as_of(as_of_ms: i64) -> Result<(), WritebackError> {
    if as_of_ms <= 0 {
        return Err(WritebackError::new(
            INVALID_WRITEBACK_AS_OF,
            format!("as-of instant {as_of_ms} must be a positive unix-millisecond instant"),
        ));
    }
    Ok(())
}

pub use medulla_decision_types::decision::statistical::writeback::RiskTier;

/// One high-stakes write-back proposal, stamped with the as-of instant it
/// derives from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WritebackProposal {
    pub id: ResourceId,
    pub tenant: TenantId,
    /// The external id the proposal deduplicates by.
    pub external_id: ResourceId,
    /// The system of record the proposal reaches.
    pub system_of_record: ResourceId,
    pub risk_tier: RiskTier,
    /// Milliseconds since the Unix epoch: the instant the proposal derives from.
    pub as_of_ms: i64,
}

impl WritebackProposal {
    pub fn new(
        id: impl Into<String>,
        tenant: impl Into<String>,
        external_id: impl Into<String>,
        system_of_record: impl Into<String>,
        risk_tier: RiskTier,
        as_of_ms: i64,
    ) -> Result<Self, WritebackError> {
        let id = resource_id(id, "id")?;
        let tenant = tenant_id(tenant, "tenant")?;
        let external_id = resource_id(external_id, "external_id")?;
        let system_of_record = resource_id(system_of_record, "system_of_record")?;
        as_of(as_of_ms)?;
        Ok(Self {
            id,
            tenant,
            external_id,
            system_of_record,
            risk_tier,
            as_of_ms,
        })
    }
}

/// The approval queue: high-stakes proposals previewed, not yet released.
/// Approval releases the queued proposals as one governed change set
/// (R615.2); nothing here executes them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalQueue {
    pub id: ResourceId,
    pub tenant: TenantId,
    /// The queued proposals, in enqueue order.
    pub proposals: Vec<WritebackProposal>,
}

impl ProposalQueue {
    /// A queue never holds two proposals with the same external id, and
    /// never holds another tenant's proposal.
    pub fn new(
        id: impl Into<String>,
        tenant: impl Into<String>,
        proposals: Vec<WritebackProposal>,
    ) -> Result<Self, WritebackError> {
        let id = resource_id(id, "id")?;
        let tenant = tenant_id(tenant, "tenant")?;
        let mut external: BTreeSet<(&ResourceId, &ResourceId)> = BTreeSet::new();
        for proposal in &proposals {
            if proposal.tenant != tenant {
                return Err(WritebackError::new(
                    WRITEBACK_TENANT_MISMATCH,
                    format!(
                        "proposal {} belongs to tenant {}, not {}",
                        proposal.id.as_str(),
                        proposal.tenant.as_str(),
                        tenant.as_str()
                    ),
                ));
            }
            if !external.insert((&proposal.system_of_record, &proposal.external_id)) {
                return Err(WritebackError::new(
                    WRITEBACK_DUPLICATE_EXTERNAL_ID,
                    format!(
                        "external id {} for system {} is queued more than once",
                        proposal.external_id.as_str(),
                        proposal.system_of_record.as_str()
                    ),
                ));
            }
        }
        Ok(Self {
            id,
            tenant,
            proposals,
        })
    }
}

/// The stable code of an approval-queue transition the machine refuses.
pub const WRITEBACK_QUEUE_TRANSITION_REFUSED: &str = "WRITEBACK_QUEUE_TRANSITION_REFUSED";
/// The stable code of a release before every proposal holds an approval.
pub const WRITEBACK_NOT_FULLY_APPROVED: &str = "WRITEBACK_NOT_FULLY_APPROVED";
/// The stable code of a queue the tenant does not hold.
pub const WRITEBACK_QUEUE_UNKNOWN: &str = "WRITEBACK_QUEUE_UNKNOWN";
/// The stable code of a queue id that is already stored.
pub const WRITEBACK_QUEUE_EXISTS: &str = "WRITEBACK_QUEUE_EXISTS";
/// The stable code of an approval naming a proposal the queue does not hold.
pub const WRITEBACK_PROPOSAL_UNKNOWN: &str = "WRITEBACK_PROPOSAL_UNKNOWN";
/// The stable code of a proposal whose tier does not need approval.
pub const WRITEBACK_BELOW_QUEUE_TIER: &str = "WRITEBACK_BELOW_QUEUE_TIER";
/// The stable code of a queue beyond `writeback.queue_max_proposals`.
pub const WRITEBACK_QUEUE_TOO_LARGE: &str = "WRITEBACK_QUEUE_TOO_LARGE";

/// The states of one approval queue: the machine `proposal_queue`. The state
/// is persisted as this one field; `Released` is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueState {
    /// Stored, waiting for approvals.
    Pending,
    /// Released as one governed change set.
    Released,
}

/// The closed set of events of the machine `proposal_queue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueEvent {
    /// A proposal of the queue was approved.
    Approve,
    /// A release was requested; `fully_approved` says every proposal holds an
    /// approval.
    Release { fully_approved: bool },
}

impl QueueState {
    fn name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Released => "released",
        }
    }
}

impl QueueEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Release { .. } => "release",
        }
    }
}

/// The one transition function of `proposal_queue`, and the only writer of
/// [`QueueState`]. Approving keeps `Pending`; releasing a fully approved
/// pending queue reaches `Released`; releasing a released queue is an
/// idempotent replay; approving a released queue and releasing an unapproved
/// one are refused, naming the machine, the state and the event.
pub fn queue_transition(
    state: QueueState,
    event: QueueEvent,
) -> Result<QueueState, WritebackError> {
    match (state, event) {
        (QueueState::Pending, QueueEvent::Approve) => Ok(QueueState::Pending),
        (
            QueueState::Pending,
            QueueEvent::Release {
                fully_approved: true,
            },
        ) => Ok(QueueState::Released),
        (
            QueueState::Pending,
            QueueEvent::Release {
                fully_approved: false,
            },
        ) => Err(WritebackError::new(
            WRITEBACK_NOT_FULLY_APPROVED,
            "machine proposal_queue, state pending, event release: a queued proposal \
                 holds no approval yet",
        )),
        (QueueState::Released, QueueEvent::Release { .. }) => Ok(QueueState::Released),
        (QueueState::Released, QueueEvent::Approve) => Err(WritebackError::new(
            WRITEBACK_QUEUE_TRANSITION_REFUSED,
            format!(
                "machine proposal_queue, state {}, event {}: a released queue takes no \
                 further approval",
                state.name(),
                event.name()
            ),
        )),
    }
}

impl WritebackError {
    /// A refusal of one of this module's stable codes, for the served handler.
    pub fn refusal(code: &'static str, detail: impl Into<String>) -> Self {
        Self::new(code, detail)
    }
}

/// Reconciled inventory becoming a creation proposal for one enabled system
/// of record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryCreation {
    pub id: ResourceId,
    pub tenant: TenantId,
    /// The external id the creation deduplicates by.
    pub external_id: ResourceId,
    /// The system of record the creation reaches.
    pub system_of_record: ResourceId,
    /// Milliseconds since the Unix epoch: the instant the creation derives from.
    pub as_of_ms: i64,
}

impl InventoryCreation {
    pub fn new(
        id: impl Into<String>,
        tenant: impl Into<String>,
        external_id: impl Into<String>,
        system_of_record: impl Into<String>,
        as_of_ms: i64,
    ) -> Result<Self, WritebackError> {
        let id = resource_id(id, "id")?;
        let tenant = tenant_id(tenant, "tenant")?;
        let external_id = resource_id(external_id, "external_id")?;
        let system_of_record = resource_id(system_of_record, "system_of_record")?;
        as_of(as_of_ms)?;
        Ok(Self {
            id,
            tenant,
            external_id,
            system_of_record,
            as_of_ms,
        })
    }
}

/// A risk finding becoming a ticket proposal, stamped with the as-of
/// instant it derives from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingTicket {
    pub id: ResourceId,
    pub tenant: TenantId,
    /// The external id the ticket deduplicates by.
    pub external_id: ResourceId,
    /// The risk finding the ticket derives from.
    pub finding_id: ResourceId,
    /// The ticketing system of record the ticket reaches.
    pub system_of_record: ResourceId,
    pub risk_tier: RiskTier,
    /// Milliseconds since the Unix epoch: the instant the ticket derives from.
    pub as_of_ms: i64,
}

impl FindingTicket {
    pub fn new(
        id: impl Into<String>,
        tenant: impl Into<String>,
        external_id: impl Into<String>,
        finding_id: impl Into<String>,
        system_of_record: impl Into<String>,
        risk_tier: RiskTier,
        as_of_ms: i64,
    ) -> Result<Self, WritebackError> {
        let id = resource_id(id, "id")?;
        let tenant = tenant_id(tenant, "tenant")?;
        let external_id = resource_id(external_id, "external_id")?;
        let finding_id = resource_id(finding_id, "finding_id")?;
        let system_of_record = resource_id(system_of_record, "system_of_record")?;
        as_of(as_of_ms)?;
        Ok(Self {
            id,
            tenant,
            external_id,
            finding_id,
            system_of_record,
            risk_tier,
            as_of_ms,
        })
    }
}

/// What the preflight decided for one proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preflight {
    /// Two or more sources disagree: refused with [`WRITEBACK_SOURCE_CONFLICT`].
    Refuse(WritebackError),
    /// The tier is at or above `writeback.queue_min_tier`: wait for approval.
    Queue,
    /// Every other proposal proceeds.
    Proceed,
}

/// Preflight one proposal. `source_values` maps each source id to a value
/// digest for the proposal's external id; two or more distinct digests refuse.
/// `queue_min_tier` is the `writeback.queue_min_tier` knob (0 low, 1 medium,
/// 2 high).
pub fn preflight(
    proposal: &WritebackProposal,
    source_values: &BTreeMap<String, String>,
    queue_min_tier: u64,
) -> Preflight {
    let distinct: BTreeSet<&String> = source_values.values().collect();
    if distinct.len() >= 2 {
        return Preflight::Refuse(WritebackError::new(
            WRITEBACK_SOURCE_CONFLICT,
            format!(
                "{} sources hold {} different values for external id {}",
                source_values.len(),
                distinct.len(),
                proposal.external_id.as_str()
            ),
        ));
    }
    if proposal.risk_tier.ordinal() >= queue_min_tier {
        Preflight::Queue
    } else {
        Preflight::Proceed
    }
}

/// One reconciled inventory item: the systems of record that already hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryItem {
    pub tenant: TenantId,
    pub external_id: ResourceId,
    pub present_in: BTreeSet<ResourceId>,
    pub as_of_ms: i64,
}

/// One risk finding to raise a ticket for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskFinding {
    pub tenant: TenantId,
    pub finding_id: ResourceId,
    pub external_id: ResourceId,
    pub system_of_record: ResourceId,
    pub risk_tier: RiskTier,
    pub as_of_ms: i64,
}

/// One `InventoryCreation` per reconciled item per enabled system that lacks
/// its external id, copying the item's as-of instant, sorted by
/// `(system_of_record, external_id)`.
pub fn creation_proposals(
    inventory: &[InventoryItem],
    enabled_systems: &[ResourceId],
) -> Result<Vec<InventoryCreation>, WritebackError> {
    let mut out: BTreeMap<(ResourceId, ResourceId), InventoryCreation> = BTreeMap::new();
    for item in inventory {
        for system in enabled_systems {
            if item.present_in.contains(system) {
                continue;
            }
            let key = (system.clone(), item.external_id.clone());
            if out.contains_key(&key) {
                continue;
            }
            let creation = InventoryCreation::new(
                format!("inv:{}:{}", system.as_str(), item.external_id.as_str()),
                item.tenant.as_str(),
                item.external_id.as_str(),
                system.as_str(),
                item.as_of_ms,
            )?;
            out.insert(key, creation);
        }
    }
    Ok(out.into_values().collect())
}

/// One `FindingTicket` per risk finding, with the finding's tier and as-of
/// instant. Findings that name one `(system_of_record, external_id)` collapse
/// to the highest tier, then the lowest finding id. Sorted by
/// `(system_of_record, external_id)`.
pub fn ticket_proposals(findings: &[RiskFinding]) -> Result<Vec<FindingTicket>, WritebackError> {
    let mut chosen: BTreeMap<(&ResourceId, &ResourceId), &RiskFinding> = BTreeMap::new();
    for finding in findings {
        let key = (&finding.system_of_record, &finding.external_id);
        match chosen.get(&key) {
            Some(held)
                if (held.risk_tier, std::cmp::Reverse(&held.finding_id))
                    >= (finding.risk_tier, std::cmp::Reverse(&finding.finding_id)) => {}
            _ => {
                chosen.insert(key, finding);
            }
        }
    }
    chosen
        .into_values()
        .map(|f| {
            FindingTicket::new(
                format!("ticket:{}", f.finding_id.as_str()),
                f.tenant.as_str(),
                f.external_id.as_str(),
                f.finding_id.as_str(),
                f.system_of_record.as_str(),
                f.risk_tier,
                f.as_of_ms,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(external_id: &str, tenant: &str) -> WritebackProposal {
        WritebackProposal::new(
            "wp:1",
            tenant,
            external_id,
            "sor:ledger",
            RiskTier::High,
            1_700_000_000_000,
        )
        .unwrap()
    }

    // spec: MEDULLA-DECISION-ENGINE-R615.1
    #[test]
    fn writeback_types_round_trip() {
        let queue =
            ProposalQueue::new("queue:1", "tenant-a", vec![proposal("ext:1", "tenant-a")]).unwrap();
        let creation = InventoryCreation::new(
            "inv:1",
            "tenant-a",
            "ext:1",
            "sor:ledger",
            1_700_000_000_000,
        )
        .unwrap();
        let ticket = FindingTicket::new(
            "tick:1",
            "tenant-a",
            "ext:2",
            "finding:1",
            "sor:tickets",
            RiskTier::Medium,
            1_700_000_000_001,
        )
        .unwrap();
        macro_rules! round_trip {
            ($value:expr, $ty:ty) => {{
                let json = serde_json::to_string(&$value).unwrap();
                assert_eq!(serde_json::from_str::<$ty>(&json).unwrap(), $value);
            }};
        }
        round_trip!(queue, ProposalQueue);
        round_trip!(creation, InventoryCreation);
        round_trip!(ticket, FindingTicket);
        assert_eq!(serde_json::to_value(ticket.risk_tier).unwrap(), "medium");
    }

    // spec: MEDULLA-DECISION-ENGINE-R615.1
    #[test]
    fn writeback_types_reject_invalid_identifiers() {
        for bad in ["", "has space", "é"] {
            assert_eq!(
                WritebackProposal::new(bad, "tenant-a", "ext:1", "sor:ledger", RiskTier::High, 1)
                    .unwrap_err()
                    .code,
                INVALID_WRITEBACK_IDENTIFIER,
                "{bad:?}"
            );
        }
        assert_eq!(
            WritebackProposal::new(
                "wp:1",
                "bad tenant",
                "ext:1",
                "sor:ledger",
                RiskTier::High,
                1
            )
            .unwrap_err()
            .code,
            INVALID_WRITEBACK_IDENTIFIER
        );
        assert_eq!(
            WritebackProposal::new(
                "wp:1",
                "tenant-a",
                "bad external",
                "sor:ledger",
                RiskTier::High,
                1
            )
            .unwrap_err()
            .code,
            INVALID_WRITEBACK_IDENTIFIER
        );
        assert_eq!(
            WritebackProposal::new("wp:1", "tenant-a", "ext:1", "sor:ledger", RiskTier::High, 0)
                .unwrap_err()
                .code,
            INVALID_WRITEBACK_AS_OF
        );
        let other = proposal("ext:1", "tenant-b");
        let queue = ProposalQueue::new("queue:1", "tenant-a", vec![other]);
        assert_eq!(queue.unwrap_err().code, WRITEBACK_TENANT_MISMATCH);
        let duplicate = ProposalQueue::new(
            "queue:1",
            "tenant-a",
            vec![proposal("ext:1", "tenant-a"), proposal("ext:1", "tenant-a")],
        );
        assert_eq!(duplicate.unwrap_err().code, WRITEBACK_DUPLICATE_EXTERNAL_ID);
        assert!(serde_json::from_str::<WritebackProposal>(
            r#"{"id":"wp:1","tenant":"tenant-a","external_id":"ext:1","system_of_record":"sor:ledger","risk_tier":"high","as_of_ms":1,"extra":true}"#
        )
        .is_err());
        assert!(serde_json::from_str::<FindingTicket>(
            r#"{"id":"t","tenant":"tenant-a","external_id":"ext","finding_id":"f","system_of_record":"s","risk_tier":"urgent","as_of_ms":1}"#
        )
        .is_err());
    }

    fn rid(value: &str) -> ResourceId {
        ResourceId::new(value).unwrap()
    }

    // spec: MEDULLA-DECISION-ENGINE-R615.2.1
    #[test]
    fn preflight_refuses_conflicts_and_queues_high_tier() {
        let high = proposal("ext:1", "tenant-a");
        let low =
            WritebackProposal::new("wp:2", "tenant-a", "ext:2", "sor:ledger", RiskTier::Low, 1)
                .unwrap();
        let agree: BTreeMap<String, String> = [("a", "d1"), ("b", "d1")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let conflict: BTreeMap<String, String> = [("a", "d1"), ("b", "d2")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(preflight(&high, &agree, 2), Preflight::Queue);
        assert_eq!(preflight(&low, &agree, 2), Preflight::Proceed);
        assert_eq!(preflight(&low, &BTreeMap::new(), 0), Preflight::Queue);
        let Preflight::Refuse(error) = preflight(&high, &conflict, 2) else {
            panic!("a source conflict refuses")
        };
        assert_eq!(error.code, WRITEBACK_SOURCE_CONFLICT);
        // A conflict refuses even a proposal that would proceed.
        assert!(matches!(
            preflight(&low, &conflict, 2),
            Preflight::Refuse(_)
        ));
        // The queue keys duplicates on (system_of_record, external_id).
        let other_system =
            WritebackProposal::new("wp:3", "tenant-a", "ext:1", "sor:crm", RiskTier::High, 1)
                .unwrap();
        assert!(ProposalQueue::new("q", "tenant-a", vec![high.clone(), other_system]).is_ok());
        assert_eq!(
            ProposalQueue::new("q", "tenant-a", vec![high.clone(), high])
                .unwrap_err()
                .code,
            WRITEBACK_DUPLICATE_EXTERNAL_ID
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R615.2.2
    #[test]
    fn inventory_and_findings_become_deduplicated_proposals() {
        let item = |external: &str, present: &[&str], as_of: i64| InventoryItem {
            tenant: TenantId::new("tenant-a").unwrap(),
            external_id: rid(external),
            present_in: present.iter().map(|s| rid(s)).collect(),
            as_of_ms: as_of,
        };
        let inventory = vec![
            item("ext:b", &["sor:ledger"], 20),
            item("ext:a", &[], 10),
            item("ext:a", &[], 11),
        ];
        let systems = vec![rid("sor:ledger"), rid("sor:crm")];
        let creations = creation_proposals(&inventory, &systems).unwrap();
        let keys: Vec<(&str, &str, i64)> = creations
            .iter()
            .map(|c| {
                (
                    c.system_of_record.as_str(),
                    c.external_id.as_str(),
                    c.as_of_ms,
                )
            })
            .collect();
        assert_eq!(
            keys,
            vec![
                ("sor:crm", "ext:a", 10),
                ("sor:crm", "ext:b", 20),
                ("sor:ledger", "ext:a", 10),
            ]
        );
        let finding = |id: &str, external: &str, tier: RiskTier, as_of: i64| RiskFinding {
            tenant: TenantId::new("tenant-a").unwrap(),
            finding_id: rid(id),
            external_id: rid(external),
            system_of_record: rid("sor:tickets"),
            risk_tier: tier,
            as_of_ms: as_of,
        };
        let tickets = ticket_proposals(&[
            finding("f:2", "ext:z", RiskTier::Low, 5),
            finding("f:1", "ext:z", RiskTier::High, 6),
            finding("f:3", "ext:a", RiskTier::Medium, 7),
        ])
        .unwrap();
        let rows: Vec<(&str, RiskTier, i64)> = tickets
            .iter()
            .map(|t| (t.external_id.as_str(), t.risk_tier, t.as_of_ms))
            .collect();
        assert_eq!(
            rows,
            vec![("ext:a", RiskTier::Medium, 7), ("ext:z", RiskTier::High, 6)]
        );
        assert!(creation_proposals(&[item("bad id", &[], 1)], &systems).is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R615.3
    #[test]
    fn proposal_queue_machine_takes_every_legal_move_and_refuses_the_rest() {
        use QueueEvent::{Approve, Release};
        use QueueState::{Pending, Released};
        let full = Release {
            fully_approved: true,
        };
        let partial = Release {
            fully_approved: false,
        };
        assert_eq!(queue_transition(Pending, Approve).unwrap(), Pending);
        assert_eq!(queue_transition(Pending, full).unwrap(), Released);
        assert_eq!(queue_transition(Released, full).unwrap(), Released);
        assert_eq!(queue_transition(Released, partial).unwrap(), Released);
        let early = queue_transition(Pending, partial).unwrap_err();
        assert_eq!(early.code, WRITEBACK_NOT_FULLY_APPROVED);
        assert!(early.detail.contains("proposal_queue") && early.detail.contains("pending"));
        let late = queue_transition(Released, Approve).unwrap_err();
        assert_eq!(late.code, WRITEBACK_QUEUE_TRANSITION_REFUSED);
        assert!(late.detail.contains("proposal_queue"));
        assert!(late.detail.contains("released") && late.detail.contains("approve"));
        assert_eq!(
            serde_json::to_string(&QueueState::Pending).unwrap(),
            "\"pending\""
        );
    }
}
