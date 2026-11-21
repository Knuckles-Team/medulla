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
    /// Whether the state leaves no edge: the reservation is finished and its row is the
    /// exact replay record (MEDULLA-PLACEMENT-R781.3). Replaces the stored tombstone flag.
    pub fn is_terminal(self) -> bool {
        RESOURCE_RESERVATION_MACHINE.is_terminal(self)
    }

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

/// The state a reservation result reports: the one `ResourceReservationMachine` state, not a copy
/// (MEDULLA-PLACEMENT-R781.5).
pub type ResourceReservationResultState = ResourceReservationRecordState;

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
    pub changed_work_item_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-PLACEMENT-R781.5, MEDULLA-PLACEMENT-R781.6
    #[test]
    fn every_legal_transition_applies() {
        let machine = &RESOURCE_RESERVATION_MACHINE;
        assert!(!machine.edges.is_empty());
        for edge in machine.edges {
            assert_eq!(edge.from.apply(edge.event, |_| true), Ok(edge.to), "{}", edge.id);
            if edge.guard.is_some() {
                let refused = edge.from.apply(edge.event, |_| false).unwrap_err();
                assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed, "{}", edge.id);
            }
        }
    }

    // spec: MEDULLA-PLACEMENT-R781.5, MEDULLA-PLACEMENT-R781.6
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &RESOURCE_RESERVATION_MACHINE;
        for &state in machine.states {
            for &event in machine.events {
                let has_edge = machine
                    .edges
                    .iter()
                    .any(|edge| edge.from == state && edge.event == event);
                if has_edge {
                    continue;
                }
                let refused = state.apply(event, |_| true).unwrap_err();
                let expected = if machine.is_terminal(state) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, expected, "{state:?} {event:?}");
                assert_eq!(refused.machine, "ResourceReservationMachine");
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
    }

    // spec: MEDULLA-PLACEMENT-R781.5, MEDULLA-PLACEMENT-R781.6
    #[test]
    fn no_writer_sets_state_outside_apply() {
        // The result and summary states are the machine's own enum, so a writer that
        // assigns one of the three names directly would skip `apply`.
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../medulla-resource-store/src/redb_store/resource"
        );
        let needles = [
            ".state = ResourceReservationRecordState::",
            ".state = ResourceReservationResultState::",
            ".state = ResourceReservationSummaryState::",
        ];
        let mut scanned = 0;
        for entry in std::fs::read_dir(dir).expect("resource store source directory") {
            let path = entry.expect("directory entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            scanned += 1;
            let source = std::fs::read_to_string(&path).expect("writer source");
            for (n, line) in source.lines().map(str::trim).enumerate() {
                if line.starts_with("//") {
                    continue;
                }
                assert!(
                    !needles.iter().any(|needle| line.contains(needle)),
                    "{}:{}: reservation state written outside apply",
                    path.display(),
                    n + 1
                );
            }
        }
        assert!(scanned > 3, "the resource store writers were not scanned");
    }
}
