//! MEDULLA-DECISION-ENGINE-R036.103 and R036.104: the cross-owner admission journey of one
//! served `DecisionCommit`, through the handler, against a durable RBAC store and a durable
//! Agent Library. MEDULLA-DECISION-ENGINE-R036.106: the same journey holds durable profile-use
//! grants, which the admission consults and a reservation never changes.

use std::sync::Arc;

use medulla_decision_types::decision::{DecisionCommitRequest, DecisionCommitResult};
use medulla_isolation::isolation::IsolationLayer;
use medulla_isolation::rbac_persist::RbacPolicyStore;
use medulla_persistence_records::agent_library::admission::AdmissionState;
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_persistence_records::decision_record::tests::{
    commit_context, decided, seed_library, TENANT,
};
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::reservation_machine::{ReservationEvent, ReservationState};
use tokio::sync::RwLock;

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
}
