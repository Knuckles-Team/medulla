//! The `defect_triage` family served through `TriageDefect` (MEDULLA-SDLC-R004.3 to R004.6): a
//! seeded defect, its frames, symbols and blame give a sealed decision that replays; an unknown,
//! closed or foreign defect refuses and stores nothing.

use medulla_agent_types::agent_component::DecisionPolicyRef;
use medulla_decision_types::decision::statistical::log::{DecisionLogEntry, DecisionLogOp};
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_source_types::lifecycle::{
    defect_node_id, Defect, DefectIntake, DefectStatus, Severity, Symptom, DEFECT_RECORD_PROPERTY,
};
use {
    medulla_decision_statistical_types::decision::defect_triage::TriageAbstainReason,
    medulla_decision_statistical_types::decision::defect_triage::TriageDefectRequest,
};

use medulla_h_decide_stat_tests::decide::stat_tests::log_tests::log_op;
use medulla_h_decide_stat_tests::decide::stat_tests::*;

const GRAPH: &str = "defect-graph";
const FRAME: &str = "at run (src/a.rs:15:3)";

fn defect(id: &str, status: DefectStatus, text: &str) -> Defect {
    Defect {
        id: id.to_string(),
        repo: "demo".to_string(),
        title: "boom".to_string(),
        intake: DefectIntake::Direct,
        signature_digest: format!("sig-{id}"),
        severity: Severity::Medium,
        status,
        symptoms: vec![Symptom {
            intake: DefectIntake::Direct,
            source_record_id: "src-1".to_string(),
            observed_at: 1_000,
            text: text.to_string(),
        }],
        created_at: 900,
    }
}

async fn graph_harness() -> Harness {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    h.state
        .write()
        .await
        .registry
        .create_graph(
            GRAPH,
            medulla_protocol_types::protocol::GraphType::Team,
            None,
        )
        .unwrap();
    h
}

async fn put_defect(h: &Harness, defect: &Defect, tenant: Option<&str>) {
    let mut props = serde_json::json!({
        "type": "Defect",
        DEFECT_RECORD_PROPERTY: serde_json::to_string(defect).unwrap(),
        "repo": defect.repo,
    });
    if let Some(tenant) = tenant {
        props["tenant_id"] = tenant.into();
    }
    h.seed_node(GRAPH, &defect_node_id(&defect.repo, &defect.id), props)
        .await;
}

/// One symbol `symbol:a` over lines 10 to 20 of `src/a.rs`, changed by `c1` (author `alice`) at
/// 500 ms.
async fn seed_code(h: &Harness) {
    h.seed_node(
        GRAPH,
        "symbol:a",
        serde_json::json!({
            "type": "SYMBOL", "file_path": "src/a.rs", "line": "10", "end_line": "20",
            "qualified_symbol": "a::run",
        }),
    )
    .await;
    h.seed_node(
        GRAPH,
        "commit:c1",
        serde_json::json!({
            "type": "Changeset", "changeset_id": "c1", "repo": "demo",
            "author": "alice", "timestamp_ms": "500",
        }),
    )
    .await;
    let core = h
        .state
        .read()
        .await
        .registry
        .get(GRAPH)
        .expect("graph exists")
        .core
        .clone();
    core.add_edge(
        "commit:c1".to_string(),
        "symbol:a".to_string(),
        rmp_serde::to_vec_named(&serde_json::json!({ "relationship": "changes" })).unwrap(),
    )
    .unwrap();
}

fn request(pin: &ComponentDependency, defect_id: &str) -> TriageDefectRequest {
    TriageDefectRequest {
        tenant_id: TENANT.to_string(),
        graph: GRAPH.to_string(),
        defect_id: defect_id.to_string(),
        as_of_ms: Some(1_000),
        culprit: None,
        path_owners: vec!["alice".to_string()],
        feature_schema: pin.clone(),
        policy: DecisionPolicyRef::Default,
    }
}

fn artifact_rows(h: &Harness) -> usize {
    h.store
        .decision_artifacts_with_prefix(TENANT, "", 100_000)
        .unwrap()
        .len()
}

/// Verifies MEDULLA-SDLC-R004.3: a seeded defect triages to its symbol and owner, and the
/// sealed record reads back from the decision log.
// spec: MEDULLA-SDLC-R004.3
#[tokio::test]
async fn served_decide_triages_a_seeded_defect() {
    let h = graph_harness().await;
    let (pin, _) = publish_route_schema(&h);
    put_defect(&h, &defect("d1", DefectStatus::New, FRAME), None).await;
    seed_code(&h).await;
    let outcome = medulla_h_decide::decide::triage::handle_triage_defect(
        &h.state,
        1,
        &verified(),
        request(&pin, "d1"),
    )
    .await
    .unwrap();
    assert_eq!(outcome.decision.suspected_symbols, ["symbol:a"]);
    assert_eq!(outcome.decision.owner_principal.as_deref(), Some("alice"));
    assert_eq!(outcome.decision.abstain_reason, None);
    assert_eq!(
        outcome.decision.affects_edges(),
        [("d1".to_string(), "symbol:a".to_string())]
    );
    assert_eq!(outcome.defect_node_id, "defect:demo/d1");
    let entry: DecisionLogEntry = decode(
        log_op(
            &h,
            "decider",
            DecisionLogOp::Get {
                tenant_id: TENANT.to_string(),
                record_id: outcome.record_id.clone(),
            },
        )
        .await,
    )
    .unwrap();
    assert!(matches!(
        entry.record.answer,
        Some(FamilyAnswer::DefectTriage(_))
    ));
}

/// Verifies MEDULLA-SDLC-R004.4: unknown, closed and foreign defects refuse and store nothing.
// spec: MEDULLA-SDLC-R004.4
#[tokio::test]
async fn triage_refusals() {
    let h = graph_harness().await;
    let (pin, _) = publish_route_schema(&h);
    put_defect(&h, &defect("closed", DefectStatus::Closed, FRAME), None).await;
    put_defect(&h, &defect("verified", DefectStatus::Verified, FRAME), None).await;
    put_defect(
        &h,
        &defect("foreign", DefectStatus::New, FRAME),
        Some("other-tenant"),
    )
    .await;
    seed_code(&h).await;
    let before = artifact_rows(&h);
    for (id, code) in [
        ("missing", "DEFECT_UNKNOWN"),
        ("foreign", "DEFECT_UNKNOWN"),
        ("closed", "DEFECT_CLOSED"),
        ("verified", "DEFECT_CLOSED"),
    ] {
        let refused = medulla_h_decide::decide::triage::handle_triage_defect(
            &h.state,
            1,
            &verified(),
            request(&pin, id),
        )
        .await
        .unwrap_err();
        assert!(refused.starts_with(code), "{id}: {refused}");
    }
    assert_eq!(artifact_rows(&h), before, "a refusal stores no record");
}

/// Verifies MEDULLA-SDLC-R004.5: the sealed record carries its premises and replays.
// spec: MEDULLA-SDLC-R004.5
#[tokio::test]
async fn triage_record_replays_to_an_equal_digest() {
    let h = graph_harness().await;
    let (pin, _) = publish_route_schema(&h);
    put_defect(&h, &defect("d1", DefectStatus::New, FRAME), None).await;
    seed_code(&h).await;
    let mut asked = request(&pin, "d1");
    asked.culprit = Some("c1".to_string());
    // The commit inside the handler is the verify-replay: it re-derives the record from its
    // stored inputs over the same filtered view and refuses a different digest.
    let outcome =
        medulla_h_decide::decide::triage::handle_triage_defect(&h.state, 1, &verified(), asked)
            .await
            .unwrap();
    let entry: DecisionLogEntry = decode(
        log_op(
            &h,
            "decider",
            DecisionLogOp::Get {
                tenant_id: TENANT.to_string(),
                record_id: outcome.record_id,
            },
        )
        .await,
    )
    .unwrap();
    let facts: Vec<&str> = entry
        .record
        .premises
        .as_slice()
        .iter()
        .map(|premise| premise.fact.as_str())
        .collect();
    let digest = medulla_core::lifecycle::defect::signature_digest(FRAME);
    assert!(facts.contains(&"triage.defect"));
    assert!(facts.contains(&format!("triage.symptom_digest:{digest}").as_str()));
    assert!(facts.contains(&"triage.blame:symbol:a=c1@500"));
    assert!(facts.contains(&"triage.culprit:c1"));
    assert!(facts
        .iter()
        .any(|fact| fact.starts_with("knob:triage.max_frames=")));
    assert_eq!(entry.record.record_digest.len(), 64);
}

/// Verifies MEDULLA-SDLC-R004.6: a defect whose frame resolves to no symbol abstains with the
/// typed reason, seals the record and writes no edge.
// spec: MEDULLA-SDLC-R004.6
#[tokio::test]
async fn triage_journey() {
    let h = graph_harness().await;
    let (pin, _) = publish_route_schema(&h);
    put_defect(&h, &defect("d1", DefectStatus::New, FRAME), None).await;
    put_defect(
        &h,
        &defect("d2", DefectStatus::New, "nothing resolves here"),
        None,
    )
    .await;
    seed_code(&h).await;
    let triaged = medulla_h_decide::decide::triage::handle_triage_defect(
        &h.state,
        1,
        &verified(),
        request(&pin, "d1"),
    )
    .await
    .unwrap();
    assert_eq!(triaged.decision.suspected_symbols, ["symbol:a"]);
    let abstained = medulla_h_decide::decide::triage::handle_triage_defect(
        &h.state,
        1,
        &verified(),
        request(&pin, "d2"),
    )
    .await
    .unwrap();
    assert_eq!(
        abstained.decision.abstain_reason,
        Some(TriageAbstainReason::NoSymbolMatch)
    );
    assert!(abstained.decision.affects_edges().is_empty());
}
