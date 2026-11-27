// MEDULLA-PLACEMENT-R786.9: two lanes and a land gate share one build host's slots through the
// served capacity methods. The cell has two slots and one reserved: lane A is granted, lane B
// waits because the second slot is reserved, and the land gate is granted from the reserved
// slot. Lane A starts, renews and goes silent; the clock passes its TTL and the sweep expires
// it, but lane B stays queued because only the reserved slot is free. An administrator revokes
// the land gate, which returns the reserved slot's holder and grants lane B; a later renew by
// the land gate is refused `TRANSITION_REFUSED`. Every step is read back through
// `CapacityStatus`.

use medulla_kernel::kernel::dispatch_support::with_engine_clock_offset;
use medulla_mutation_types::capacity_lease::{
    CapacityCell, CapacityLease, CapacityLeaseEvent, CapacityLeaseState, CapacityResourceClass,
    LeasePriority,
};
use medulla_protocol_types::protocol::{Method, Response};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_work_types::native_control::{
    CapacityAcquireRequest, CapacityAcquireResult, CapacityCellUpdateRequest, CapacityDemand,
    CapacityLeaseFence, CapacityLeaseMutationRequest, CapacityMutationResult,
    CapacityReclaimRequest, CapacityReclaimResult, CapacityRevokeRequest, CapacityStatusRequest,
    CapacityStatusResult, NativeControlSchemaVersion,
};

use super::connector_pack_harness::{ok, Served, GRAPH, TENANT};

const CELL: &str = "build-slots:r820";
const LEASE_SCOPE: &[&str] = &["capacity:lease", "capacity:read"];
const ADMIN_SCOPE: &[&str] = &["capacity:admin", "capacity:read"];
const V1: NativeControlSchemaVersion = NativeControlSchemaVersion::V1;
/// The heartbeat deadline of the lanes in this test: small, so one clock step passes it.
const LANE_TTL_MS: u64 = 1_000;
const GATE_TTL_MS: u64 = 600_000;
const STEP_MS: u64 = 5_000;

fn owner(principal: &str) -> String {
    VerifiedRequestContext::verified_for_test_with_scopes(principal, TENANT, &[])
        .principal_persistence_id()
}

fn refusal_text(response: &Response) -> String {
    format!(
        "{} {}",
        response.error.clone().unwrap_or_default(),
        response.error_detail.clone().unwrap_or_default()
    )
}

async fn acquire(
    served: &Served,
    principal: &str,
    priority: LeasePriority,
    ttl_ms: u64,
) -> CapacityAcquireResult {
    let request = CapacityAcquireRequest {
        schema_version: V1,
        tenant_ref: TENANT.to_string(),
        work_item_id: format!("wi-{principal}"),
        owner_digest: owner(principal),
        idempotency_key: format!("key-{principal}"),
        priority,
        demands: vec![CapacityDemand {
            cell_id: CELL.to_string(),
            resource_class: CapacityResourceClass::BuildSlot,
            amount: 1,
        }],
        lease_id: None,
        ttl_ms,
        now_ms: 0,
        cost_budget_micros: None,
        token_budget: None,
        queue_ttl_ms: Some(600_000),
    };
    ok(
        "AcquireCapacity",
        served
            .verified_as(
                principal,
                TENANT,
                LEASE_SCOPE,
                Method::AcquireCapacity { request },
            )
            .await,
    )
}

fn fence(lease: &CapacityLease, principal: &str, ttl_ms: u64) -> CapacityLeaseMutationRequest {
    CapacityLeaseMutationRequest {
        schema_version: V1,
        tenant_ref: TENANT.to_string(),
        owner_digest: owner(principal),
        leases: vec![CapacityLeaseFence {
            lease_id: lease.lease_id.clone(),
            lease_epoch: lease.lease_epoch,
            fence_token: lease.fence_token,
        }],
        now_ms: 0,
        ttl_ms: Some(ttl_ms),
        idempotency_key: None,
    }
}

async fn status(
    served: &Served,
    lease: &CapacityLease,
) -> (CapacityLease, Vec<CapacityLeaseEvent>) {
    let request = CapacityStatusRequest {
        schema_version: V1,
        tenant_ref: TENANT.to_string(),
        cell_id: None,
        lease_id: Some(lease.lease_id.clone()),
        max_count: 1,
        cursor: None,
    };
    let result: CapacityStatusResult = ok(
        "CapacityStatus",
        served
            .verified_as(
                "reader",
                TENANT,
                LEASE_SCOPE,
                Method::CapacityStatus { request },
            )
            .await,
    );
    let events = result.transitions[0]
        .entries
        .iter()
        .map(|record| record.event)
        .collect();
    (result.leases.into_iter().next().expect("the lease"), events)
}

// spec: MEDULLA-PLACEMENT-R786.9
// spec: MEDULLA-PLACEMENT-R786
#[tokio::test]
async fn two_lanes_one_slot_queue_grant_and_expire_in_order() {
    let served =
        Served::provisioned(GRAPH, "build-slot-source", &[("paper", "ResearchPaper")]).await;
    // The operator registers the host's pool: two slots, one reserved.
    let cell = CapacityCell {
        cell_id: CELL.to_string(),
        parent_id: None,
        resource_class: CapacityResourceClass::BuildSlot,
        capacity: 2,
        reserved_floor: 1,
        epoch: 1,
        policy_digest: "d".repeat(64),
        updated_at_ms: 0,
        throttle: None,
    };
    let update = Method::UpdateCapacityCell {
        request: CapacityCellUpdateRequest {
            schema_version: V1,
            cell,
            expected_epoch: None,
            now_ms: 0,
        },
    };
    ok::<serde_json::Value>(
        "UpdateCapacityCell",
        served
            .verified_as("operator", TENANT, ADMIN_SCOPE, update)
            .await,
    );

    // Lane A is granted; lane B waits because the second slot is reserved; the land gate takes it.
    let lane_a = acquire(&served, "lane-a", LeasePriority::Lane, LANE_TTL_MS).await;
    let a = lane_a.leases[0].clone();
    assert_eq!(a.state, CapacityLeaseState::Granted);
    let lane_b = acquire(&served, "lane-b", LeasePriority::Lane, LANE_TTL_MS).await;
    let b = lane_b.leases[0].clone();
    assert_eq!(b.state, CapacityLeaseState::Queued);
    assert_eq!(lane_b.queue_position, Some(1));
    let gate = acquire(&served, "land-gate", LeasePriority::LandGate, GATE_TTL_MS).await;
    let g = gate.leases[0].clone();
    assert_eq!(g.state, CapacityLeaseState::Granted);
    assert_eq!(
        status(&served, &b).await.0.state,
        CapacityLeaseState::Queued
    );

    // Lane A starts and renews, then goes silent.
    ok::<CapacityMutationResult>(
        "StartCapacity",
        served
            .verified_as(
                "lane-a",
                TENANT,
                LEASE_SCOPE,
                Method::StartCapacity {
                    request: fence(&a, "lane-a", LANE_TTL_MS),
                },
            )
            .await,
    );
    ok::<CapacityMutationResult>(
        "RenewCapacity",
        served
            .verified_as(
                "lane-a",
                TENANT,
                LEASE_SCOPE,
                Method::RenewCapacity {
                    request: fence(&a, "lane-a", LANE_TTL_MS),
                },
            )
            .await,
    );
    let (running, events) = status(&served, &a).await;
    assert_eq!(running.state, CapacityLeaseState::Running);
    assert_eq!(
        events,
        [
            CapacityLeaseEvent::Grant,
            CapacityLeaseEvent::Start,
            CapacityLeaseEvent::Renew
        ]
    );

    // The clock passes lane A's deadline; the sweep expires it. Only the reserved slot is free
    // (the land gate holds the other), and a lane never takes it, so lane B keeps waiting.
    let sweep = Method::ReclaimExpiredCapacity {
        request: CapacityReclaimRequest {
            schema_version: V1,
            tenant_ref: TENANT.to_string(),
            cell_id: Some(CELL.to_string()),
            max_count: 16,
            now_ms: 0,
            cursor: None,
        },
    };
    let swept: CapacityReclaimResult = ok(
        "ReclaimExpiredCapacity",
        with_engine_clock_offset(
            STEP_MS,
            served.verified_as("sweeper", TENANT, LEASE_SCOPE, sweep),
        )
        .await,
    );
    assert_eq!(swept.reclaimed_lease_ids, [a.lease_id.clone()]);
    let (expired, expired_events) = status(&served, &a).await;
    assert_eq!(expired.state, CapacityLeaseState::Expired);
    assert_eq!(expired_events.last(), Some(&CapacityLeaseEvent::Expire));
    assert_eq!(
        status(&served, &b).await.0.state,
        CapacityLeaseState::Queued
    );
    // The silent holder's next heartbeat is the stop signal.
    let stale = served
        .verified_as(
            "lane-a",
            TENANT,
            LEASE_SCOPE,
            Method::RenewCapacity {
                request: fence(&a, "lane-a", LANE_TTL_MS),
            },
        )
        .await;
    let text = refusal_text(&stale);
    assert!(
        text.contains("TRANSITION_REFUSED") && text.contains("terminal_state"),
        "{text}"
    );

    // An administrator revokes the land gate: lane B is granted, and a later renew by the land
    // gate is refused.
    let revoke = Method::RevokeCapacity {
        request: CapacityRevokeRequest {
            schema_version: V1,
            tenant_ref: TENANT.to_string(),
            lease_id: g.lease_id.clone(),
            reason: "host drained for maintenance".to_string(),
            now_ms: 0,
        },
    };
    let revoked: CapacityMutationResult = ok(
        "RevokeCapacity",
        served
            .verified_as("operator", TENANT, ADMIN_SCOPE, revoke)
            .await,
    );
    assert_eq!(revoked.leases[0].state, CapacityLeaseState::Revoked);
    let late = served
        .verified_as(
            "land-gate",
            TENANT,
            LEASE_SCOPE,
            Method::RenewCapacity {
                request: fence(&g, "land-gate", GATE_TTL_MS),
            },
        )
        .await;
    let late_text = refusal_text(&late);
    assert!(
        late_text.contains("TRANSITION_REFUSED") && late_text.contains("terminal_state"),
        "{late_text}"
    );
    // The revoke returned the land gate's slot and lane B, first in its queue, is granted.
    let (granted, granted_events) = status(&served, &b).await;
    assert_eq!(granted.state, CapacityLeaseState::Granted);
    assert_eq!(
        granted_events,
        [CapacityLeaseEvent::Enqueue, CapacityLeaseEvent::Grant]
    );
    let (final_gate, gate_events) = status(&served, &g).await;
    assert_eq!(final_gate.state, CapacityLeaseState::Revoked);
    assert_eq!(
        gate_events,
        [CapacityLeaseEvent::Grant, CapacityLeaseEvent::Revoke]
    );

    // A holder that is not the lease's owner is refused before the ledger, and a lease
    // administrator without `capacity:admin` cannot revoke.
    let intruder = served
        .verified_as(
            "lane-a",
            TENANT,
            LEASE_SCOPE,
            Method::RevokeCapacity {
                request: CapacityRevokeRequest {
                    schema_version: V1,
                    tenant_ref: TENANT.to_string(),
                    lease_id: b.lease_id.clone(),
                    reason: "no".to_string(),
                    now_ms: 0,
                },
            },
        )
        .await;
    assert!(
        refusal_text(&intruder).contains("ACCESS_DENIED"),
        "{intruder:?}"
    );
}
