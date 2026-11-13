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
use medulla_server_api::api::persistence::PersistenceBackend;
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
        topology: None,
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

/// A served state with a durable backend holding one capacity ledger graph, an Agent Library
/// and a solved topology assembly whose plan leases from `cell-llm`.
struct TopologyWorld {
    _dir: tempfile::TempDir,
    backend: Arc<RedbBackend>,
    state: crate::decide::SharedState,
    store: Arc<medulla_persistence_records::agent_library::AgentLibraryStore>,
    assembly: medulla_compute::assemble::Assembly,
}

const LEDGER: &str = "topology";

async fn topology_world() -> TopologyWorld {
    use medulla_mutation_types::capacity_lease::{CapacityCell, CapacityResourceClass};
    use medulla_work_types::native_control::{
        CapacityCellUpdateRequest, NativeControlSchemaVersion,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap().to_string();
    let backend = Arc::new(RedbBackend::open_with_shards(path.clone(), 16, 1).unwrap());
    backend
        .commit_capacity_lease(
            LEDGER,
            medulla_protocol_types::protocol::Method::UpdateCapacityCell {
                request: CapacityCellUpdateRequest {
                    schema_version: NativeControlSchemaVersion::V1,
                    cell: CapacityCell {
                        cell_id: "cell-llm".into(),
                        parent_id: None,
                        resource_class: CapacityResourceClass::LlmGenerator,
                        capacity: 8,
                        reserved_floor: 0,
                        epoch: 1,
                        policy_digest: "d".repeat(64),
                        updated_at_ms: 0,
                        throttle: None,
                    },
                    expected_epoch: None,
                    now_ms: 1,
                },
            },
        )
        .await
        .unwrap();
    let mut server = ServerState::new_for_test("topology-world", IsolationLayer::new());
    server.persist_dir = Some(path);
    server.persistence = Some(backend.clone());
    server
        .registry
        .create_graph(
            LEDGER,
            medulla_protocol_types::protocol::GraphType::Global,
            None,
        )
        .expect("the ledger graph is registered");
    let state = Arc::new(RwLock::new(server));
    let store = state.write().await.ensure_agent_library().unwrap();
    seed_library(&store);
    let assembly = crate::decide::commit::tests::topology_assembly(&store);
    TopologyWorld {
        _dir: dir,
        backend,
        state,
        store,
        assembly,
    }
}

impl TopologyWorld {
    /// Stop the engine and open the same stores again, as a restart does.
    async fn reopen(self) -> TopologyWorld {
        let TopologyWorld {
            _dir: dir,
            backend,
            state,
            store,
            assembly,
        } = self;
        let path = dir.path().to_str().unwrap().to_string();
        drop(store);
        drop(state);
        drop(backend);
        let backend = Arc::new(RedbBackend::open_with_shards(path.clone(), 16, 1).unwrap());
        let mut server = ServerState::new_for_test("topology-world", IsolationLayer::new());
        server.persist_dir = Some(path);
        server.persistence = Some(backend.clone());
        server
            .registry
            .create_graph(
                LEDGER,
                medulla_protocol_types::protocol::GraphType::Global,
                None,
            )
            .expect("the ledger graph is registered");
        let state = Arc::new(RwLock::new(server));
        let store = state.write().await.ensure_agent_library().unwrap();
        TopologyWorld {
            _dir: dir,
            backend,
            state,
            store,
            assembly,
        }
    }

    /// The leases the ledger still holds for the plan's cell.
    async fn active_leases(&self) -> usize {
        self.leases()
            .await
            .iter()
            .filter(|lease| {
                lease.state == medulla_mutation_types::capacity_lease::CapacityLeaseState::Active
            })
            .count()
    }

    fn request(&self, key: &str, nonce: u8, graph_nonce: u8) -> DecisionCommitRequest {
        let record = self.assembly.record.clone();
        DecisionCommitRequest {
            context: commit_context(&self.store, key, nonce),
            expected_catalog_digest: record.inputs.catalog_digest.clone(),
            record,
            topology: Some(medulla_decision_types::decision::TopologyCommitEvidence {
                capacity_graph: LEDGER.into(),
                evidence: medulla_persistence_records::decision_record::tests::evidence_request(
                    &self.store,
                    &self.assembly,
                    &format!("{key}-graph"),
                    graph_nonce,
                ),
            }),
        }
    }

    async fn leases(&self) -> Vec<medulla_mutation_types::capacity_lease::CapacityLease> {
        use medulla_work_types::native_control::{
            CapacityStatusRequest, NativeControlSchemaVersion,
        };
        self.backend
            .read_capacity_status(
                LEDGER,
                &CapacityStatusRequest {
                    schema_version: NativeControlSchemaVersion::V1,
                    tenant_ref: TENANT.into(),
                    cell_id: Some("cell-llm".into()),
                    lease_id: None,
                    max_count: 50,
                    cursor: None,
                },
            )
            .await
            .unwrap()
            .leases
    }
}

fn decode_commit(response: medulla_protocol_types::protocol::Response) -> DecisionCommitResult {
    assert!(response.error.is_none(), "{:?}", response.error);
    let Some(ResultPayload::Raw(bytes)) = response.result else {
        panic!("commit result")
    };
    rmp_serde::from_slice(&bytes).unwrap()
}

// spec: MEDULLA-DECISION-ENGINE-R113.2.5
#[tokio::test]
async fn topology_shared_commit_journey() {
    #[cfg(any(feature = "security", feature = "raft"))]
    let _env_guard = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let world = topology_world().await;
    let record = world.assembly.record.clone();
    let demands = crate::decide::commit::capacity::demands(&record);
    assert!(!demands.is_empty());

    // 1. A capacity-backed plan without its capacity graph and evidence graph is refused before
    //    any write: no record, no lease.
    let mut bare = world.request("bare", 1, 61);
    bare.topology = None;
    let refused = handle_decision_commit(&world.state, 1, &verified("bare", 1), bare).await;
    assert_eq!(refused.error.as_deref(), Some("DERIVATION_REJECTED"));
    assert!(world
        .store
        .current_component(TENANT, &record.record_id)
        .unwrap()
        .is_none());
    assert!(world.leases().await.is_empty());

    // 2. The ledger graph is a graph the caller may write: an unknown graph refuses and nothing
    //    is written.
    let mut elsewhere = world.request("elsewhere", 2, 63);
    elsewhere.topology.as_mut().unwrap().capacity_graph = "not-a-graph".into();
    let refused =
        handle_decision_commit(&world.state, 2, &verified("elsewhere", 2), elsewhere).await;
    assert!(refused.error.is_some(), "{:?}", refused.error);
    assert!(world.leases().await.is_empty());

    // 3. A caller of another tenant is refused before any write.
    let foreign = VerifiedRequestContext::verified_for_test_in_tenant("caller-a", "foreign");
    let refused =
        handle_decision_commit(&world.state, 3, &foreign, world.request("foreign", 3, 65)).await;
    assert_eq!(refused.error.as_deref(), Some("ACCESS_DENIED"));
    assert!(world.leases().await.is_empty());

    // 4. The served commit makes the record, the evidence graph and the lease visible together,
    //    and the coordinator decision is a durable COMMIT.
    let good = world.request("good", 4, 67);
    let graph_id = good
        .topology
        .as_ref()
        .unwrap()
        .evidence
        .graph
        .graph_id
        .clone();
    let committed = decode_commit(
        handle_decision_commit(&world.state, 4, &verified("good", 4), good.clone()).await,
    );
    assert!(!committed.replayed);
    assert_eq!(committed.record_id, record.record_id);
    assert!(world
        .store
        .current_component(TENANT, &record.record_id)
        .unwrap()
        .is_some());
    let evidence = world
        .store
        .current_graph(TENANT, &graph_id)
        .unwrap()
        .expect("the evidence graph is published with the record");
    let outcome = committed.topology.expect("topology outcome");
    assert_eq!(outcome.evidence_graph_id, graph_id);
    assert_eq!(outcome.evidence_revision, evidence.entry_revision);
    let leases = world.leases().await;
    assert_eq!(leases.len(), demands.len());
    assert_eq!(
        outcome.premises,
        leases
            .iter()
            .map(|lease| format!("lease:{}", lease.lease_id))
            .collect::<Vec<_>>()
    );
    let txn = format!("decision-commit:{}:good", record.record_id);
    assert_eq!(world.backend.xshard_decision_get(&txn).unwrap(), Some(true));

    // 5. A repeat of the decided commit is refused with its original outcome (R113.2.4): no
    //    second lease, no second evidence revision.
    let again = handle_decision_commit(&world.state, 5, &verified("good", 4), good).await;
    assert_eq!(again.error.as_deref(), Some("COMMIT_ALREADY_DECIDED"));
    assert!(again
        .error_detail
        .as_deref()
        .is_some_and(|detail| detail.contains(&record.record_id) && detail.contains(&graph_id)));
    assert_eq!(world.leases().await.len(), demands.len());
    assert_eq!(
        world
            .store
            .current_graph(TENANT, &graph_id)
            .unwrap()
            .unwrap()
            .entry_revision,
        evidence.entry_revision
    );

    // 6. With no capacity authority configured, a capacity-backed record is refused by the
    //    narrowed R113.2.1 guard, before any owner changes.
    world.state.write().await.persistence = None;
    let guarded = handle_decision_commit(
        &world.state,
        6,
        &verified("guarded", 6),
        world.request("guarded", 6, 69),
    )
    .await;
    assert_eq!(guarded.error.as_deref(), Some("DERIVATION_REJECTED"));
    assert_eq!(
        guarded.error_detail.as_deref(),
        Some("topology capacity acquisition requires shared transaction authority")
    );
}

// spec: MEDULLA-DECISION-ENGINE-R113.2.2
#[tokio::test]
async fn topology_commit_publishes_capacity_and_evidence() {
    #[cfg(any(feature = "security", feature = "raft"))]
    let _env_guard = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let world = topology_world().await;
    let record = world.assembly.record.clone();
    let demands = crate::decide::commit::capacity::demands(&record);

    // The ledger's own accepting decision builds the typed commit outcome the row names.
    let request = world.request("accepted", 7, 71);
    let graph_id = request
        .topology
        .as_ref()
        .unwrap()
        .evidence
        .graph
        .graph_id
        .clone();
    let committed = decode_commit(
        handle_decision_commit(&world.state, 1, &verified("accepted", 7), request.clone()).await,
    );

    // Record, lease and evidence node are all visible.
    let stored = world
        .store
        .current_component(TENANT, &record.record_id)
        .unwrap()
        .expect("the decision record is visible");
    assert_eq!(stored.component_id, record.record_id);
    let leases = world.leases().await;
    assert_eq!(leases.len(), demands.len());
    for (lease, demand) in leases.iter().zip(demands.iter()) {
        assert_eq!(lease.cell_id, demand.cell_id);
        assert_eq!(lease.amount, demand.amount);
        assert_eq!(lease.tenant_ref, TENANT);
    }
    let evidence = world
        .store
        .current_graph(TENANT, &graph_id)
        .unwrap()
        .expect("the evidence node is visible");
    assert_eq!(
        evidence.synthesis_evidence.as_ref().unwrap().component_id,
        record.record_id
    );

    // The typed outcome agrees with the three outputs, and the premises name every lease.
    let lease_ids: Vec<String> = leases.iter().map(|lease| lease.lease_id.clone()).collect();
    let outcome = medulla_agent_types::agent_topology::capacity_commit::CapacitySynthesisCommit::from_capacity_decision(
        medulla_work_types::native_control::CapacityDecision::Accepted,
        lease_ids.clone(),
        true,
    )
    .expect("an accepted decision with leases and evidence commits");
    assert_eq!(
        outcome,
        medulla_agent_types::agent_topology::capacity_commit::CapacitySynthesisCommit::Committed {
            lease_ids: lease_ids.clone(),
            synthesis_evidence_published: true,
        }
    );
    let published = committed.topology.expect("topology outcome");
    assert_eq!(published.evidence_graph_id, graph_id);
    assert_eq!(
        published.premises,
        lease_ids
            .iter()
            .map(|id| format!("lease:{id}"))
            .collect::<Vec<_>>()
    );

    // A cross-tenant read returns nothing: the other tenant sees no record, no evidence and
    // no lease.
    assert!(world
        .store
        .current_component("foreign", &record.record_id)
        .unwrap()
        .is_none());
    assert!(world
        .store
        .current_graph("foreign", &graph_id)
        .unwrap()
        .is_none());
    let foreign_leases = world
        .backend
        .read_capacity_status(
            LEDGER,
            &medulla_work_types::native_control::CapacityStatusRequest {
                schema_version: medulla_work_types::native_control::NativeControlSchemaVersion::V1,
                tenant_ref: "foreign".into(),
                cell_id: Some("cell-llm".into()),
                lease_id: None,
                max_count: 50,
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert!(foreign_leases.leases.is_empty());
}

// spec: MEDULLA-DECISION-ENGINE-R113.2.3
#[tokio::test]
async fn topology_capacity_denial_leaves_no_commit_parts() {
    use medulla_mutation_types::capacity_lease::{CapacityResourceClass, LeasePriority};
    use medulla_work_types::native_control::{
        CapacityAcquireRequest, CapacityAcquireResult, CapacityDecision, CapacityDemand,
        NativeControlSchemaVersion,
    };
    #[cfg(any(feature = "security", feature = "raft"))]
    let _env_guard = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let world = topology_world().await;
    let record = world.assembly.record.clone();

    // Another holder takes the whole cell, so the plan's lease is denied by the ledger itself.
    let blocker = world
        .backend
        .commit_capacity_lease(
            LEDGER,
            medulla_protocol_types::protocol::Method::AcquireCapacity {
                request: CapacityAcquireRequest {
                    schema_version: NativeControlSchemaVersion::V1,
                    tenant_ref: TENANT.into(),
                    work_item_id: "blocker".into(),
                    owner_digest: "other-owner".into(),
                    idempotency_key: "blocker".into(),
                    priority: LeasePriority::BackgroundIngestion,
                    demands: vec![CapacityDemand {
                        cell_id: "cell-llm".into(),
                        resource_class: CapacityResourceClass::LlmGenerator,
                        amount: 8,
                    }],
                    lease_id: None,
                    ttl_ms: 600_000,
                    now_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
                    cost_budget_micros: None,
                    token_budget: None,
                },
            },
        )
        .await
        .unwrap();
    let blocker: CapacityAcquireResult = rmp_serde::from_slice(&blocker).unwrap();
    assert_eq!(blocker.decision, CapacityDecision::Accepted);
    let held_before = world.leases().await;
    assert_eq!(held_before.len(), 1);

    let request = world.request("denied", 8, 73);
    let graph_id = request
        .topology
        .as_ref()
        .unwrap()
        .evidence
        .graph
        .graph_id
        .clone();
    let denied =
        handle_decision_commit(&world.state, 1, &verified("denied", 8), request.clone()).await;

    // The stable code answers, with the one-time retry allowance of `RetryAllowance`.
    assert_eq!(denied.error.as_deref(), Some("CAPACITY_DENIED"));
    assert!(denied.result.is_none());
    assert!(
        denied
            .error_detail
            .as_deref()
            .is_some_and(|detail| detail.contains("retry allowance 1")),
        "{:?}",
        denied.error_detail
    );
    assert!(
        medulla_agent_types::agent_topology::enforcement::REFUSALS
            .iter()
            .any(|code| code.as_str() == "CAPACITY_DENIED"),
        "the code is registered in the topology family refusals"
    );

    // Nothing was written: no new record, no new lease, no evidence graph.
    assert!(world
        .store
        .current_component(TENANT, &record.record_id)
        .unwrap()
        .is_none());
    assert!(world
        .store
        .current_graph(TENANT, &graph_id)
        .unwrap()
        .is_none());
    assert_eq!(world.leases().await, held_before);

    // The coordinator decided ABORT durably and the Agent Library reservation was released.
    let txn = format!("decision-commit:{}:denied", record.record_id);
    assert_eq!(
        world.backend.xshard_decision_get(&txn).unwrap(),
        Some(false)
    );
    assert_eq!(
        world
            .store
            .admission(TENANT, &txn)
            .unwrap()
            .expect("the library reserved before the lease was asked")
            .state,
        AdmissionState::Aborted
    );
}

// spec: MEDULLA-DECISION-ENGINE-R113.2.4
#[tokio::test]
async fn topology_commit_recovery_preserves_atomic_outcome() {
    use crate::decide::commit::{CrashPoint, CRASH_AT};
    #[cfg(any(feature = "security", feature = "raft"))]
    let _env_guard = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    for (index, point) in [CrashPoint::Prepare, CrashPoint::Decision, CrashPoint::Apply]
        .into_iter()
        .enumerate()
    {
        let key = format!("crash-{index}");
        let nonce = 20 + index as u8;
        let world = topology_world().await;
        let record = world.assembly.record.clone();
        let demands = crate::decide::commit::capacity::demands(&record).len();
        let request = world.request(&key, nonce, 80 + index as u8);
        let graph_id = request
            .topology
            .as_ref()
            .unwrap()
            .evidence
            .graph
            .graph_id
            .clone();
        let txn = format!("decision-commit:{}:{key}", record.record_id);

        // The process stops at the participant boundary: no cleanup runs.
        let crashed = CRASH_AT
            .scope(
                point,
                handle_decision_commit(&world.state, 1, &verified(&key, nonce), request.clone()),
            )
            .await;
        assert_eq!(
            crashed.error.as_deref(),
            Some("DECISION_COORDINATOR_UNAVAILABLE"),
            "{point:?}"
        );

        // Reopen every store, as a restart does, and read what the crash left behind.
        let world = world.reopen().await;
        let decision = world.backend.xshard_decision_get(&txn).unwrap();
        let record_stored = |world: &TopologyWorld| {
            world
                .store
                .current_component(TENANT, &record.record_id)
                .unwrap()
                .is_some()
        };
        let evidence_stored = |world: &TopologyWorld| {
            world
                .store
                .current_graph(TENANT, &graph_id)
                .unwrap()
                .is_some()
        };
        let admission = |world: &TopologyWorld| world.store.admission(TENANT, &txn).unwrap();
        match point {
            CrashPoint::Prepare => {
                assert_eq!(decision, None);
                assert!(!record_stored(&world) && !evidence_stored(&world));
                assert_eq!(world.active_leases().await, demands);
                assert_eq!(admission(&world).unwrap().state, ReservationState::Reserved);
            }
            CrashPoint::Decision => {
                assert_eq!(decision, Some(true));
                assert!(!record_stored(&world) && !evidence_stored(&world));
                assert_eq!(world.active_leases().await, demands);
                assert_eq!(admission(&world).unwrap().state, ReservationState::Reserved);
            }
            CrashPoint::Apply => {
                assert_eq!(decision, Some(true));
                assert!(record_stored(&world) && evidence_stored(&world));
                assert_eq!(world.active_leases().await, demands);
                assert_eq!(admission(&world).unwrap().state, ReservationState::Reserved);
            }
        }

        // The same request is presented again and finishes the transaction.
        let retry = match point {
            // An undecided transaction is finished by presumed abort once its deadline passed.
            CrashPoint::Prepare => {
                REQUEST_NOW_MS
                    .scope(
                        u64::MAX / 2,
                        handle_decision_commit(
                            &world.state,
                            2,
                            &verified(&key, nonce),
                            request.clone(),
                        ),
                    )
                    .await
            }
            _ => {
                handle_decision_commit(&world.state, 2, &verified(&key, nonce), request.clone())
                    .await
            }
        };
        match point {
            CrashPoint::Prepare => {
                assert_eq!(retry.error.as_deref(), Some("CERTIFICATE_DECIDED_ABORT"));
                assert_eq!(
                    world.backend.xshard_decision_get(&txn).unwrap(),
                    Some(false)
                );
                assert_eq!(admission(&world).unwrap().state, ReservationState::Aborted);
                assert!(!record_stored(&world) && !evidence_stored(&world));
                assert_eq!(world.active_leases().await, 0);
            }
            CrashPoint::Decision => {
                let committed = decode_commit(retry);
                assert!(!committed.replayed);
                assert_eq!(committed.record_id, record.record_id);
                assert_eq!(committed.topology.unwrap().evidence_graph_id, graph_id);
                assert_eq!(world.backend.xshard_decision_get(&txn).unwrap(), Some(true));
                assert_eq!(
                    admission(&world).unwrap().state,
                    ReservationState::Committed
                );
                assert!(record_stored(&world) && evidence_stored(&world));
                assert_eq!(world.active_leases().await, demands);
            }
            CrashPoint::Apply => {
                assert_eq!(retry.error.as_deref(), Some("COMMIT_ALREADY_DECIDED"));
                assert_eq!(world.backend.xshard_decision_get(&txn).unwrap(), Some(true));
                assert_eq!(
                    admission(&world).unwrap().state,
                    ReservationState::Committed
                );
                assert!(record_stored(&world) && evidence_stored(&world));
                assert_eq!(world.active_leases().await, demands);
            }
        }

        // Never a record without its lease, or a lease without its record.
        let active = world.active_leases().await;
        assert_eq!(record_stored(&world), active == demands, "{point:?}");
        assert_eq!(record_stored(&world), evidence_stored(&world), "{point:?}");
        assert!(active == 0 || active == demands, "{point:?}");

        // A repeat of a decided transaction is refused with the original outcome and takes no
        // second lease.
        if point != CrashPoint::Prepare {
            let repeated =
                handle_decision_commit(&world.state, 3, &verified(&key, nonce), request.clone())
                    .await;
            assert_eq!(repeated.error.as_deref(), Some("COMMIT_ALREADY_DECIDED"));
            assert!(repeated
                .error_detail
                .as_deref()
                .is_some_and(|detail| detail.contains(&record.record_id)));
            assert_eq!(world.leases().await.len(), demands);
        }
    }
}
