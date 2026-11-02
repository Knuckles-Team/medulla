use super::DurableLaneCounter;
use medulla_result_types::epistemic_operations::DevelopmentLaneIntent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneDecision {
    Accepted,
    Idempotent,
    Stale,
    Conflict,
    InputConflict,
    Quota,
    Policy,
    Drained,
    NotFound,
    WrongKind,
    WrongTenant,
    WrongOwner,
    WrongAttempt,
    WrongLeaseEpoch,
    WrongFence,
    Expired,
    Terminal,
    CleanupRequired,
    Exclusivity,
    Invalid,
}

#[derive(Debug, Clone)]
pub struct LaneWorkItem {
    pub status: String,
    pub terminal: bool,
    pub lease_expires_at_ms: u64,
    pub host_ref: String,
    pub resource_reservation_id: String,
    pub cleanup_hold_id: Option<String>,
    pub cleanup_lane_id: Option<String>,
    pub cleanup_expected_hold_revision: Option<u64>,
    pub lane_intent: Option<DevelopmentLaneIntent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Tenant,
    Owner,
    Session,
    Workspace,
    Repository,
    Host,
    Global,
}

#[derive(Debug, Clone)]
pub struct ScopeCounter {
    pub key: String,
    pub scope: Scope,
    pub value: DurableLaneCounter,
}
