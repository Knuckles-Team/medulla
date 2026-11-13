//! MEDULLA-DECISION-ENGINE-R036.103 and R036.104: the cross-owner admission journey of one
//! served `DecisionCommit`, through the handler, against a durable RBAC store and a durable
//! Agent Library. MEDULLA-DECISION-ENGINE-R036.106: the same journey holds durable profile-use
//! grants, which the admission consults and a reservation never changes.

use std::sync::Arc;

use medulla_decision_types::decision::{DecisionCommitRequest, DecisionCommitResult};
use medulla_isolation::isolation::IsolationLayer;
use medulla_isolation::rbac_persist::RbacPolicyStore;
use medulla_persistence::redb_backend::RedbBackend;
use medulla_persistence_records::agent_library::admission::AdmissionState;
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_persistence_records::decision_record::tests::{
    commit_context, decided, seed_library, TENANT,
};
use medulla_protocol_types::protocol::ResultPayload;
use medulla_raft_xshard::cross_shard_txn::certificate::{request_digest, OwnerDecision};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::reservation_machine::{ReservationEvent, ReservationState};
use tokio::sync::RwLock;

use crate::decide::commit::admission::Admitted;
use crate::decide::commit::certified::{Coordinator, REQUEST_NOW_MS};
use crate::decide::commit::handle_decision_commit;

fn verified(key: &str, nonce: u8) -> VerifiedRequestContext {
    let claims = VerifiedRequestContext::verified_for_test_in_tenant("caller-a", TENANT);
    VerifiedRequestContext::from_verified_claims_with_nonce(
        claims.claims().clone(),
        key.into(),
        Some(medulla_types::contract::Nonce::from_bytes([nonce; 32])),
    )
}

// spec: MEDULLA-DECISION-ENGINE-R036.103
// spec: MEDULLA-DECISION-ENGINE-R036.104
// spec: MEDULLA-DECISION-ENGINE-R036.106
// spec: MEDULLA-DECISION-ENGINE-R036.105
#[tokio::test]
async fn cross_owner_admission_journey() {
    #[cfg(any(feature = "security", feature = "raft"))]
    let _env_guard = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let dir = tempfile::tempdir().unwrap();
    let rbac_dir = tempfile::tempdir().unwrap();
    let rbac =
        Arc::new(medulla_isolation::test_scope_grant::open_rbac_store(rbac_dir.path()).unwrap());
    // Durable profile-use grants the admission will consult: one active, one revoked.
    let active = rbac.grant_profile_use("caller-a", "profile-1", 10).unwrap();
    let withdrawn = rbac.grant_profile_use("caller-a", "profile-2", 11).unwrap();
    rbac.revoke_profile_use(&withdrawn.grant_id, 12).unwrap();
    let grants_before = rbac.profile_use_grants().unwrap();
    let mut server = ServerState::new_for_test("admission-journey", IsolationLayer::new());
    server.persist_dir = Some(dir.path().to_str().unwrap().to_string());
    let backend = Arc::new(
        RedbBackend::open_with_shards(dir.path().to_str().unwrap().to_string(), 16, 1).unwrap(),
    );
    server.persistence = Some(backend.clone());
    server.isolation =
        IsolationLayer::with_policy_store(rbac.clone() as Arc<dyn RbacPolicyStore>).unwrap();
    let state = Arc::new(RwLock::new(server));
    let store = state.write().await.ensure_agent_library().unwrap();
    seed_library(&store);
    let record = decided(&store).record;
    let txn = |key: &str| format!("decision-commit:{}:{key}", record.record_id);
    let request = |key: &str, nonce: u8, catalog: &str| DecisionCommitRequest {
        context: commit_context(&store, key, nonce),
        expected_catalog_digest: catalog.to_string(),
        record: record.clone(),
    };
    let rbac_state = |txn_id: &str| {
        rbac.reservations()
            .unwrap()
            .into_iter()
            .find(|held| held.txn_id == txn_id)
            .map(|held| held.state)
    };

    // 1. An outside reservation of the authorization image refuses the commit by name,
    //    before the library reserves anything or any record is written.
    let revision = rbac.authority_snapshot().unwrap().revision();
    rbac.reserve(revision, "outside").unwrap();
    let blocked = handle_decision_commit(
        &state,
        1,
        &verified("blocked", 1),
        request("blocked", 1, &record.inputs.catalog_digest),
    )
    .await;
    assert_eq!(blocked.error.as_deref(), Some("RBAC_RESERVED"));
    assert!(store.admission(TENANT, &txn("blocked")).unwrap().is_none());
    assert!(store
        .current_component(TENANT, &record.record_id)
        .unwrap()
        .is_none());
    // The refused commit is decided ABORT, durably and with its certificate.
    assert_eq!(
        backend.xshard_decision_get(&txn("blocked")).unwrap(),
        Some(false)
    );
    rbac.release("outside", Some(ReservationEvent::Abort))
        .unwrap();

    // 2. A commit that fails after both reservations resolves both with ABORT.
    let failed = handle_decision_commit(
        &state,
        2,
        &verified("failed", 2),
        request("failed", 2, &format!("sha256:{}", "0".repeat(64))),
    )
    .await;
    assert!(failed.error.is_some() && failed.result.is_none());
    assert_eq!(failed.error.as_deref(), Some("STALE_CATALOG"));
    assert_eq!(
        backend.xshard_decision_get(&txn("failed")).unwrap(),
        Some(false)
    );
    assert_eq!(rbac_state(&txn("failed")), Some(ReservationState::Aborted));
    assert_eq!(
        store
            .admission(TENANT, &txn("failed"))
            .unwrap()
            .unwrap()
            .state,
        AdmissionState::Aborted
    );
    assert!(store
        .current_component(TENANT, &record.record_id)
        .unwrap()
        .is_none());

    // 3. The served commit reserves both owners, writes the record, and resolves both with COMMIT.
    let good = handle_decision_commit(
        &state,
        3,
        &verified("good", 3),
        request("good", 3, &record.inputs.catalog_digest),
    )
    .await;
    assert!(good.error.is_none(), "{:?}", good.error);
    let Some(ResultPayload::Raw(bytes)) = good.result else {
        panic!("commit result")
    };
    let committed: DecisionCommitResult = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(committed.record_id, record.record_id);
    assert_eq!(rbac_state(&txn("good")), Some(ReservationState::Committed));
    // The decision is durable COMMIT with a request-bound certificate the writer stamped.
    assert_eq!(
        backend.xshard_decision_get(&txn("good")).unwrap(),
        Some(true)
    );
    let certified = backend
        .xshard_certificate_get(&txn("good"), TENANT)
        .unwrap()
        .expect("certificate stored with the decision");
    assert_eq!(
        certified.request_digest,
        request_digest(&[
            "caller-a",
            TENANT,
            &record.record_id,
            &record.inputs.catalog_digest,
            &record.record_digest,
        ])
    );
    assert_eq!(
        certified.coordinator_scope,
        format!("decision-commit:{TENANT}")
    );
    assert!(certified.decided_ms > 0 && certified.decided_ms < certified.deadline_ms);
    let held = store.admission(TENANT, &txn("good")).unwrap().unwrap();
    assert_eq!(held.state, AdmissionState::Committed);
    assert_eq!(
        held.intent.write_key,
        format!("decision-record:{}", record.record_id)
    );
    assert_eq!(
        held.intent.component_heads.len(),
        record.inputs.candidates.len()
    );

    // 4. Nothing stays fenced: the image takes an ordinary checked save, and the layer's
    //    own revision followed the reservation writes.
    let snapshot = rbac.authority_snapshot().unwrap();
    rbac.compare_and_save(
        snapshot.revision(),
        snapshot.policy(),
        snapshot.identities(),
        snapshot.bootstrap(),
    )
    .unwrap();
    assert!(rbac
        .reservations()
        .unwrap()
        .iter()
        .all(|held| held.state != ReservationState::Reserved));

    // 4b. The reservation granted no profile access and changed no grant: the active grant is
    //     still the only one the caller may use, and the revoked one stayed revoked.
    assert_eq!(rbac.profile_use_grants().unwrap(), grants_before);
    let usable: Vec<_> = rbac
        .profile_use_grants()
        .unwrap()
        .into_iter()
        .filter(|grant| grant.is_active() && grant.principal == "caller-a")
        .map(|grant| grant.grant_id)
        .collect();
    assert_eq!(usable, vec![active.grant_id]);

    // 5. A repeat of the stored record replays without taking a new reservation.
    let before = (
        rbac.reservations().unwrap().len(),
        rbac.current_version().unwrap(),
    );
    let replay = handle_decision_commit(
        &state,
        4,
        &verified("good", 3),
        request("good", 3, &record.inputs.catalog_digest),
    )
    .await;
    assert!(replay.error.is_none(), "{:?}", replay.error);
    assert_eq!(
        (
            rbac.reservations().unwrap().len(),
            rbac.current_version().unwrap()
        ),
        before
    );

    // 6. An exact retry of an aborted request answers the original ABORT and changes nothing;
    //    a changed request under the same key is refused CERTIFICATE_INTENT_CHANGED.
    let stamped = backend
        .xshard_certificate_get(&txn("failed"), TENANT)
        .unwrap()
        .unwrap();
    assert!(stamped.decided_ms > 0);
    let wrong = format!("sha256:{}", "0".repeat(64));
    let retried = handle_decision_commit(
        &state,
        5,
        &verified("failed", 2),
        request("failed", 2, &wrong),
    )
    .await;
    assert_eq!(retried.error.as_deref(), Some("CERTIFICATE_DECIDED_ABORT"));
    let changed = handle_decision_commit(
        &state,
        6,
        &verified("failed", 2),
        request("failed", 2, &record.inputs.catalog_digest),
    )
    .await;
    assert_eq!(changed.error.as_deref(), Some("CERTIFICATE_INTENT_CHANGED"));
    assert_eq!(
        backend
            .xshard_certificate_get(&txn("failed"), TENANT)
            .unwrap()
            .unwrap(),
        stamped,
        "decided_ms and the request digest are unchanged"
    );
    assert_eq!(
        backend.xshard_decision_get(&txn("failed")).unwrap(),
        Some(false)
    );

    // 7. A COMMIT at or after the deadline refuses CERTIFICATE_EXPIRED: the coordinator records
    //    ABORT with the certificate, both owners resolve ABORT, nothing is written.
    let late = REQUEST_NOW_MS
        .scope(
            1,
            handle_decision_commit(
                &state,
                7,
                &verified("late", 4),
                request("late", 4, &record.inputs.catalog_digest),
            ),
        )
        .await;
    assert_eq!(late.error.as_deref(), Some("CERTIFICATE_EXPIRED"));
    assert_eq!(
        backend.xshard_decision_get(&txn("late")).unwrap(),
        Some(false)
    );
    assert!(
        backend
            .xshard_certificate_get(&txn("late"), TENANT)
            .unwrap()
            .unwrap()
            .decided_ms
            > 0
    );
    assert_eq!(rbac_state(&txn("late")), Some(ReservationState::Aborted));
    assert_eq!(
        store
            .admission(TENANT, &txn("late"))
            .unwrap()
            .unwrap()
            .state,
        AdmissionState::Aborted
    );

    // 8. The pending marker is observable before the decision, and a crash between the marker
    //    and the decision is finished on restart: both owners resolve ABORT from the durable
    //    record, never from a local result.
    let pending = verified("pending", 5);
    let catalog = record.inputs.catalog_digest.clone();
    let coordinator = Coordinator::begin(&state, &pending, &record, &catalog)
        .await
        .unwrap();
    assert_eq!(
        backend.xshard_decision_get(coordinator.txn_id()).unwrap(),
        None
    );
    assert_eq!(
        backend
            .xshard_certificate_get(coordinator.txn_id(), TENANT)
            .unwrap()
            .unwrap()
            .decided_ms,
        0
    );
    let held = Admitted::reserve(
        &state,
        &store,
        &pending,
        &record,
        &commit_context(&store, "pending", 5).policy_revision,
        coordinator.txn_id(),
    )
    .await
    .unwrap();
    assert_eq!(
        rbac_state(coordinator.txn_id()),
        Some(ReservationState::Reserved)
    );
    let recovered = coordinator.recover().await.unwrap();
    assert_eq!(recovered, OwnerDecision::Abort);
    held.resolve_decided(&state, &store, recovered)
        .await
        .unwrap();
    assert_eq!(
        backend.xshard_decision_get(coordinator.txn_id()).unwrap(),
        Some(false)
    );
    assert_eq!(
        rbac_state(coordinator.txn_id()),
        Some(ReservationState::Aborted)
    );
    assert_eq!(
        store
            .admission(TENANT, coordinator.txn_id())
            .unwrap()
            .unwrap()
            .state,
        AdmissionState::Aborted
    );

    // 9. A caller of another tenant is refused ACCESS_DENIED and leaves no decision or certificate.
    let foreign = VerifiedRequestContext::verified_for_test_in_tenant("caller-a", "foreign");
    let denied = handle_decision_commit(&state, 8, &foreign, request("foreign", 6, &catalog)).await;
    assert_eq!(denied.error.as_deref(), Some("ACCESS_DENIED"));
    assert_eq!(backend.xshard_decision_get(&txn("foreign")).unwrap(), None);
    assert!(backend
        .xshard_certificate_get(&txn("foreign"), "foreign")
        .unwrap()
        .is_none());
}
