//! Served `Decide` of kind `Placement` (MEDULLA-PLACEMENT-R001.7): the
//! cheapest feasible deployment target, decided over the declared target
//! facts of a graph and the capacity headroom of the ledger.

use std::sync::Arc;

use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::placement::{
    cpu_cell, memory_cell, PlacementConstraints, PlacementQuestion, PlacementRead,
    PlacementRequestBody, PlacementSubject, PlacementTarget, PLACEMENT_PARAM,
};
use medulla_decision_types::decision::{EvidenceClass, ResolutionKind};
use medulla_mutation_types::capacity_lease::{CapacityCell, CapacityResourceClass, LeasePriority};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::protocol::Method;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_work_types::native_control::{CapacityCellUpdateRequest, NativeControlSchemaVersion};

use super::*;

const GRAPH: &str = "placement";
const CPU_HEADROOM: u64 = 4_000;
const MEMORY_HEADROOM: u64 = 8 << 30;

/// `(ha, volumes, max replicas, relative cost)` per registered target.
fn declared_facts(target: PlacementTarget) -> (bool, bool, u32, u64) {
    match target {
        PlacementTarget::InProcess => (false, true, 1, 1),
        PlacementTarget::Compose => (false, true, 3, 2),
        PlacementTarget::Helm => (true, true, 10, 5),
        PlacementTarget::Swarm => (true, true, 20, 3),
    }
}

async fn cell(backend: &RedbBackend, cell_id: String, class: CapacityResourceClass, capacity: u64) {
    backend
        .commit_capacity_lease(
            GRAPH,
            Method::UpdateCapacityCell {
                request: CapacityCellUpdateRequest {
                    schema_version: NativeControlSchemaVersion::V1,
                    cell: CapacityCell {
                        cell_id,
                        parent_id: None,
                        resource_class: class,
                        capacity,
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
}

/// A harness whose graph holds the four `placement:Target` nodes and whose
/// ledger holds one CPU and one memory cell per target.
async fn placement_harness() -> Harness {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let backend = Arc::new(
        RedbBackend::open_with_shards(h._dir.path().to_string_lossy().into_owned(), 16, 1).unwrap(),
    );
    for target in PlacementTarget::ALL {
        cell(
            &backend,
            cpu_cell(target),
            CapacityResourceClass::Cpu,
            CPU_HEADROOM,
        )
        .await;
        cell(
            &backend,
            memory_cell(target),
            CapacityResourceClass::Memory,
            MEMORY_HEADROOM,
        )
        .await;
    }
    let mut guard = h.state.write().await;
    guard.persistence = Some(backend);
    guard
        .registry
        .create_graph(
            GRAPH,
            medulla_protocol_types::protocol::GraphType::Team,
            None,
        )
        .unwrap();
    let core = guard.registry.get(GRAPH).unwrap().core.clone();
    for target in PlacementTarget::ALL {
        let (ha, volumes, max_replicas, cost) = declared_facts(target);
        let props = serde_json::json!({
            "type": super::super::stat_placement::TARGET_CLASS,
            medulla_types::ontology_iri!("placement#supportsHa"): ha,
            medulla_types::ontology_iri!("placement#supportsVolumes"): volumes,
            medulla_types::ontology_iri!("placement#maxReplicas"): max_replicas,
            medulla_types::ontology_iri!("placement#relativeCost"): cost,
        });
        core.add_node(
            target.option_id().to_string(),
            rmp_serde::to_vec_named(&props).unwrap(),
        );
    }
    drop(guard);
    h
}

fn body(requires_ha: bool, replicas: u32) -> PlacementRequestBody {
    PlacementRequestBody {
        subject: PlacementSubject {
            workload_iri: "urn:workload:api".into(),
            cpu_millis: 500,
            memory_bytes: 1 << 30,
            replicas,
            needs_persistent_volume: false,
        },
        constraints: PlacementConstraints {
            requires_ha,
            forbidden_targets: vec![],
            max_relative_cost: None,
            capacity_cells: PlacementTarget::ALL
                .iter()
                .flat_map(|target| [cpu_cell(*target), memory_cell(*target)])
                .collect(),
            priority: LeasePriority::Interactive,
        },
        candidate_targets: PlacementTarget::ALL
            .iter()
            .map(|target| target.option_id().to_string())
            .collect(),
    }
}

fn placement_request(schema: &ComponentDependency, body: PlacementRequestBody) -> DecideRequest {
    let options = body
        .candidate_targets
        .iter()
        .map(|id| DeclaredOption {
            option_id: id.clone(),
            classification: BoundedVec::default(),
            numbers: BoundedVec::default(),
            texts: BoundedVec::default(),
        })
        .collect();
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "deploy.placement".to_string(),
            kind: QuestionKind::Placement,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(options).unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![TypedParam {
            name: PLACEMENT_PARAM.to_string(),
            value: TypedValue::Placement(Box::new(PlacementQuestion {
                graph: GRAPH.to_string(),
                body,
                read: None,
            })),
        }])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn ranked(batch: &DecisionBatch) -> Vec<String> {
    let StatisticalOutcome::Advisory { scores, calibrated } = &batch.records.as_slice()[0].outcome
    else {
        panic!(
            "expected an Advisory outcome: {:?}",
            batch.records.as_slice()[0].outcome
        )
    };
    assert!(!calibrated, "a placement score is not a probability");
    scores.iter().map(|s| s.option_id.clone()).collect()
}

// spec: MEDULLA-PLACEMENT-R001.7
#[tokio::test]
async fn placement_request_returns_cheapest_feasible_target() {
    let h = placement_harness().await;
    let (schema_pin, _digest) = publish_route_schema(&h);

    // Two replicas: in_process (max 1) is eliminated; compose is cheapest,
    // then swarm, then helm. The cheapest feasible target is the top score.
    let batch = decide(&h, placement_request(&schema_pin, body(false, 2)))
        .await
        .unwrap();
    assert_eq!(
        ranked(&batch),
        ["placement:compose", "placement:swarm", "placement:helm"]
    );
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::Placement);
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let facts: Vec<&str> = record.premises.iter().map(|p| p.fact.as_str()).collect();
    assert!(
        facts.contains(&"lease:placement:compose:cpu:cpu=4000"),
        "the headroom read is a premise: {facts:?}"
    );
    assert!(
        facts
            .iter()
            .any(|f| f.starts_with("placement:facts ha=false")),
        "the declared target facts are premises: {facts:?}"
    );

    // Requiring HA removes compose: swarm is now the cheapest feasible target.
    let batch = decide(&h, placement_request(&schema_pin, body(true, 2)))
        .await
        .unwrap();
    assert_eq!(ranked(&batch), ["placement:swarm", "placement:helm"]);

    // A request caps cost below every feasible target: nothing is feasible and
    // the answer abstains naming each binding constraint.
    let mut capped = body(true, 2);
    capped.constraints.max_relative_cost = Some(2);
    let batch = decide(&h, placement_request(&schema_pin, capped))
        .await
        .unwrap();
    assert!(matches!(
        batch.records.as_slice()[0].outcome,
        StatisticalOutcome::Abstained { .. }
    ));

    // A caller never supplies the engine's read.
    let mut supplied = placement_request(&schema_pin, body(false, 2));
    supplied.params = BoundedVec::new(vec![TypedParam {
        name: PLACEMENT_PARAM.to_string(),
        value: TypedValue::Placement(Box::new(PlacementQuestion {
            graph: GRAPH.to_string(),
            body: body(false, 2),
            read: Some(PlacementRead {
                targets: vec![],
                headroom: vec![],
            }),
        })),
    }])
    .unwrap();
    let refused = decide(&h, supplied).await.unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");

    // Cross-tenant negative: a verified caller of another tenant is refused
    // at the served door, before any graph or ledger is read.
    let stranger = VerifiedRequestContext::verified_for_test_in_tenant("decider", "tenant-other");
    let response = super::super::statistical::handle_decide(
        &h.state,
        1,
        &stranger,
        placement_request(&schema_pin, body(false, 2)),
    )
    .await;
    assert!(
        response
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("ACCESS_DENIED")),
        "{response:?}"
    );
}
