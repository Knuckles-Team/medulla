//! The agent-run delegation kind (MEDULLA-IDENTITY-R742): a record for one run on behalf of a
//! principal, scoped to a set of operations, expiring and revocable on the engine clock.

use super::*;

const TENANT: &str = "tenant-a";
const DELEGATION: &str = "delegation:run-1";

fn agent_run(principal: &str) -> DelegationRecord {
    DelegationRecord {
        tenant: TENANT.to_string(),
        principal: principal.to_string(),
        expires_at: NOW + 60_000,
        kind: DelegationKind::AgentRun,
        run_id: Some("run-1".to_string()),
        operations: ["graph:read".to_string()].into_iter().collect(),
        ..DelegationRecord::default()
    }
}

fn record(id: &str, record: DelegationRecord) -> IdentityOp {
    IdentityOp::Delegation(DelegationOp::Record {
        request: DelegationRecordWrite {
            work_item_id: id.to_string(),
            record,
        },
    })
}

// spec: MEDULLA-IDENTITY-R742
#[test]
fn an_agent_run_record_needs_its_run_a_bounded_operation_set_and_a_future_expiry() {
    let mut store = store_in(AuthMode::Local);
    let alice = create(&mut store, "alice", UserKind::Human).unwrap();
    let mut refused = |mutate: &dyn Fn(&mut DelegationRecord)| {
        let mut bad = agent_run(&alice);
        mutate(&mut bad);
        store.apply(&record(DELEGATION, bad), &admin(), &ctx_at(NOW))
    };
    assert_eq!(
        refused(&|bad| bad.run_id = None),
        Err(IdentityRefusal::InvalidRequest)
    );
    assert_eq!(
        refused(&|bad| bad.operations.clear()),
        Err(IdentityRefusal::InvalidRequest)
    );
    assert_eq!(
        refused(&|bad| bad.tool = "graph:read".to_string()),
        Err(IdentityRefusal::InvalidRequest),
        "an agent run names operations, never a tool scope"
    );
    assert_eq!(
        refused(&|bad| bad.expires_at = NOW),
        Err(IdentityRefusal::InvalidRequest),
        "a delegation already expired at the engine clock is not recorded"
    );
    assert_eq!(
        refused(&|bad| bad.principal = "usr:nobody".to_string()),
        Err(IdentityRefusal::InvalidRequest)
    );
    apply_kept(
        &mut store,
        &record(DELEGATION, agent_run(&alice)),
        &admin(),
        NOW,
    )
    .unwrap();
}

// spec: MEDULLA-IDENTITY-R742
#[test]
fn an_agent_run_delegation_expires_and_is_revoked_on_the_engine_clock_and_is_tenant_keyed() {
    let mut store = store_in(AuthMode::Local);
    let alice = create(&mut store, "alice", UserKind::Human).unwrap();
    apply_kept(
        &mut store,
        &record(DELEGATION, agent_run(&alice)),
        &admin(),
        NOW,
    )
    .unwrap();
    let standing = |store: &IdentityStore, tenant: &str, now: u64| {
        store
            .delegation_state(DELEGATION, tenant, now, &TestRegistry)
            .map(|state| (state.record.kind, state.standing))
    };
    assert_eq!(
        standing(&store, TENANT, NOW),
        Some((DelegationKind::AgentRun, DelegationStanding::Active))
    );
    assert_eq!(
        standing(&store, TENANT, NOW + 60_000),
        Some((DelegationKind::AgentRun, DelegationStanding::Expired)),
        "the delegation expires at its horizon"
    );
    assert_eq!(
        standing(&store, "tenant-b", NOW),
        None,
        "a foreign tenant resolves the record as absent"
    );
    let revoke = IdentityOp::Delegation(DelegationOp::Revoke {
        request: ObjectRef {
            id: DELEGATION.to_string(),
        },
    });
    apply_kept(&mut store, &revoke, &admin(), NOW + 1).unwrap();
    assert_eq!(
        standing(&store, TENANT, NOW + 1),
        Some((DelegationKind::AgentRun, DelegationStanding::Revoked))
    );
}
