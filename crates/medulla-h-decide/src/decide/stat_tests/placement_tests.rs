//! Served `Decide` of kind `Placement` (MEDULLA-PLACEMENT-R001.7): the
//! cheapest feasible deployment target, decided over the declared target
//! facts of a graph and the capacity headroom of the ledger.

use std::sync::Arc;

use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::placement::{
    cpu_cell, memory_cell, PlacementConstraints, PlacementQuestion, PlacementRead,
    PlacementRequestBody, PlacementSubject, PlacementTarget, PLACEMENT_PARAM,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionOutcome, EvidenceClass, ResolutionKind, Violation,
};
use medulla_mutation_types::capacity_lease::{CapacityCell, CapacityResourceClass, LeasePriority};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::protocol::Method;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_work_types::native_control::{CapacityCellUpdateRequest, NativeControlSchemaVersion};

use super::assemble_tests as library;
use super::*;

const GRAPH: &str = "placement";
pub(super) const CPU_HEADROOM: u64 = 4_000;
/// Memory headroom in bytes; a ledger cell holds at most
/// `MAX_CAPACITY_AMOUNT` (10^9) units, so the fixture stays under it.
const MEMORY_HEADROOM: u64 = 512 << 20;

/// `(ha, volumes, max replicas, relative cost)` per registered target.
fn declared_facts(target: PlacementTarget) -> (bool, bool, u32, u64) {
    match target {
        PlacementTarget::InProcess => (false, true, 1, 1),
        PlacementTarget::Compose => (false, true, 3, 2),
        PlacementTarget::Helm => (true, true, 10, 5),
        PlacementTarget::Swarm => (true, true, 20, 3),
    }
}

async fn cell(
    backend: &RedbBackend,
    cell_id: String,
    class: CapacityResourceClass,
    (capacity, reserved_floor): (u64, u64),
) {
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
                        reserved_floor,
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
    placement_harness_with_floor(0).await
}

/// [`placement_harness`] whose CPU cells keep `cpu_floor` millicores in reserve
/// for every priority but `BackgroundIngestion`.
pub(super) async fn placement_harness_with_floor(cpu_floor: u64) -> Harness {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let backend = Arc::new(
        RedbBackend::open_with_shards(h._dir.path().to_string_lossy().into_owned(), 16, 1).unwrap(),
    );
    for target in PlacementTarget::ALL {
        cell(
            &backend,
            cpu_cell(target),
            CapacityResourceClass::Cpu,
            (CPU_HEADROOM, cpu_floor),
        )
        .await;
        cell(
            &backend,
            memory_cell(target),
            CapacityResourceClass::Memory,
            (MEMORY_HEADROOM, 0),
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

pub(super) fn body(requires_ha: bool, replicas: u32) -> PlacementRequestBody {
    PlacementRequestBody {
        subject: PlacementSubject {
            workload_iri: "urn:workload:api".into(),
            cpu_millis: 500,
            memory_bytes: 64 << 20,
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
        // The declared options mirror the candidates, and a declared option
        // set is strictly sorted (check_declared): name them in id order.
        candidate_targets: {
            let mut ids: Vec<String> = PlacementTarget::ALL
                .iter()
                .map(|target| target.option_id().to_string())
                .collect();
            ids.sort();
            ids
        },
    }
}

pub(super) fn placement_request(
    schema: &ComponentDependency,
    body: PlacementRequestBody,
) -> DecideRequest {
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
        evidence_streams: Default::default(),
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

/// Every cell's headroom at `priority`, read straight from the ledger.
async fn ledger(h: &Harness, priority: LeasePriority) -> Vec<(String, u64, u64)> {
    let backend = h.state.read().await.persistence.clone().unwrap();
    let cells: Vec<String> = PlacementTarget::ALL
        .iter()
        .flat_map(|target| [cpu_cell(*target), memory_cell(*target)])
        .collect();
    backend
        .read_capacity_headroom(GRAPH, &cells, priority)
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.cell_id, row.available, row.epoch))
        .collect()
}

pub(super) fn premise_facts(batch: &DecisionBatch) -> Vec<String> {
    batch.records.as_slice()[0]
        .premises
        .iter()
        .map(|p| p.fact.clone())
        .collect()
}

// spec: MEDULLA-PLACEMENT-R001.8
#[tokio::test]
async fn placement_reads_headroom_without_lease() {
    const FLOOR: u64 = 1_500;
    let h = placement_harness_with_floor(FLOOR).await;
    let (schema_pin, _digest) = publish_route_schema(&h);
    let before = ledger(&h, LeasePriority::Interactive).await;

    // An interactive request sees the whole cell, as one `lease:<cell>:<class>`
    // observation premise per cell read.
    let batch = decide(&h, placement_request(&schema_pin, body(false, 2)))
        .await
        .unwrap();
    let facts = premise_facts(&batch);
    assert!(
        facts.contains(&format!("lease:placement:compose:cpu:cpu={CPU_HEADROOM}")),
        "{facts:?}"
    );
    let record = &batch.records.as_slice()[0];
    assert!(record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("lease:"))
        .all(|p| p.class == medulla_decision_types::decision::PremiseClass::Observation));

    // A lower-priority request sees less: the reserved floor is not its to spend.
    let mut background = body(false, 2);
    background.constraints.priority = LeasePriority::BackgroundIngestion;
    let batch = decide(&h, placement_request(&schema_pin, background))
        .await
        .unwrap();
    let facts = premise_facts(&batch);
    assert!(
        facts.contains(&format!(
            "lease:placement:compose:cpu:cpu={}",
            CPU_HEADROOM - FLOOR
        )),
        "{facts:?}"
    );

    // Nothing was acquired: the ledger reads the same after both decisions.
    assert_eq!(ledger(&h, LeasePriority::Interactive).await, before);
}

pub(super) fn abstain_reasons(batch: &DecisionBatch) -> Vec<AbstainReason> {
    let StatisticalOutcome::Abstained { reasons } = &batch.records.as_slice()[0].outcome else {
        panic!(
            "expected an abstention: {:?}",
            batch.records.as_slice()[0].outcome
        )
    };
    reasons.as_slice().to_vec()
}

// spec: MEDULLA-PLACEMENT-R001.10
#[tokio::test]
async fn placement_abstains_with_typed_reasons() {
    let h = placement_harness().await;
    let (schema_pin, _digest) = publish_route_schema(&h);

    // Infeasible: a demand above every cell's headroom names the binding cell
    // as `lease:<cell>:<class>`; a property rule names its `placement:*` property.
    let mut heavy = body(true, 2);
    heavy.subject.cpu_millis = CPU_HEADROOM;
    let batch = decide(&h, placement_request(&schema_pin, heavy))
        .await
        .unwrap();
    let reasons = abstain_reasons(&batch);
    let [AbstainReason::Infeasible { constraints }] = reasons.as_slice() else {
        panic!("expected one Infeasible reason: {reasons:?}")
    };
    let constraints: Vec<&str> = constraints.iter().map(String::as_str).collect();
    assert!(
        constraints.contains(&"lease:placement:swarm:cpu:cpu"),
        "{constraints:?}"
    );
    assert!(
        constraints.contains(&"lease:placement:helm:cpu:cpu"),
        "{constraints:?}"
    );
    assert!(
        constraints.contains(&"placement:compose: placement:supportsHa=false"),
        "{constraints:?}"
    );

    // PolicyLoosening: a request cap above the pinned policy cap abstains.
    let mut capped_policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    capped_policy.knobs.insert(
        medulla_decision_types::decision::knob_families::placement::KNOBS[0].knob_name(),
        medulla_decision_types::decision::knobs::KnobValue::Integer(2),
    );
    let mut loosening = body(false, 2);
    loosening.constraints.max_relative_cost = Some(5);
    let mut request = placement_request(&schema_pin, loosening);
    request.policy = DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-placement-cap", &capped_policy),
    };
    let batch = decide(&h, request).await.unwrap();
    assert_eq!(
        abstain_reasons(&batch),
        vec![AbstainReason::PolicyLoosening {
            field: "max_relative_cost".to_string()
        }]
    );

    // UnknownFact: a target with no declared facts is named, never guessed.
    {
        let guard = h.state.read().await;
        let core = guard.registry.get(GRAPH).unwrap().core.clone();
        for target in PlacementTarget::ALL {
            core.remove_node(target.option_id().to_string());
        }
    }
    let batch = decide(&h, placement_request(&schema_pin, body(false, 2)))
        .await
        .unwrap();
    let reasons = abstain_reasons(&batch);
    assert_eq!(reasons.len(), PlacementTarget::ALL.len(), "{reasons:?}");
    // The declared options are strictly sorted by id, so the reasons follow id order.
    let mut by_id = PlacementTarget::ALL;
    by_id.sort_by_key(|target| target.option_id());
    for (reason, target) in reasons.iter().zip(by_id) {
        assert_eq!(
            *reason,
            AbstainReason::UnknownFact {
                component_id: target.option_id().to_string(),
                field: "placement:facts".to_string(),
            }
        );
    }

    // CAPACITY_UNAVAILABLE: no durable ledger is an error, never a guess.
    h.state.write().await.persistence = None;
    let refused = decide(&h, placement_request(&schema_pin, body(false, 2)))
        .await
        .unwrap_err();
    assert!(refused.starts_with("CAPACITY_UNAVAILABLE"), "{refused}");

    // Every code the family raises is registered in its table or the fixed set.
    let table = medulla_decision_types::decision::statistical::placement::REFUSALS;
    for code in ["PLACEMENT_FACTS_INVALID", "CAPACITY_UNAVAILABLE"] {
        assert!(table.iter().any(|c| c.as_str() == code), "{code}");
    }
    // The abstentions are the fixed `AbstainReason` codes, kept apart from the
    // registered tables (`record::outcome::codes_are_unique_and_stable` pins both).
    for code in ["INFEASIBLE", "UNKNOWN_FACT", "POLICY_LOOSENING"] {
        assert!(
            medulla_decision_types::decision::record::outcome::FIXED_CODES
                .iter()
                .any(|fixed| fixed.as_str() == code),
            "{code}"
        );
    }
}

// spec: MEDULLA-PLACEMENT-R001.12
/// Verifies MEDULLA-PLACEMENT-R001.12: the served assembly of the template the
/// chosen target names answers the deployment steps in slot order.
#[tokio::test]
async fn deployment_template_assembles_ordered_steps() {
    let library = library::harness().await;
    let targets: Vec<&str> = PlacementTarget::ALL
        .iter()
        .map(|target| target.option_id().trim_start_matches("placement:"))
        .collect();
    library::seed(&library.store, None);
    for (nonce, tool) in (40u8..).zip(["tool-render-artifact", "tool-apply", "tool-verify"]) {
        library::publish(
            &library.store,
            nonce,
            (
                tool,
                medulla_agent_types::agent_component::AgentComponentKind::Tool,
            ),
            None,
        );
    }
    let templates = medulla_persistence_records::agent_graph::tests::publish_deployment_templates(
        &library.store,
        library::TENANT,
        &targets,
    );
    assert_eq!(templates.len(), targets.len(), "one template per target");

    for (target, template) in targets.iter().zip(templates) {
        assert_eq!(template.graph_id, format!("deployment:{target}"));
        let mut request = library::request(Vec::new());
        request.templates = BoundedVec::new(vec![template]).unwrap();
        let result = library::assemble(&library, request)
            .await
            .unwrap_or_else(|error| panic!("{target}: the template assembles: {error}"));
        let DecisionOutcome::Solved { slots, .. } = &result.record.outcome else {
            panic!(
                "{target}: expected a solved assembly, got {:?}",
                result.record.outcome
            );
        };
        let mut nodes: Vec<&str> = Vec::new();
        for slot in slots.iter() {
            let (node, _kind) = slot
                .slot
                .split_once('/')
                .expect("a template slot names its node");
            if nodes.last() != Some(&node) {
                nodes.push(node);
            }
        }
        assert_eq!(
            nodes,
            ["render-artifact", "apply", "verify"],
            "{target}: slots follow the template order"
        );
        let graph = result
            .graph
            .as_ref()
            .expect("a solved record answers its graph");
        let steps: Vec<&str> = graph
            .shape
            .nodes
            .iter()
            .map(|n| n.node_id.as_str())
            .collect();
        assert_eq!(steps, ["render-artifact", "apply", "verify", "done"]);
        assert_eq!(
            result.agents.as_slice().len(),
            3,
            "one agent per template slot"
        );
    }
}

// spec: MEDULLA-PLACEMENT-R001.9
#[tokio::test]
async fn placement_record_has_one_why_not_per_non_chosen_target() {
    let h = placement_harness().await;
    let (schema_pin, _digest) = publish_route_schema(&h);

    // Two replicas: compose is chosen. in_process is eliminated by its
    // replica cap; swarm and helm are feasible but costlier.
    let batch = decide(&h, placement_request(&schema_pin, body(false, 2)))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let Some(FamilyAnswer::Placement(answer)) = &record.answer else {
        panic!("a placement answer is sealed: {:?}", record.answer)
    };
    let rows = answer.why_not.as_slice();
    let mut ids: Vec<&str> = rows.iter().map(|row| row.component_id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        ["placement:helm", "placement:in_process", "placement:swarm"],
        "one why-not per non-chosen target and none for the chosen one"
    );
    for row in rows {
        assert_eq!(row.slot, "target");
    }
    let by_id = |id: &str| rows.iter().find(|row| row.component_id == id).unwrap();
    let swarm = by_id("placement:swarm");
    assert!(
        swarm.forced_objective.is_some(),
        "a feasible but costlier target carries the objective it would force"
    );
    assert_eq!(
        swarm.violation,
        Some(Violation::Placement {
            reason: "costlier than placement:compose (3 > 2)".to_string()
        })
    );
    let in_process = by_id("placement:in_process");
    assert_eq!(in_process.forced_objective, None);
    assert_eq!(
        in_process.violation,
        Some(Violation::Placement {
            reason: "placement:maxReplicas=1 < 2".to_string()
        })
    );

    // Requiring HA eliminates compose by a declared property.
    let batch = decide(&h, placement_request(&schema_pin, body(true, 2)))
        .await
        .unwrap();
    let Some(FamilyAnswer::Placement(answer)) = &batch.records.as_slice()[0].answer else {
        panic!("a placement answer is sealed")
    };
    let compose = answer
        .why_not
        .iter()
        .find(|row| row.component_id == "placement:compose")
        .expect("compose is explained");
    assert_eq!(
        compose.violation,
        Some(Violation::Placement {
            reason: "placement:supportsHa=false".to_string()
        })
    );
    assert_eq!(
        answer.why_not.len(),
        3,
        "swarm chosen; three others explained"
    );
}

// spec: MEDULLA-PLACEMENT-R001.15
#[tokio::test]
async fn capacity_proposal_reuses_placement_headroom() {
    let h = placement_harness().await;
    let (schema, _) = publish_route_schema(&h);
    let cell_id = cpu_cell(PlacementTarget::Compose);

    // The headroom the ledger holds for the cell at the proposal's priority.
    let backend = h.state.read().await.persistence.clone().unwrap();
    let rows = backend
        .read_capacity_headroom(GRAPH, std::slice::from_ref(&cell_id), LeasePriority::Orchestration)
        .await
        .unwrap();
    let row = rows.iter().find(|row| row.cell_id == cell_id).unwrap();
    let (available, epoch) = (row.available, row.epoch);
    assert_eq!(available, CPU_HEADROOM);

    // A capacity proposal over that cell is decided against the same headroom: its
    // observation premise carries the ledger's number and epoch, and it allows.
    let proposal = serde_json::json!({
        "proposal_id": "p-placement", "tenant_id": TENANT, "resource_class_iri": cell_id,
        "unit": "cpu", "current_capacity": 2_000, "proposed_capacity": 3_000,
        "min_capacity": 1_000, "max_capacity": 4_000, "measured_load": 1_500,
        "available_headroom": 1_000, "window_start_ms": 1, "window_end_ms": 2,
        "premise_revision": 1, "policy_revision": 1,
        "proposal_digest": "a".repeat(64),
    });
    let request = declared_request(
        &schema,
        TENANT,
        QuestionKind::CapacityProposal,
        "verdict",
        DecisionPolicyRef::Default,
        vec![
            TypedParam {
                name: "graph".to_string(),
                value: TypedValue::Text(GRAPH.to_string()),
            },
            TypedParam {
                name: "proposal".to_string(),
                value: TypedValue::Text(proposal.to_string()),
            },
        ],
    );
    let batch = decide(&h, request).await.unwrap();
    match &batch.records.as_slice()[0].outcome {
        StatisticalOutcome::Advisory { scores, .. } => {
            assert_eq!(scores.as_slice()[0].option_id, "allow");
        }
        other => panic!("unexpected outcome {other:?}"),
    }
    assert!(premise_facts(&batch).contains(&format!("headroom:{cell_id}:{available}:epoch={epoch}")));

    // A placement decision at the same priority reads the same number from the same
    // ledger: there is one headroom premise, not a second controller.
    let mut placement = body(false, 2);
    placement.constraints.priority = LeasePriority::Orchestration;
    let batch = decide(&h, placement_request(&schema, placement)).await.unwrap();
    assert!(premise_facts(&batch).contains(&format!("lease:{cell_id}:cpu={available}")));
}
