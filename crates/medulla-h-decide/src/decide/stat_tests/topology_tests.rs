//! Served acceptance of elastic topology admission against the sandbox caps
//! (MEDULLA-DECISION-ENGINE-R046.750): the assembly of a topology record holds
//! the sealed plan to the pinned policy's cap knobs, records the caps it held
//! it to as premises, replays byte for byte, refuses every cap breach by code,
//! and refuses a foreign tenant at the served `AgentAssemble` door.

use std::sync::Arc;

use medulla_agent_types::agent_fixtures::topology::{
    facts, requirements, FAN_OUT_JOIN, INDEPENDENT_SUBTASKS,
};
use medulla_agent_types::agent_topology::{
    CapacityHeadroom, SchemaAuthority, TopologyAdmission, TopologyCaps, TopologyInputs,
    TopologyPlan,
};
use medulla_agent_types::delegation::AgentGraphEntryRef;
use medulla_compute::assemble::{admit_topology, assemble, inputs, replay_check, RecordIdentity};
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::{
    AssemblyRequest, DecisionOutcome, DecisionPolicy, DecisionRecord, TemplateFacts,
};
use medulla_mutation_types::capacity_lease::CapacityResourceClass;
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_persistence_records::decision_record::tests::{decided, seed_library, TENANT};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::BoundedVec;
use tokio::sync::RwLock;

const ASKER: &str = "asker";

/// A topology assembly over the seeded library under `policy`.
fn topology_record(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    policy: DecisionPolicy,
) -> (DecisionRecord, AssemblyRequest) {
    let base = decided(store);
    let mut topology = facts();
    let shape = base.graph.expect("a base graph").shape;
    let mut slot = topology.slots.as_slice()[0].clone();
    slot.node_id = shape.entry_node.clone();
    topology.slots = BoundedVec::new(vec![slot]).unwrap();
    let template = TemplateFacts {
        graph_id: "topology-template".into(),
        entry_revision: 1,
        definition_digest: "a".repeat(64),
        shape,
        topology: Some(topology),
    };
    let mut request = base.record.inputs.request;
    request.requirements.topology = Some(requirements(1));
    request.templates = BoundedVec::new(vec![AgentGraphEntryRef {
        tenant_id: TENANT.into(),
        graph_id: template.graph_id.clone(),
        entry_revision: template.entry_revision,
        definition_digest: template.definition_digest.clone(),
        actor_scope: "operator".into(),
        purpose_id: "agent-graph:publish".into(),
        policy_digest: "b".repeat(64),
        composed_work_ceiling: 8,
    }])
    .unwrap();
    let mut input = inputs(
        request.clone(),
        base.record.inputs.candidates.as_slice().to_vec(),
        vec![template],
        policy,
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
    let identity = || RecordIdentity {
        tenant_id: TENANT.into(),
        caller_principal: "caller-a".into(),
        created_at_ms: 20,
    };
    let record = assemble(input, identity()).expect("assembles").record;
    (record, request)
}

fn plan_of(record: &DecisionRecord) -> TopologyPlan {
    record
        .topology_plan()
        .unwrap_or_else(|| panic!("a topology plan: {:?}", record.outcome))
        .clone()
}

fn caps(width: u8, depth: u8, rounds: u8, tokens: Option<u64>) -> TopologyCaps {
    TopologyCaps {
        max_width: width,
        max_depth: depth,
        max_rounds: rounds,
        max_tokens: tokens,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R046.750
#[tokio::test]
async fn admission_enforces_caps_and_replays() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = ServerState::new_for_test(
        "topology-admission-secret",
        ServerState::test_isolation(ASKER),
    );
    server.persist_dir = Some(dir.path().to_string_lossy().into_owned());
    let state = Arc::new(RwLock::new(server));
    let store = state.write().await.ensure_agent_library().unwrap();
    seed_library(&store);

    // The default caps are the engine bounds; the record names each one and the plan digest.
    let (record, request) = topology_record(&store, DecisionPolicy::engine_default());
    let plan = plan_of(&record);
    let facts_of = |record: &DecisionRecord| -> Vec<String> {
        record.premises.iter().map(|p| p.fact.clone()).collect()
    };
    let facts = facts_of(&record);
    for expected in [
        "topology_cap:max_width=16",
        "topology_cap:max_depth=8",
        "topology_cap:max_rounds=8",
    ] {
        assert!(
            facts.iter().any(|f| f == expected),
            "{expected} in {facts:?}"
        );
    }
    let digest = medulla_agent_types::agent_topology::enforcement::plan_digest(&plan);
    assert!(facts.iter().any(|f| *f == format!("plan_digest:{digest}")));
    replay_check(&record).expect("an admitted topology record replays");

    // Byte-identical replay: the same inputs seal the same record.
    let (again, _) = topology_record(&store, DecisionPolicy::engine_default());
    assert_eq!(
        serde_json::to_vec(&record).unwrap(),
        serde_json::to_vec(&again).unwrap()
    );

    // A pinned cap knob is the cap the plan is held to, and is recorded as such.
    let mut tight = DecisionPolicy::engine_default();
    tight
        .knobs
        .insert(KnobName::new("topology.max_width"), KnobValue::Integer(1));
    let (tightened, _) = topology_record(&store, tight);
    assert!(facts_of(&tightened)
        .iter()
        .any(|f| f == "topology_cap:max_width=1"));
    replay_check(&tightened).expect("the tightened record replays");

    // Each breach is refused with its code; the cap boundary itself is admitted.
    let width = plan.slots.as_slice().iter().map(|s| s.width).max().unwrap();
    assert!(admit_topology(&plan, 1, &caps(width, 1, 8, None)).is_ok());
    let refused = |result: Result<_, medulla_agent_types::agent_topology::TopologyRefusal>| {
        result.expect_err("breach").code().as_str().to_string()
    };
    assert_eq!(
        refused(admit_topology(&plan, 1, &caps(width - 1, 8, 8, None))),
        "TOPOLOGY_WIDTH_EXCEEDS_CAP"
    );
    assert_eq!(
        refused(admit_topology(&plan, 3, &caps(16, 2, 8, None))),
        "TOPOLOGY_DEPTH_EXCEEDS_CAP"
    );
    let mut rounded = plan.clone();
    let mut slots = rounded.slots.as_slice().to_vec();
    slots[0].rounds = 4;
    rounded.slots = BoundedVec::new(slots).unwrap();
    assert_eq!(
        refused(admit_topology(&rounded, 1, &caps(16, 8, 3, None))),
        "TOPOLOGY_ROUNDS_EXCEEDS_CAP"
    );
    assert_eq!(
        refused(admit_topology(&plan, 1, &caps(16, 8, 8, Some(0)))),
        "TOPOLOGY_TOKENS_EXCEEDS_CAP"
    );
    assert!(matches!(record.outcome, DecisionOutcome::Solved { .. }));

    // Cross-tenant negative: a verified caller of another tenant is refused at the served door.
    let stranger = VerifiedRequestContext::verified_for_test_in_tenant(ASKER, "tenant-other");
    let response =
        crate::decide::handle_agent_assemble(&state, 1, ("__commons__", &stranger), request).await;
    assert!(
        response
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("ACCESS_DENIED")),
        "{response:?}"
    );
}
