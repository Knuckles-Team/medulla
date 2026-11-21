//! The served guard must refuse before either durable owner changes.

#[cfg(feature = "redb")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use std::sync::Arc;

use medulla_agent_types::agent_fixtures::topology::facts;
use medulla_agent_types::agent_fixtures::topology::requirements;
use medulla_agent_types::agent_fixtures::topology::FAN_OUT_JOIN;
use medulla_agent_types::agent_fixtures::topology::INDEPENDENT_SUBTASKS;
use medulla_agent_types::agent_topology::CapacityHeadroom;
use medulla_agent_types::agent_topology::SchemaAuthority;
use medulla_agent_types::agent_topology::TopologyAdmission;
use medulla_agent_types::agent_topology::TopologyInputs;
use medulla_agent_types::delegation::AgentGraphEntryRef;
use medulla_compute::assemble::{assemble, inputs, replay_check, RecordIdentity};
use medulla_decision_types::decision::DecisionCommitRequest;
use medulla_decision_types::decision::DecisionCommitResult;
use medulla_decision_types::decision::TemplateFacts;
use medulla_mutation_types::{capacity_lease::CapacityCell, capacity_lease::CapacityResourceClass};
use medulla_types::contract::BoundedVec;
use medulla_work_types::{
    native_control::CapacityCellUpdateRequest, native_control::CapacityStatusRequest,
    native_control::NativeControlSchemaVersion,
};
use tokio::sync::RwLock;

use medulla_persistence::redb_backend::RedbBackend;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_record::tests::commit_context;
use medulla_persistence_records::decision_record::tests::decided;
use medulla_persistence_records::decision_record::tests::seed_library;
use medulla_persistence_records::decision_record::tests::TENANT;
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

pub(in crate::decide) fn topology_assembly(
    store: &AgentLibraryStore,
) -> medulla_compute::assemble::Assembly {
    let base = decided(store, TENANT);
    let mut topology = facts();
    let base_graph = base.graph.unwrap();
    let shape = base_graph.shape.clone();
    let mut slot = topology.slots.as_slice()[0].clone();
    slot.node_id = shape.entry_node.clone();
    topology.slots = BoundedVec::new(vec![slot]).unwrap();
    // The template is a published library graph: the commit re-reads every template the record
    // names inside its write (STALE_CATALOG when it is gone), so it is published the way an
    // operator publishes one and the record names the stored revision.
    // The template's shape pins the assembled agent: publish it first, as
    // `evidence_request` does (which then reuses the published entry).
    store
        .publish(
            medulla_agent_types::agent_library::AgentLibraryPublishRequest {
                context: medulla_persistence_records::agent_fixtures::mutation_context(
                    store,
                    TENANT,
                    "topology-template-agent",
                    199,
                    0,
                    "agent-library:publish",
                ),
                entry: base
                    .agents
                    .first()
                    .cloned()
                    .expect("solved answers an agent"),
            },
        )
        .expect("the assembled agent publishes");
    let mut draft = base_graph;
    draft.graph_id = "topology-template".into();
    draft.synthesis_evidence = None;
    draft.topology = Some(topology);
    let entry = store
        .publish_graph(medulla_agent_types::agent_graph::AgentGraphPublishRequest {
            context: medulla_persistence_records::agent_fixtures::mutation_context(
                store,
                TENANT,
                "topology-template",
                200,
                0,
                "agent-graph:publish",
            ),
            graph: draft,
        })
        .expect("the topology template publishes")
        .result
        .graph;
    let template = TemplateFacts {
        graph_id: entry.graph_id.clone(),
        entry_revision: entry.entry_revision,
        definition_digest: entry.definition_digest.clone(),
        shape: entry.shape.clone(),
        topology: entry.topology.clone(),
    };
    let mut request = base.record.inputs.request;
    request.requirements.topology = Some(requirements(1));
    request.templates = BoundedVec::new(vec![AgentGraphEntryRef {
        tenant_id: TENANT.into(),
        graph_id: entry.graph_id.clone(),
        entry_revision: entry.entry_revision,
        definition_digest: entry.definition_digest.clone(),
        actor_scope: entry.actor_scope.clone(),
        purpose_id: entry.purpose_id.clone(),
        policy_digest: entry.policy_digest.clone(),
        composed_work_ceiling: entry.composed_work_ceiling,
    }])
    .unwrap();
    let mut input = inputs(
        request,
        base.record.inputs.candidates.as_slice().to_vec(),
        vec![template],
        base.record.inputs.policy,
    )
    .unwrap();
    input.topology = Some(TopologyInputs {
        schema_digest: format!("sha256:{}", "ab".repeat(32)),
        admissions: BoundedVec::new(vec![TopologyAdmission {
            task_class: INDEPENDENT_SUBTASKS.into(),
            topology_class: FAN_OUT_JOIN.into(),
            admit_class: format!("{FAN_OUT_JOIN}Admission"),
            source_key: "swarm-topology".into(),
            authority: SchemaAuthority::Admin,
            axioms: BoundedVec::default(),
        }])
        .unwrap(),
        verify_required_by: BoundedVec::default(),
        headroom: BoundedVec::new(vec![CapacityHeadroom {
            cell_id: "cell-llm".into(),
            class: CapacityResourceClass::LlmGenerator,
            available: 8,
            epoch: 1,
        }])
        .unwrap(),
    });
    let assembly = assemble(
        input,
        RecordIdentity {
            tenant_id: TENANT.into(),
            caller_principal: "caller-a".into(),
            created_at_ms: 20,
        },
    )
    .unwrap();
    let record = &assembly.record;
    replay_check(record).expect("genuine topology record replays");
    assert!(record
        .topology_plan()
        .unwrap()
        .lease
        .per_cell
        .iter()
        .any(|cell| cell.amount > 0));
    assembly
}

fn owner_bytes(dir: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "redb")
        })
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

// spec: MEDULLA-DECISION-ENGINE-R113.2.1
#[tokio::test]
async fn topology_commit_refuses_uncoordinated_owners() {
    #[cfg(any(feature = "security", feature = "raft"))]
    let _env_guard = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap().to_string();
    let backend = Arc::new(RedbBackend::open_with_shards(path.clone(), 16, 1).unwrap());
    backend
        .commit_capacity_lease(
            "topology",
            Method::UpdateCapacityCell {
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
    let mut server = ServerState::new_for_test(
        "commit-guard",
        medulla_isolation::isolation::IsolationLayer::new(),
    );
    server.persist_dir = Some(path);
    // No capacity authority is configured on the served state, so the topology guard applies.
    server.persistence = None;
    let state = Arc::new(RwLock::new(server));
    let store = state.write().await.ensure_agent_library().unwrap();
    seed_library(&store, TENANT);
    let record = topology_assembly(&store).record;
    let request = DecisionCommitRequest {
        context: commit_context(&store, TENANT, "topology-commit", 1),
        expected_catalog_digest: record.inputs.catalog_digest.clone(),
        record: record.clone(),
        topology: None,
        retry_of: None,
        approval: None,
    };
    let status = CapacityStatusRequest {
        schema_version: NativeControlSchemaVersion::V1,
        tenant_ref: TENANT.into(),
        cell_id: Some("cell-llm".into()),
        lease_id: None,
        max_count: 10,
        cursor: None,
    };
    let capacity_before = backend
        .read_capacity_status("topology", &status)
        .await
        .unwrap();
    let before = owner_bytes(dir.path());
    assert!(before.len() >= 2, "both durable owner files exist");
    let claims = VerifiedRequestContext::verified_for_test_in_tenant("caller-a", TENANT);
    let verified = VerifiedRequestContext::from_verified_claims_with_nonce(
        claims.claims().clone(),
        "ordinary-commit".into(),
        Some(medulla_types::contract::Nonce::from_bytes([3; 32])),
    );
    let response = super::handle_decision_commit(&state, 1, &verified, request.clone()).await;
    assert_eq!(response.error.as_deref(), Some("DERIVATION_REJECTED"));
    assert_eq!(
        response.error_detail.as_deref(),
        Some("topology capacity acquisition requires shared transaction authority")
    );
    assert!(response.result.is_none());
    assert_eq!(
        owner_bytes(dir.path()),
        before,
        "neither owner writes, including receipts and outbox"
    );
    assert_eq!(
        backend
            .read_capacity_status("topology", &status)
            .await
            .unwrap(),
        capacity_before
    );
    assert!(store
        .current_component(TENANT, &record.record_id)
        .unwrap()
        .is_none());
    let foreign = VerifiedRequestContext::verified_for_test_in_tenant("caller-a", "foreign");
    let response = super::handle_decision_commit(&state, 2, &foreign, request).await;
    assert_eq!(response.error.as_deref(), Some("ACCESS_DENIED"));
    assert_eq!(owner_bytes(dir.path()), before);

    // With the durable backend configured, the same served path accepts an ordinary record.
    state.write().await.persistence = Some(backend.clone());
    let plain = decided(&store, TENANT).record;
    let response = super::handle_decision_commit(
        &state,
        3,
        &verified,
        DecisionCommitRequest {
            context: commit_context(&store, TENANT, "ordinary-commit", 3),
            expected_catalog_digest: plain.inputs.catalog_digest.clone(),
            record: plain.clone(),
            topology: None,
            retry_of: None,
            approval: None,
        },
    )
    .await;
    assert!(response.error.is_none(), "{:?}", response.error);
    let Some(ResultPayload::Raw(bytes)) = response.result else {
        panic!("commit result")
    };
    let committed: DecisionCommitResult = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(committed.record_id, plain.record_id);
    assert!(store
        .current_component(TENANT, &plain.record_id)
        .unwrap()
        .is_some());
    assert_eq!(
        backend
            .read_capacity_status("topology", &status)
            .await
            .unwrap(),
        capacity_before
    );
}

// spec: MEDULLA-DECISION-ENGINE-R113.3.3
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn topology_denial_accepts_one_followup_request() {
    use crate::decide::commit::handle_decision_commit;
    use crate::decide::commit::retry::allowance_context;
    use crate::decide::stat_tests::commit_admission_tests::{
        block_cell, decode_commit, topology_world, verified,
    };
    use medulla_persistence_records::decision_record::retry_allowance::DenialAllowanceState;
    use medulla_persistence_records::decision_record::tests::decided_at;

    #[cfg(any(feature = "security", feature = "raft"))]
    let _env_guard = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let world = topology_world().await;
    let denied_id = world.assembly.record.record_id.clone();
    block_cell(&world).await;
    let followup = |record: medulla_decision_types::decision::DecisionRecord,
                    key: &str,
                    nonce: u8| DecisionCommitRequest {
        context: commit_context(&world.store, TENANT, key, nonce),
        expected_catalog_digest: record.inputs.catalog_digest.clone(),
        record,
        topology: None,
        retry_of: Some(denied_id.clone()),
        approval: None,
    };

    // A commit that is itself a retry grants no further allowance when the ledger denies it.
    let earlier = "decision:earlier-denial";
    let issue = allowance_context(
        &world.store,
        1,
        &verified("seed-earlier", 40),
        &commit_context(&world.store, TENANT, "seed-earlier", 40),
        "issue",
    )
    .unwrap();
    world.store.issue_denial_allowance(&issue, earlier).unwrap();
    let mut retried = world.request("retried", 41, 74);
    retried.retry_of = Some(earlier.to_string());
    let response = handle_decision_commit(&world.state, 1, &verified("retried", 41), retried).await;
    assert_eq!(response.error.as_deref(), Some("CAPACITY_DENIED"));
    assert_eq!(
        world.store.denial_allowance_state(TENANT, earlier),
        Ok(Some(DenialAllowanceState::Consumed))
    );
    assert_eq!(
        world.store.denial_allowance_state(TENANT, &denied_id),
        Ok(None),
        "a denied retry grants nothing"
    );

    // The denial itself grants the one further decision, as an allowance row and no record.
    let denied = handle_decision_commit(
        &world.state,
        2,
        &verified("denied", 8),
        world.request("denied", 8, 73),
    )
    .await;
    assert_eq!(denied.error.as_deref(), Some("CAPACITY_DENIED"));
    assert_eq!(
        world.store.denial_allowance_state(TENANT, &denied_id),
        Ok(Some(DenialAllowanceState::Issued))
    );
    assert!(world
        .store
        .current_component(TENANT, &denied_id)
        .unwrap()
        .is_none());

    // Two follow-ups race for the one allowance: exactly one is accepted.
    let first = decided_at(&world.store, TENANT, 21).record;
    let second = decided_at(&world.store, TENANT, 22).record;
    let third = decided_at(&world.store, TENANT, 23).record;
    let (first_request, second_request) = (
        followup(first.clone(), "retry-1", 51),
        followup(second.clone(), "retry-2", 52),
    );
    let (a, b) = (world.state.clone(), world.state.clone());
    let task_a = tokio::spawn(async move {
        handle_decision_commit(&a, 3, &verified("retry-1", 51), first_request).await
    });
    let task_b = tokio::spawn(async move {
        handle_decision_commit(&b, 4, &verified("retry-2", 52), second_request).await
    });
    let (a, b) = (task_a.await.unwrap(), task_b.await.unwrap());
    let (won, lost, winner, loser) = if a.error.is_none() {
        (a, b, first, second)
    } else {
        (b, a, second, first)
    };
    let committed = decode_commit(won);
    assert_eq!(committed.record_id, winner.record_id);
    assert_eq!(lost.error.as_deref(), Some("RETRY_ALLOWANCE_EXHAUSTED"));
    assert!(lost.result.is_none());
    assert!(world
        .store
        .current_component(TENANT, &winner.record_id)
        .unwrap()
        .is_some());
    assert!(world
        .store
        .current_component(TENANT, &loser.record_id)
        .unwrap()
        .is_none());
    assert_eq!(
        world.store.denial_allowance_state(TENANT, &denied_id),
        Ok(Some(DenialAllowanceState::Consumed))
    );

    // Replay: the accepted follow-up, sent again, is an idempotent replay that consumes nothing.
    let (winner_key, winner_nonce) = if winner.created_at_ms == 21 {
        ("retry-1", 51)
    } else {
        ("retry-2", 52)
    };
    let replay = handle_decision_commit(
        &world.state,
        5,
        &verified(winner_key, winner_nonce),
        followup(winner, winner_key, winner_nonce),
    )
    .await;
    assert!(decode_commit(replay).replayed);

    // A later, different follow-up naming the same denied decision is refused.
    let again = handle_decision_commit(
        &world.state,
        6,
        &verified("retry-3", 53),
        followup(third.clone(), "retry-3", 53),
    )
    .await;
    assert_eq!(again.error.as_deref(), Some("RETRY_ALLOWANCE_EXHAUSTED"));
    assert!(world
        .store
        .current_component(TENANT, &third.record_id)
        .unwrap()
        .is_none());

    // Tenant binding: another tenant's request is refused ACCESS_DENIED, and that tenant holds
    // no allowance under the denied decision.
    let foreign = VerifiedRequestContext::verified_for_test_in_tenant("caller-a", "foreign");
    let refused =
        handle_decision_commit(&world.state, 7, &foreign, followup(third, "retry-4", 54)).await;
    assert_eq!(refused.error.as_deref(), Some("ACCESS_DENIED"));
    assert_eq!(
        world.store.denial_allowance_state("foreign", &denied_id),
        Ok(None)
    );
}

// spec: MEDULLA-DECISION-ENGINE-R113.3.2
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn topology_denial_allowance_is_applied_by_the_recovered_abort() {
    use super::admission::Admitted;
    use super::certified::Coordinator;
    use crate::decide::commit::handle_decision_commit;
    use crate::decide::stat_tests::commit_admission_tests::{topology_world, verified};
    use medulla_persistence_records::decision_record::retry_allowance::DenialAllowanceState;

    #[cfg(any(feature = "security", feature = "raft"))]
    let _env_guard = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let world = topology_world().await;
    let record = world.assembly.record.clone();
    let denied_id = record.record_id.clone();
    let caller = verified("crashed", 9);
    let catalog = record.inputs.catalog_digest.clone();

    // The denial stages its allowance, the coordinator decides ABORT durably, and the process
    // stops before either owner resolves.
    let coordinator = Coordinator::begin(&world.state, &caller, &record, &catalog)
        .await
        .unwrap();
    let _held = Admitted::reserve(
        &world.state,
        &world.store,
        &caller,
        &record,
        &commit_context(&world.store, TENANT, "crashed", 9).policy_revision,
        coordinator.txn_id(),
    )
    .await
    .unwrap();
    world
        .store
        .stage_abort_allowance(TENANT, coordinator.txn_id(), &denied_id)
        .unwrap();
    coordinator.decide(false).await.unwrap();
    assert_eq!(
        world.store.denial_allowance_state(TENANT, &denied_id),
        Ok(None)
    );

    // The same request, sent again after the restart, is answered with the original ABORT and
    // the recovery applies the abort together with the allowance.
    let retried =
        handle_decision_commit(&world.state, 1, &caller, world.request("crashed", 9, 75)).await;
    assert_eq!(retried.error.as_deref(), Some("CERTIFICATE_DECIDED_ABORT"));
    assert_eq!(
        world.store.denial_allowance_state(TENANT, &denied_id),
        Ok(Some(DenialAllowanceState::Issued))
    );
    assert!(world
        .store
        .current_component(TENANT, &denied_id)
        .unwrap()
        .is_none());
}
