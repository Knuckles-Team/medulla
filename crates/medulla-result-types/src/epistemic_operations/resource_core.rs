use super::common::deserialize_required_option;
use super::resource_status::ResourceTargetSnapshot;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ResourceReservationRequestSchemaVersion {
    #[serde(rename = "1")]
    V1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ResourceReservationRequestTargetKind {
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "inventory_alias")]
    InventoryAlias,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ResourceReservationRecordTargetKind {
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "inventory_alias")]
    InventoryAlias,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ResourceReservationRecordState {
    #[serde(rename = "reserved")]
    Reserved,
    #[serde(rename = "released")]
    Released,
    #[serde(rename = "reclaimed")]
    Reclaimed,
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "superseded")]
    Superseded,
    #[serde(rename = "absent")]
    Absent,
}

// MEDULLA-PLACEMENT-R781.1 — the reservation edge table and its one transition function.
// ResourceReservationMachine: the row's edge table. The ontology test compares it with state-machines-v1.ttl.
/// An event of `ResourceReservationMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReservationEvent {
    /// The `Reserve` event.
    Reserve,
    /// The `Release` event.
    Release,
    /// The `Reclaim` event.
    Reclaim,
    /// The `Expire` event.
    Expire,
    /// The `Supersede` event.
    Supersede,
}

impl MachineState for ResourceReservationRecordState {
    fn stored(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Reserved => "reserved",
            Self::Released => "released",
            Self::Reclaimed => "reclaimed",
            Self::Expired => "expired",
            Self::Superseded => "superseded",
        }
    }
}

impl MachineEvent for ReservationEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Reserve => "Reserve",
            Self::Release => "Release",
            Self::Reclaim => "Reclaim",
            Self::Expire => "Expire",
            Self::Supersede => "Supersede",
        }
    }
}

/// Guard of `ResourceReservationMachine`: the resource is free.
pub const RESOURCE_RESERVATION_GUARD_RESOURCE_FREE: &str = "ResourceFree";
/// Guard of `ResourceReservationMachine`: the owner matches.
pub const RESOURCE_RESERVATION_GUARD_OWNER_MATCHES: &str = "OwnerMatches";
/// Guard of `ResourceReservationMachine`: the reclaimer is authorized.
pub const RESOURCE_RESERVATION_GUARD_RECLAIMER_AUTHORIZED: &str = "ReclaimerAuthorized";
/// Guard of `ResourceReservationMachine`: the expiry time has passed.
pub const RESOURCE_RESERVATION_GUARD_EXPIRY_PASSED: &str = "ExpiryPassed";
/// Guard of `ResourceReservationMachine`: a newer reservation of the same resource is admitted.
pub const RESOURCE_RESERVATION_GUARD_NEWER_RESERVATION_ADMITTED: &str = "NewerReservationAdmitted";

/// `ResourceReservationMachine`: the closed edge table. Every other pair of state and event is refused.
pub static RESOURCE_RESERVATION_MACHINE: MachineDef<
    ResourceReservationRecordState,
    ReservationEvent,
    &'static str,
> = MachineDef {
    name: "ResourceReservationMachine",
    states: &[
        ResourceReservationRecordState::Absent,
        ResourceReservationRecordState::Reserved,
        ResourceReservationRecordState::Released,
        ResourceReservationRecordState::Reclaimed,
        ResourceReservationRecordState::Expired,
        ResourceReservationRecordState::Superseded,
    ],
    initial: ResourceReservationRecordState::Absent,
    terminals: &[
        ResourceReservationRecordState::Released,
        ResourceReservationRecordState::Reclaimed,
        ResourceReservationRecordState::Expired,
        ResourceReservationRecordState::Superseded,
    ],
    events: &[
        ReservationEvent::Reserve,
        ReservationEvent::Release,
        ReservationEvent::Reclaim,
        ReservationEvent::Expire,
        ReservationEvent::Supersede,
    ],
    guards: &[
        RESOURCE_RESERVATION_GUARD_RESOURCE_FREE,
        RESOURCE_RESERVATION_GUARD_OWNER_MATCHES,
        RESOURCE_RESERVATION_GUARD_RECLAIMER_AUTHORIZED,
        RESOURCE_RESERVATION_GUARD_EXPIRY_PASSED,
        RESOURCE_RESERVATION_GUARD_NEWER_RESERVATION_ADMITTED,
    ],
    edges: &[
        Edge {
            id: "V01",
            from: ResourceReservationRecordState::Absent,
            event: ReservationEvent::Reserve,
            guard: Some(RESOURCE_RESERVATION_GUARD_RESOURCE_FREE),
            to: ResourceReservationRecordState::Reserved,
        },
        Edge {
            id: "V02",
            from: ResourceReservationRecordState::Reserved,
            event: ReservationEvent::Release,
            guard: Some(RESOURCE_RESERVATION_GUARD_OWNER_MATCHES),
            to: ResourceReservationRecordState::Released,
        },
        Edge {
            id: "V03",
            from: ResourceReservationRecordState::Reserved,
            event: ReservationEvent::Reclaim,
            guard: Some(RESOURCE_RESERVATION_GUARD_RECLAIMER_AUTHORIZED),
            to: ResourceReservationRecordState::Reclaimed,
        },
        Edge {
            id: "V04",
            from: ResourceReservationRecordState::Reserved,
            event: ReservationEvent::Expire,
            guard: Some(RESOURCE_RESERVATION_GUARD_EXPIRY_PASSED),
            to: ResourceReservationRecordState::Expired,
        },
        Edge {
            id: "V05",
            from: ResourceReservationRecordState::Reserved,
            event: ReservationEvent::Supersede,
            guard: Some(RESOURCE_RESERVATION_GUARD_NEWER_RESERVATION_ADMITTED),
            to: ResourceReservationRecordState::Superseded,
        },
    ],
};

impl ResourceReservationRecordState {
    /// The one transition function of `ResourceReservationMachine`. `guard_holds` answers each guard
    /// by name; an unguarded edge always holds.
    pub fn apply(
        self,
        event: ReservationEvent,
        guard_holds: impl Fn(&'static str) -> bool,
    ) -> Result<Self, TransitionRefused> {
        transition(&RESOURCE_RESERVATION_MACHINE, self, event, guard_holds).map(|edge| edge.to)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ResourceReservationResultSchemaVersion {
    #[serde(rename = "1")]
    V1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ResourceReservationResultDecision {
    #[serde(rename = "accepted")]
    Accepted,
    #[serde(rename = "idempotent")]
    Idempotent,
    #[serde(rename = "stale")]
    Stale,
    #[serde(rename = "conflict")]
    Conflict,
    #[serde(rename = "input_conflict")]
    InputConflict,
    #[serde(rename = "capacity")]
    Capacity,
    #[serde(rename = "policy")]
    Policy,
    #[serde(rename = "drained")]
    Drained,
    #[serde(rename = "quarantined")]
    Quarantined,
    #[serde(rename = "stale_host")]
    StaleHost,
    #[serde(rename = "labels")]
    Labels,
    #[serde(rename = "anti_affinity")]
    AntiAffinity,
    #[serde(rename = "disk")]
    Disk,
    #[serde(rename = "concurrency")]
    Concurrency,
    #[serde(rename = "exclusivity")]
    Exclusivity,
    #[serde(rename = "not_found")]
    NotFound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ResourceReservationResultState {
    #[serde(rename = "reserved")]
    Reserved,
    #[serde(rename = "released")]
    Released,
    #[serde(rename = "reclaimed")]
    Reclaimed,
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "superseded")]
    Superseded,
    #[serde(rename = "absent")]
    Absent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ResourceReservationRequest {
    pub schema_version: ResourceReservationRequestSchemaVersion,
    pub tenant_ref: String,
    pub work_item_id: String,
    pub owner_id: String,
    pub fence: String,
    pub lease_epoch: u64,
    pub fencing_token: u64,
    pub attempt: u64,
    pub reservation_id: String,
    pub input_fingerprint: String,
    pub profile_name: String,
    pub profile_version: String,
    pub host_ref: String,
    pub requirement: ResourceRequirement,
    pub target_kind: ResourceReservationRequestTargetKind,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub target_alias: Option<String>,
    pub repository_id: String,
    pub branch: String,
    pub concurrency_key: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub concurrency_limit: Option<u64>,
    pub repository_exclusive: bool,
    pub branch_exclusive: bool,
    pub required_labels: Vec<String>,
    pub anti_affinity: Vec<String>,
    pub fairness_group: String,
    pub fairness_cost: u64,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub disk_low_watermark_mib: Option<u64>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub disk_high_watermark_mib: Option<u64>,
    pub disk_policy_key: String,
    pub reserved_at_ms: u64,
    pub expires_at_ms: u64,
    pub idempotency_key: String,
    pub now_ms: u64,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub expected_host_revision: Option<u64>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub expected_lifecycle_revision: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ResourceCapacitySnapshot {
    pub cpu_weight: u64,
    pub memory_mib: u64,
    pub disk_mib: u64,
    pub process_slots: u64,
    pub host_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ResourceRequirement {
    pub cpu_weight: u64,
    pub memory_mib: u64,
    pub disk_mib: u64,
    pub process_slots: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ResourceReservationRecord {
    pub reservation_id: String,
    pub tenant_ref: String,
    pub owner_id: String,
    pub work_item_id: String,
    pub fence: String,
    pub attempt: u64,
    pub lease_epoch: u64,
    pub fencing_token: u64,
    pub input_fingerprint: String,
    pub host_ref: String,
    pub profile_name: String,
    pub profile_version: String,
    pub requirement: ResourceRequirement,
    pub capacity_snapshot: ResourceCapacitySnapshot,
    pub selected_target: ResourceTargetSnapshot,
    pub target_kind: ResourceReservationRecordTargetKind,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub target_alias: Option<String>,
    pub repository_id: String,
    pub branch: String,
    pub concurrency_key: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub concurrency_limit: Option<u64>,
    pub repository_exclusive: bool,
    pub branch_exclusive: bool,
    pub required_labels: Vec<String>,
    pub anti_affinity: Vec<String>,
    pub fairness_group: String,
    pub fairness_cost: u64,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub disk_low_watermark_mib: Option<u64>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub disk_high_watermark_mib: Option<u64>,
    pub disk_policy_key: String,
    pub reserved_at_ms: u64,
    pub expires_at_ms: u64,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub expected_host_revision: Option<u64>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub expected_lifecycle_revision: Option<u64>,
    pub state: ResourceReservationRecordState,
    pub revision: u64,
    pub lifecycle_revision: u64,
    pub tombstone: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ResourceReservationResult {
    pub schema_version: ResourceReservationResultSchemaVersion,
    pub decision: ResourceReservationResultDecision,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub reservation_id: Option<String>,
    pub work_item_id: String,
    pub attempt: u64,
    pub lease_epoch: u64,
    pub fencing_token: u64,
    pub lifecycle_revision: u64,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub host_ref: Option<String>,
    pub host_revision: u64,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub record: Option<ResourceReservationRecord>,
    pub state: ResourceReservationResultState,
    pub held_cpu_weight: u64,
    pub held_memory_mib: u64,
    pub held_disk_mib: u64,
    pub held_process_slots: u64,
    pub fairness_debt: u64,
    pub tombstone: bool,
    pub changed_work_item_ids: Vec<String>,
}
