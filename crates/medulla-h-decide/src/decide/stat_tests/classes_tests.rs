//! EH-200: rule-derived capability classes feed library candidates as claims,
//! filter `classification_under`, and pin the rule-set identity.

use super::*;

fn model_profile(h: &Harness, id: &str, supports_tools: bool) {
    let mut draft = test_component_draft(TENANT, id);
    draft.kind = AgentComponentKind::ModelProfile;
    draft.summary = format!("model {id}");
    draft.facts = medulla_agent_types::agent_component::AgentComponentFacts::ModelProfile {
        provider: "provider".to_string(),
        model_identity: format!("model:{id}"),
        context_window_tokens: 8_192,
        max_output_tokens: 1_024,
        supports_tools,
        supports_structured_output: false,
        supports_vision: false,
        input_capabilities: None,
        modalities: Default::default(),
        cost: None,
        latency_declared: None,
        latency_observed_ref: None,
        shell_proficiency: None,
        stopping_behaviour: None,
    };
    h.commit(draft, None).unwrap();
}

#[cfg(feature = "owl")]
#[tokio::test]
async fn derived_classes_select_candidates_as_claims_under_a_pinned_rule_set() {
    let h = Harness::new().await;
    model_profile(&h, "model-a-tools", true);
    model_profile(&h, "model-b-plain", false);
    let schema_pin = h
        .publish_test_component(
            "schema-models",
            AgentComponentKind::FeatureSchema,
            "model features",
            Some(&schema()),
            None,
        )
        .unwrap();
    let mut request = request(
        &schema_pin,
        None,
        DecisionPolicyRef::Default,
        QuestionSafety::Ordinary,
    );
    request.candidates = CandidateSource::AgentLibrary {
        scope: LibraryCandidateScope {
            kinds: BoundedVec::new(vec![AgentComponentKind::ModelProfile]).unwrap(),
            classification_under: Some("eg:profile/tool-calling".to_string()),
        },
    };
    let batch = decide(&h, request).await.unwrap();
    let record = &batch.records.as_slice()[0];
    let FeatureMatrixRef::Inline { candidate_ids, .. } = &record.inputs.feature_matrix else {
        panic!("inline matrix")
    };
    assert_eq!(
        candidate_ids.as_slice(),
        ["model-a-tools".to_string()],
        "only the derived class matches the root"
    );
    assert!(
        record.inputs.classification_rules.is_some(),
        "the rule-set identity is recorded"
    );
    let derived = record
        .premises
        .iter()
        .find(|p| p.fact.starts_with("derived:eg:profile/tool-calling"))
        .expect("the derived class is a named premise");
    assert_eq!(
        derived.class,
        medulla_decision_types::decision::PremiseClass::Claim
    );
    assert_eq!(
        record.evidence_class,
        medulla_decision_types::decision::EvidenceClass::Claim
    );
}

#[cfg(feature = "query")]
async fn add_graph_rows(h: &Harness, graph: &str, rows: &[(&str, serde_json::Value)]) {
    let core = {
        let mut guard = h.state.write().await;
        guard
            .registry
            .create_graph(
                graph,
                medulla_protocol_types::protocol::GraphType::Team,
                None,
            )
            .unwrap();
        guard.registry.get(graph).unwrap().core.clone()
    };
    for (id, props) in rows {
        core.add_node(id.to_string(), rmp_serde::to_vec_named(props).unwrap());
        // The dispatch marks a graph dirty after every committed write; that bump
        // is the version the node history stamps the write with.
        core.mark_dirty();
    }
}

#[cfg(feature = "query")]
async fn graph_class_request(h: &Harness, graph: &str, label: &str) -> DecideRequest {
    let body = FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![number_feature("score")]).unwrap(),
    };
    let schema_pin = h
        .publish_test_component(
            &format!("schema-{graph}"),
            AgentComponentKind::FeatureSchema,
            "graph class features",
            Some(&body),
            None,
        )
        .unwrap();
    let mut request = request(
        &schema_pin,
        None,
        DecisionPolicyRef::Default,
        QuestionSafety::Ordinary,
    );
    request.candidates = CandidateSource::Graph {
        graph: graph.to_string(),
        plan: Box::new(medulla_types::wire::Plan::new(vec![
            medulla_types::wire::Op::Scan {
                label: label.to_string(),
            },
        ])),
    };
    request
}

#[cfg(feature = "query")]
fn graph_row_premises(
    record: &StatisticalDecisionRecord,
) -> Vec<(String, medulla_decision_types::decision::PremiseClass)> {
    let mut rows: Vec<_> = record
        .premises
        .iter()
        .filter(|premise| premise.fact.starts_with("graph_row:"))
        .map(|premise| (premise.fact.clone(), premise.class))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

// spec: MEDULLA-DECISION-ENGINE-R121.2
#[cfg(feature = "query")]
#[tokio::test]
async fn graph_claim_and_evidence_control_answer_class() {
    use medulla_decision_types::decision::statistical::graph_rows::GraphNodeClass;
    use medulla_decision_types::decision::PremiseClass;
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    add_graph_rows(
        &h,
        "kg-evidence",
        &[
            (
                "ev-1",
                serde_json::json!({"type": "Evidence", "score": 0.9, "source": "https://example.org/doc/1"}),
            ),
            (
                "ev-2",
                serde_json::json!({"type": "Evidence", "score": 0.4, "source": "urn:doc:2"}),
            ),
        ],
    )
    .await;
    // A claim whose properties and subject look like evidence: it names a
    // source and a `kind` of Evidence, but its typed label is Claim.
    add_graph_rows(
        &h,
        "kg-claim",
        &[(
            "evidence:looks-like",
            serde_json::json!({
                "type": "Claim", "score": 0.7, "kind": "Evidence",
                "source": "https://example.org/doc/3"
            }),
        )],
    )
    .await;
    // An Evidence node that names no source is not an observation.
    add_graph_rows(
        &h,
        "kg-nosource",
        &[(
            "ev-bare",
            serde_json::json!({"type": "Evidence", "score": 0.5}),
        )],
    )
    .await;
    add_graph_rows(
        &h,
        "kg-malformed",
        &[(
            "ev-bad",
            serde_json::json!({"type": "Evidence", "score": 0.5, "source": "not an iri"}),
        )],
    )
    .await;

    let evidence = decide(&h, graph_class_request(&h, "kg-evidence", "Evidence").await)
        .await
        .unwrap();
    let evidence = evidence.records.as_slice()[0].clone();
    assert_eq!(
        evidence
            .inputs
            .graph_rows
            .iter()
            .map(|row| (
                row.node_id.as_str(),
                row.node_class,
                row.source_iri.as_deref()
            ))
            .collect::<Vec<_>>(),
        [
            (
                "ev-1",
                GraphNodeClass::Observation,
                Some("https://example.org/doc/1")
            ),
            ("ev-2", GraphNodeClass::Observation, Some("urn:doc:2")),
        ]
    );
    assert_eq!(
        graph_row_premises(&evidence),
        [
            ("graph_row:ev-1".to_string(), PremiseClass::Observation),
            ("graph_row:ev-2".to_string(), PremiseClass::Observation),
        ]
    );
    assert_eq!(
        evidence.evidence_class,
        medulla_decision_types::decision::EvidenceClass::Observation,
        "only observations feed the answer, so it is an observation"
    );

    let claim = decide(&h, graph_class_request(&h, "kg-claim", "Claim").await)
        .await
        .unwrap();
    let claim = claim.records.as_slice()[0].clone();
    assert_eq!(
        claim.inputs.graph_rows.as_slice()[0].node_class,
        GraphNodeClass::Claim
    );
    assert_eq!(claim.inputs.graph_rows.as_slice()[0].source_iri, None);
    assert_eq!(
        graph_row_premises(&claim),
        [(
            "graph_row:evidence:looks-like".to_string(),
            PremiseClass::Claim
        )]
    );
    assert_eq!(
        claim.evidence_class,
        medulla_decision_types::decision::EvidenceClass::Claim,
        "a claim row never lifts the answer, whatever its properties say"
    );

    let bare = decide(&h, graph_class_request(&h, "kg-nosource", "Evidence").await)
        .await
        .unwrap();
    assert_eq!(
        bare.records.as_slice()[0].inputs.graph_rows.as_slice()[0].node_class,
        GraphNodeClass::Claim,
        "an Evidence node without a source is a claim"
    );

    let malformed = decide(
        &h,
        graph_class_request(&h, "kg-malformed", "Evidence").await,
    )
    .await
    .unwrap_err();
    assert!(
        malformed.starts_with("GRAPH_PROVENANCE_MALFORMED"),
        "{malformed}"
    );

    // Replay: the commit re-reads the rows and refuses a difference.
    let commit = |record: &StatisticalDecisionRecord| DecisionLogOp::Commit {
        record: Box::new(record.clone()),
        evaluator: None,
    };
    let committed: DecisionLogCommitted =
        decode(log_op(&h, "decider", commit(&evidence)).await).unwrap();
    assert!(!committed.replayed);
    let core = {
        let guard = h.state.read().await;
        guard.registry.get("kg-evidence").unwrap().core.clone()
    };
    let before = core.version();
    // Later writes: ev-1 is relabelled a claim and ev-2 is removed.
    let relabelled =
        serde_json::json!({"type": "Claim", "score": 0.9, "source": "https://example.org/doc/1"});
    core.add_node(
        "ev-1".to_string(),
        rmp_serde::to_vec_named(&relabelled).unwrap(),
    );
    core.mark_dirty();
    core.remove_node("ev-2".to_string());
    core.mark_dirty();
    assert!(
        core.version() > before,
        "the writes moved the graph version"
    );
    let fresh_claim = decide(&h, graph_class_request(&h, "kg-claim", "Claim").await)
        .await
        .unwrap()
        .records
        .as_slice()[0]
        .clone();
    assert!(
        decode::<DecisionLogCommitted>(log_op(&h, "decider", commit(&fresh_claim)).await).is_ok()
    );
    // A record never committed before those writes still replays: its rows are
    // re-read AT the version they were read at, where ev-1 was evidence and
    // ev-2 existed, not from the live graph.
    let mut later = evidence.clone();
    later.record_id = "decision:after-later-writes".to_string();
    later.record_digest =
        medulla_decision_types::decision::digest::statistical_record_digest(&later);
    let replayed: DecisionLogCommitted =
        decode(log_op(&h, "decider", commit(&later)).await).unwrap();
    assert!(!replayed.replayed);
    // A record whose rows differ from what the graph held at that version is
    // refused: it says ev-1 was a claim at a version where it was evidence.
    let mut forged = evidence.clone();
    forged.record_id = "decision:forged-rows".to_string();
    let mut rows = forged.inputs.graph_rows.as_slice().to_vec();
    rows[0].node_class = GraphNodeClass::Claim;
    rows[0].source_iri = None;
    forged.inputs.graph_rows = BoundedVec::new(rows).unwrap();
    forged.record_digest =
        medulla_decision_types::decision::digest::statistical_record_digest(&forged);
    let refused =
        decode::<DecisionLogCommitted>(log_op(&h, "decider", commit(&forged)).await).unwrap_err();
    assert!(refused.starts_with("DECISION_REPLAY_MISMATCH"), "{refused}");
    // A version the graph's retained history no longer reaches refuses with the
    // typed retirement code instead of answering from the live graph.
    let retired_graph = {
        add_graph_rows(
            &h,
            "kg-retired",
            &[(
                "ev-r",
                serde_json::json!({"type": "Evidence", "score": 0.8, "source": "urn:doc:r"}),
            )],
        )
        .await;
        let guard = h.state.read().await;
        guard.registry.get("kg-retired").unwrap().core.clone()
    };
    let recorded = decide(&h, graph_class_request(&h, "kg-retired", "Evidence").await)
        .await
        .unwrap()
        .records
        .as_slice()[0]
        .clone();
    retired_graph.add_node(
        "ev-r2".to_string(),
        rmp_serde::to_vec_named(&serde_json::json!({"type": "Claim", "score": 0.1})).unwrap(),
    );
    retired_graph.mark_dirty();
    retired_graph.clear();
    let retired =
        decode::<DecisionLogCommitted>(log_op(&h, "decider", commit(&recorded)).await).unwrap_err();
    assert!(retired.starts_with("GRAPH_VERSION_RETIRED"), "{retired}");
}
