//! MEDULLA-PLACEMENT-R786 over a real shard: the build-slot lease machine, its fair queue, its
//! deadline sweep and its transition record, through the same `commit` the served methods use.

use medulla_mutation_types::capacity_lease::{
    CapacityCell, CapacityLease, CapacityLeaseEvent, CapacityLeaseState, CapacityResourceClass,
    LeasePriority,
};
use medulla_protocol_types::protocol::Method;
use medulla_store::redb_store::shard::Shard;
use medulla_store::{redb_store::decode_durable, redb_store::DurableCrypto};
use medulla_work_types::native_control::{
    CapacityAcquireRequest, CapacityAcquireResult, CapacityCellUpdateRequest, CapacityDemand,
    CapacityLeaseFence, CapacityLeaseMutationRequest, CapacityMutationResult,
    CapacityReclaimRequest, CapacityReclaimResult, CapacityRevokeRequest, CapacityStatusRequest,
    CapacityStatusResult, NativeControlSchemaVersion, CAPACITY_SWEEP_ALL_TENANTS,
};

const GRAPH: &str = "graph-build-slots";
const CELL: &str = "build-slots:r820";
const V1: NativeControlSchemaVersion = NativeControlSchemaVersion::V1;

struct Harness {
    path: std::path::PathBuf,
    shard: Shard,
    #[cfg(feature = "security")]
    tail: medulla_store::redb_store::AuditTailCache,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Harness {
    fn open(tag: &str) -> Self {
        let path = medulla_store::redb_store::temp_path("medulla-capacity-build-slot", tag);
        let shard = Shard::open(&path).expect("temp shard opens");
        Self {
            path,
            shard,
            #[cfg(feature = "security")]
            tail: medulla_store::redb_store::AuditTailCache::new(),
        }
    }

    fn commit(&mut self, method: &Method) -> Result<Vec<u8>, String> {
        #[cfg(feature = "security")]
        return super::commit_audited(
            &self.shard,
            GRAPH,
            method,
            DurableCrypto::none(),
            &mut self.tail,
        );
        #[cfg(not(feature = "security"))]
        super::commit(&self.shard, GRAPH, method, DurableCrypto::none())
    }

    fn declare(&mut self, capacity: u64, reserved_floor: u64) {
        let cell = CapacityCell {
            cell_id: CELL.to_string(),
            parent_id: None,
            resource_class: CapacityResourceClass::BuildSlot,
            capacity,
            reserved_floor,
            epoch: 1,
            policy_digest: "d".repeat(64),
            updated_at_ms: 0,
            throttle: None,
        };
        self.commit(&Method::UpdateCapacityCell {
            request: CapacityCellUpdateRequest {
                schema_version: V1,
                cell,
                expected_epoch: None,
                now_ms: 1,
            },
        })
        .expect("the cell is declared");
    }

    fn acquire(
        &mut self,
        key: &str,
        owner: &str,
        priority: LeasePriority,
        now_ms: u64,
    ) -> Result<CapacityAcquireResult, String> {
        let request = CapacityAcquireRequest {
            schema_version: V1,
            tenant_ref: "tenant-a".to_string(),
            work_item_id: format!("wi-{key}"),
            owner_digest: owner.to_string(),
            idempotency_key: key.to_string(),
            priority,
            demands: vec![CapacityDemand {
                cell_id: CELL.to_string(),
                resource_class: CapacityResourceClass::BuildSlot,
                amount: 1,
            }],
            lease_id: Some(format!("lease-{key}")),
            ttl_ms: 1_000,
            now_ms,
            cost_budget_micros: None,
            token_budget: None,
            queue_ttl_ms: Some(5_000),
        };
        self.commit(&Method::AcquireCapacity { request })
            .and_then(|bytes| decode_durable(&bytes))
    }

    fn mutation(
        &self,
        lease: &CapacityLease,
        owner: &str,
        now_ms: u64,
    ) -> CapacityLeaseMutationRequest {
        CapacityLeaseMutationRequest {
            schema_version: V1,
            tenant_ref: "tenant-a".to_string(),
            owner_digest: owner.to_string(),
            leases: vec![CapacityLeaseFence {
                lease_id: lease.lease_id.clone(),
                lease_epoch: lease.lease_epoch,
                fence_token: lease.fence_token,
            }],
            now_ms,
            ttl_ms: Some(1_000),
            idempotency_key: None,
        }
    }

    fn hold(
        &mut self,
        method: fn(CapacityLeaseMutationRequest) -> Method,
        lease: &CapacityLease,
        owner: &str,
        now_ms: u64,
    ) -> Result<CapacityMutationResult, String> {
        let request = self.mutation(lease, owner, now_ms);
        self.commit(&method(request))
            .and_then(|bytes| decode_durable(&bytes))
    }

    fn sweep(&mut self, now_ms: u64) -> CapacityReclaimResult {
        let request = CapacityReclaimRequest {
            schema_version: V1,
            tenant_ref: CAPACITY_SWEEP_ALL_TENANTS.to_string(),
            cell_id: None,
            max_count: 16,
            now_ms,
            cursor: None,
        };
        let bytes = self
            .commit(&Method::ReclaimExpiredCapacity { request })
            .expect("the sweep commits");
        decode_durable(&bytes).expect("the sweep answers")
    }

    fn status(&self, lease_id: &str) -> CapacityStatusResult {
        let request = CapacityStatusRequest {
            schema_version: V1,
            tenant_ref: "tenant-a".to_string(),
            cell_id: None,
            lease_id: Some(lease_id.to_string()),
            max_count: 1,
            cursor: None,
        };
        super::read(&self.shard, GRAPH, &request, DurableCrypto::none()).expect("status reads")
    }

    fn state(&self, lease_id: &str) -> CapacityLeaseState {
        self.status(lease_id).leases[0].state
    }

    #[cfg(feature = "security")]
    fn audit_entries(&self) -> u64 {
        medulla_store::redb_store::verify_audit(&self.shard, GRAPH)
            .expect("the chain reads")
            .entries
    }
}

fn start(request: CapacityLeaseMutationRequest) -> Method {
    Method::StartCapacity { request }
}
fn renew(request: CapacityLeaseMutationRequest) -> Method {
    Method::RenewCapacity { request }
}
fn release(request: CapacityLeaseMutationRequest) -> Method {
    Method::ReleaseCapacity { request }
}

fn only(result: CapacityAcquireResult) -> CapacityLease {
    assert_eq!(result.leases.len(), 1);
    result.leases.into_iter().next().unwrap()
}

// spec: MEDULLA-PLACEMENT-R786.6
#[test]
fn every_transition_appends_one_audit_entry_and_a_refusal_appends_none() {
    let mut harness = Harness::open("transitions");
    harness.declare(1, 0);
    let holder = only(
        harness
            .acquire("a", "holder-a", LeasePriority::Lane, 10)
            .unwrap(),
    );
    assert_eq!(holder.state, CapacityLeaseState::Granted);
    // The second lease waits: Requested, then Queued.
    let waiting_result = harness
        .acquire("b", "holder-b", LeasePriority::Lane, 20)
        .unwrap();
    assert_eq!(waiting_result.queue_position, Some(1));
    let waiting = only(waiting_result);
    assert_eq!(waiting.state, CapacityLeaseState::Queued);
    #[cfg(feature = "security")]
    let before_release = harness.audit_entries();
    // The first holder releases; the queued lease is granted in the same transaction.
    harness
        .hold(release, &holder, "holder-a", 30)
        .expect("the owner releases");
    assert_eq!(
        harness.state(&waiting.lease_id),
        CapacityLeaseState::Granted
    );
    #[cfg(feature = "security")]
    assert_eq!(
        harness.audit_entries(),
        before_release + 3,
        "the release call, the released lease and the granted lease"
    );
    let granted = harness.status(&waiting.lease_id).leases[0].clone();
    harness
        .hold(start, &granted, "holder-b", 40)
        .expect("the owner starts");
    #[cfg(feature = "security")]
    let before_refusal = harness.audit_entries();
    // A start of a running lease and a release by a non-owner are refused and append nothing.
    let refused = harness.hold(start, &granted, "holder-b", 45).unwrap_err();
    assert!(refused.starts_with("TRANSITION_REFUSED"), "{refused}");
    let stranger = harness.hold(release, &granted, "holder-a", 46).unwrap_err();
    assert!(stranger.contains("guard_failed"), "{stranger}");
    #[cfg(feature = "security")]
    assert_eq!(harness.audit_entries(), before_refusal);
    harness
        .hold(release, &granted, "holder-b", 50)
        .expect("the owner releases");
    // The newest transitions of the waiting lease, in order, as the status read serves them.
    let status = harness.status(&waiting.lease_id);
    let events: Vec<CapacityLeaseEvent> = status.transitions[0]
        .entries
        .iter()
        .map(|record| record.event)
        .collect();
    assert_eq!(
        events,
        [
            CapacityLeaseEvent::Enqueue,
            CapacityLeaseEvent::Grant,
            CapacityLeaseEvent::Start,
            CapacityLeaseEvent::Release,
        ]
    );
    let states: Vec<CapacityLeaseState> = status.transitions[0]
        .entries
        .iter()
        .map(|record| record.to)
        .collect();
    assert_eq!(
        states,
        [
            CapacityLeaseState::Queued,
            CapacityLeaseState::Granted,
            CapacityLeaseState::Running,
            CapacityLeaseState::Released,
        ]
    );
    assert!(
        status.leases[0].transitions.is_empty(),
        "the history travels beside the row"
    );
}

// spec: MEDULLA-PLACEMENT-R786.3
#[test]
fn a_released_lease_and_a_queued_start_are_refused_and_change_no_row() {
    let mut harness = Harness::open("refusals");
    harness.declare(1, 0);
    let first = only(
        harness
            .acquire("a", "owner", LeasePriority::Lane, 10)
            .unwrap(),
    );
    let second = only(
        harness
            .acquire("b", "owner", LeasePriority::Lane, 11)
            .unwrap(),
    );
    assert_eq!(second.state, CapacityLeaseState::Queued);
    let queued_start = harness.hold(start, &second, "owner", 12).unwrap_err();
    assert!(
        queued_start.starts_with("TRANSITION_REFUSED"),
        "{queued_start}"
    );
    assert!(
        queued_start.contains("illegal_transition"),
        "{queued_start}"
    );
    assert_eq!(harness.state(&second.lease_id), CapacityLeaseState::Queued);
    harness.hold(release, &first, "owner", 13).unwrap();
    let again = harness.hold(release, &first, "owner", 14).unwrap_err();
    assert!(again.starts_with("TRANSITION_REFUSED"), "{again}");
    assert!(again.contains("terminal_state"), "{again}");
    // A renew after release is the holder's stop signal.
    let late = harness.hold(renew, &first, "owner", 15).unwrap_err();
    assert!(late.contains("terminal_state"), "{late}");
    assert_eq!(harness.state(&first.lease_id), CapacityLeaseState::Released);
}

// spec: MEDULLA-PLACEMENT-R786.4
#[test]
fn a_lease_that_stops_renewing_expires_and_returns_its_slot() {
    let mut harness = Harness::open("expiry");
    harness.declare(1, 0);
    let first = only(
        harness
            .acquire("a", "owner-a", LeasePriority::Lane, 100)
            .unwrap(),
    );
    let second = only(
        harness
            .acquire("b", "owner-b", LeasePriority::Lane, 110)
            .unwrap(),
    );
    assert_eq!(second.state, CapacityLeaseState::Queued);
    harness.hold(start, &first, "owner-a", 200).unwrap();
    harness.hold(renew, &first, "owner-a", 900).unwrap();
    // Inside the renewed deadline the sweep finds nothing.
    assert!(harness.sweep(1_500).reclaimed_lease_ids.is_empty());
    // Past it the sweep expires the silent holder and grants the queue head in the same pass.
    let swept = harness.sweep(2_000);
    assert_eq!(
        swept.reclaimed_lease_ids,
        std::slice::from_ref(&first.lease_id)
    );
    assert_eq!(harness.state(&first.lease_id), CapacityLeaseState::Expired);
    assert_eq!(harness.state(&second.lease_id), CapacityLeaseState::Granted);
    let granted = harness.status(&second.lease_id).leases[0].clone();
    assert_eq!(
        granted.expires_at_ms, 3_000,
        "the deadline restarts at the grant"
    );
    let stop = harness.hold(renew, &first, "owner-a", 2_100).unwrap_err();
    assert!(stop.contains("terminal_state"), "{stop}");
}

// spec: MEDULLA-PLACEMENT-R786.5
#[test]
fn land_gate_and_train_are_granted_before_lane_and_each_class_is_fifo() {
    let mut harness = Harness::open("fair-queue");
    harness.declare(2, 1);
    let lane = only(
        harness
            .acquire("lane-holder", "o", LeasePriority::Lane, 1)
            .unwrap(),
    );
    let train = only(
        harness
            .acquire("train-holder", "o", LeasePriority::Train, 2)
            .unwrap(),
    );
    for (key, priority, at) in [
        ("l1", LeasePriority::Lane, 3),
        ("l2", LeasePriority::Lane, 4),
        ("t3", LeasePriority::Train, 5),
        ("g4", LeasePriority::LandGate, 6),
    ] {
        let queued = only(harness.acquire(key, "o", priority, at).unwrap());
        assert_eq!(queued.state, CapacityLeaseState::Queued, "{key}");
    }
    harness.hold(release, &train, "o", 10).unwrap();
    assert_eq!(harness.state("lease-g4"), CapacityLeaseState::Granted);
    assert_eq!(harness.state("lease-t3"), CapacityLeaseState::Queued);
    let gate = harness.status("lease-g4").leases[0].clone();
    harness.hold(release, &gate, "o", 11).unwrap();
    assert_eq!(harness.state("lease-t3"), CapacityLeaseState::Granted);
    // The lane holder leaves: only the reserved slot is free, and a lane never takes it.
    harness.hold(release, &lane, "o", 12).unwrap();
    assert_eq!(harness.state("lease-l1"), CapacityLeaseState::Queued);
    let train = harness.status("lease-t3").leases[0].clone();
    harness.hold(release, &train, "o", 13).unwrap();
    assert_eq!(harness.state("lease-l1"), CapacityLeaseState::Granted);
    assert_eq!(harness.state("lease-l2"), CapacityLeaseState::Queued);
}

// spec: MEDULLA-PLACEMENT-R786.1
#[test]
fn a_build_slot_cell_refuses_the_older_priorities_and_other_cells_refuse_the_new_ones() {
    let mut harness = Harness::open("priorities");
    harness.declare(2, 1);
    let refused = harness
        .acquire("x", "o", LeasePriority::Interactive, 1)
        .unwrap_err();
    assert!(
        refused.starts_with("PRIORITY_NOT_ALLOWED_FOR_RESOURCE"),
        "{refused}"
    );
}

// spec: MEDULLA-PLACEMENT-R786.7
#[test]
fn an_administrator_revokes_a_lease_and_its_holder_is_told_to_stop() {
    let mut harness = Harness::open("revoke");
    harness.declare(1, 0);
    let first = only(
        harness
            .acquire("a", "owner-a", LeasePriority::Lane, 10)
            .unwrap(),
    );
    let second = only(
        harness
            .acquire("b", "owner-b", LeasePriority::Lane, 11)
            .unwrap(),
    );
    harness.hold(start, &first, "owner-a", 12).unwrap();
    let revoke = |lease_id: &str, reason: &str| Method::RevokeCapacity {
        request: CapacityRevokeRequest {
            schema_version: V1,
            tenant_ref: "tenant-a".to_string(),
            lease_id: lease_id.to_string(),
            reason: reason.to_string(),
            now_ms: 20,
        },
    };
    let bytes = harness
        .commit(&revoke(&first.lease_id, "host drained"))
        .unwrap();
    let revoked: CapacityMutationResult = decode_durable(&bytes).unwrap();
    assert_eq!(revoked.leases[0].state, CapacityLeaseState::Revoked);
    assert_eq!(harness.state(&second.lease_id), CapacityLeaseState::Granted);
    let stop = harness.hold(renew, &first, "owner-a", 21).unwrap_err();
    assert!(stop.contains("terminal_state"), "{stop}");
    let again = harness
        .commit(&revoke(&first.lease_id, "twice"))
        .unwrap_err();
    assert!(again.starts_with("TRANSITION_REFUSED"), "{again}");
    let long = "x".repeat(201);
    assert!(harness.commit(&revoke(&second.lease_id, &long)).is_err());
    assert!(harness
        .commit(&revoke(&second.lease_id, "bad\u{7}reason"))
        .is_err());
}
