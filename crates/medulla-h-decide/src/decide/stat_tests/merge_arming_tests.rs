//! The `merge_arming` family through the actual Decide path (MEDULLA-SDLC-R019.10): the verdict
//! is read from the delivery, sealed gate verdicts, owner approvals and reviews of one graph.

use std::time::Instant;

use medulla_agent_types::agent_component::DecisionPolicyRef;
use medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption;
use medulla_sdlc::lifecycle::delivery::open::{open_delivery, OpenDelivery};
use medulla_source_types::delivery::{DeliveryProposal, ProposalState, DELIVERY_RECORD_PROPERTY};
use medulla_source_types::gate_verdict::{
    gate_verdict_node_id, gate_verdict_seal, landing_policy_node_id, GateVerdictRecord,
    LandingPolicy, GATE_VERDICT_RECORD_PROPERTY, GATE_VERDICT_SEAL_PROPERTY,
};
use medulla_source_types::lifecycle::{ProjectMode, ProposalOrigin};
use medulla_types::contract::BoundedVec;
use medulla_work_types::step_verdict::VerificationVerdict;
use {
    medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind,
    medulla_decision_types::decision::statistical::CandidateSource,
    medulla_decision_types::decision::statistical::DecideRequest,
    medulla_decision_types::decision::statistical::QuestionSafety,
    medulla_decision_types::decision::statistical::StatisticalDecisionRecord,
    medulla_decision_types::decision::statistical::StatisticalOutcome,
    medulla_decision_types::decision::statistical::StatisticalQuestion,
    medulla_decision_types::decision::statistical::TypedParam,
    medulla_decision_types::decision::statistical::TypedValue,
};

use super::super::stat_executor::ExecutionContext;
use super::*;

const HEAD: &str = "sha256:head";
const CHANGESET: &str = "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0";
const PROPOSAL: &str = "pr-1";

fn digest(seed: char) -> String {
    std::iter::repeat_n(seed, 64).collect()
}

fn put(core: &medulla_graph_core::graph::GraphCore, id: &str, props: serde_json::Value) {
    core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
}

/// What the fixture graph holds beyond the delivery.
struct World {
    approval: Option<&'static str>,
    changes_requested: bool,
    seal_ok: bool,
    reviewed_head: &'static str,
}

fn graph_view(world: &World) -> (medulla_graph_core::graph::GraphView, String) {
    let core = medulla_graph_core::graph::GraphCore::new();
    let rows = vec!["R1".to_string()];
    let mut delivery = open_delivery(
        &OpenDelivery {
            tenant: TENANT,
            repo: "demo",
            slice_root: "R1",
            scope_rows: &rows,
            dispatch_decision_id: "dec-1",
            policy_version: "p1",
            actor: "person-1",
            origin: ProposalOrigin::Requested,
            mode: ProjectMode::Development,
            run_trace_id: "run-1",
            now_ms: 1,
        },
        None,
    )
    .unwrap();
    delivery.proposals.push(DeliveryProposal {
        id: PROPOSAL.to_string(),
        state: ProposalState::Open,
    });
    put(
        &core,
        &delivery.id,
        serde_json::json!({
            "type": "Delivery",
            DELIVERY_RECORD_PROPERTY: serde_json::to_string(&delivery).unwrap(),
            "tenant_id": TENANT,
        }),
    );
    let policy = LandingPolicy {
        repo: "demo".into(),
        branch: "main".into(),
        required_gates: vec!["unit".into()],
    };
    put(
        &core,
        &landing_policy_node_id("demo", "main"),
        serde_json::json!({
            "type": "LandingPolicy",
            "policy_record": serde_json::to_string(&policy).unwrap(),
        }),
    );
    for gate in ["unit", "ci"] {
        let record = GateVerdictRecord {
            changeset: CHANGESET.to_string(),
            gate_id: gate.to_string(),
            input_digest: digest('a'),
            verdict: VerificationVerdict::Pass,
            evidence_digest: digest('b'),
            duration_ms: 1,
        };
        let seal = if world.seal_ok {
            gate_verdict_seal(&record)
        } else {
            "forged".to_string()
        };
        put(
            &core,
            &gate_verdict_node_id(CHANGESET, gate),
            serde_json::json!({
                "type": "GateVerdict",
                GATE_VERDICT_RECORD_PROPERTY: serde_json::to_string(&record).unwrap(),
                GATE_VERDICT_SEAL_PROPERTY: seal,
            }),
        );
    }
    if let Some(commit) = world.approval {
        put(
            &core,
            "approval:1",
            serde_json::json!({
                "type": "OwnerApproval", "proposal_id": PROPOSAL,
                "head_commit": commit, "recorded_at_ms": 5,
            }),
        );
    }
    put(
        &core,
        "review:1",
        serde_json::json!({
            "type": "Review", "proposal_id": PROPOSAL, "reviewer": "alice",
            "head_commit": world.reviewed_head, "recorded_at_ms": 6,
            "state": if world.changes_requested { "changes_requested" } else { "approved" },
        }),
    );
    (core.analysis_snapshot(), delivery.id)
}

fn text(name: &str, value: &str) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

fn request(pin: &ComponentDependency, delivery: &str, changed_lines: i64) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "delivery.arming".to_string(),
            kind: QuestionKind::MergeArming,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![DeclaredOption {
                option_id: "merge_arming".to_string(),
                classification: BoundedVec::default(),
                numbers: BoundedVec::default(),
                texts: BoundedVec::default(),
            }])
            .unwrap(),
        },
        feature_schema: pin.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![
            text("graph", "delivery"),
            text("delivery_id", delivery),
            text("proposal_id", PROPOSAL),
            text("head_commit", HEAD),
            text("head_changeset", CHANGESET),
            text("branch", "main"),
            text("required_check", "ci"),
            TypedParam {
                name: "changed_lines".to_string(),
                value: TypedValue::Int(changed_lines),
            },
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: Default::default(),
    }
}

fn decide(
    h: &Harness,
    pin: &ComponentDependency,
    world: &World,
    changed_lines: i64,
) -> StatisticalDecisionRecord {
    let (view, delivery) = graph_view(world);
    let ctx = ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: TENANT,
        now_ms: 1_000,
        server_secret: b"decide-stat-test-secret",
        matched: &[],
        designation: None,
        reader: None,
        assistant_profile: None,
        observation_windows: None,
        type_digest: None,
        recommend_catalog: None,
        defect_triage: None,
    };
    super::super::stat_decide::decide_blocking(
        &ctx,
        &request(pin, &delivery, changed_lines),
        Some(&view),
        0,
        "decider".to_string(),
        Instant::now(),
    )
    .unwrap()
    .records
    .as_slice()[0]
        .clone()
}

/// The verdict option a record chose, or the abstention code it reported.
fn outcome_of(record: &StatisticalDecisionRecord) -> String {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => scores
            .iter()
            .find(|s| s.score.value != 0)
            .map(|s| s.option_id.clone())
            .expect("one verdict scores one"),
        StatisticalOutcome::Abstained { reasons } => format!("{:?}", reasons.as_slice()[0]),
        other => panic!("unexpected outcome {other:?}"),
    }
}

fn good() -> World {
    World {
        approval: Some(HEAD),
        changes_requested: false,
        seal_ok: true,
        reviewed_head: HEAD,
    }
}

/// Verifies MEDULLA-SDLC-R019.10: arm needs the owner approval on the head and passing gates,
/// read from the graph.
// spec: MEDULLA-SDLC-R019.10
#[tokio::test]
async fn arm_needs_owner_approval_on_head_and_passing_gates() {
    let h = Harness::new().await;
    let pin = publish_route_schema(&h).0;
    assert_eq!(outcome_of(&decide(&h, &pin, &good(), 400)), "verdict:arm");
    // Over the line budget, or gates whose seal does not verify, only hold.
    assert_eq!(outcome_of(&decide(&h, &pin, &good(), 401)), "verdict:hold");
    let forged = World {
        seal_ok: false,
        ..good()
    };
    assert_eq!(outcome_of(&decide(&h, &pin, &forged, 10)), "verdict:hold");
    // No approval, or an approval of another commit, abstains with the registered code.
    let none = World {
        approval: None,
        ..good()
    };
    assert!(outcome_of(&decide(&h, &pin, &none, 10)).contains("ARMING_APPROVAL_MISSING"));
    let stale = World {
        approval: Some("sha256:old"),
        ..good()
    };
    assert!(outcome_of(&decide(&h, &pin, &stale, 10)).contains("ARMING_HEAD_MISMATCH"));
}

/// Verifies MEDULLA-SDLC-R019.10: a moved head or a requested change disarms.
// spec: MEDULLA-SDLC-R019.10
#[tokio::test]
async fn a_moved_head_or_a_requested_change_disarms() {
    let h = Harness::new().await;
    let pin = publish_route_schema(&h).0;
    let moved = World {
        reviewed_head: "sha256:older",
        ..good()
    };
    assert_eq!(outcome_of(&decide(&h, &pin, &moved, 10)), "verdict:disarm");
    let asked = World {
        changes_requested: true,
        approval: None,
        ..good()
    };
    assert_eq!(outcome_of(&decide(&h, &pin, &asked, 10)), "verdict:disarm");
}
