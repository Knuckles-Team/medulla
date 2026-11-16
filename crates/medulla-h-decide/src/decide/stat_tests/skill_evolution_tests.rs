//! The `skill_evolution` family through the actual Decide handler
//! (MEDULLA-DECISION-ENGINE-R703.3): a requested skill no visible skill carries
//! is a gap, a term the capability IRI resolver rejects and runs used often
//! enough is a neologism, and two near-duplicate skills are a merge candidate
//! scored by their overlap. The answer is advisory and writes nothing, and one
//! tenant never sees another tenant's skills.

use super::*;
use medulla_decision_types::decision::skill_evolution::{
    PARAM_CANDIDATE_TRACES, PARAM_EMBEDDING_GRAPH, PARAM_PAIR_A, PARAM_PAIR_B,
    PARAM_REQUESTED_SKILLS, PARAM_SKILL_EMBEDDINGS, PARAM_USED_TERMS,
};
use medulla_decision_types::decision::{PremiseClass, PremiseProvenance};
use medulla_server_api::authority_context::VerifiedRequestContext;

const OTHER_TENANT: &str = "tenant-other";
const VECTORS: &str = "skill-vectors";

fn publish_skill(h: &Harness, tenant: &str, id: &str, capabilities: &[&str]) {
    let mut draft = test_component_draft(tenant, id);
    draft.kind = AgentComponentKind::Skill;
    draft.declared_capabilities = capabilities.iter().map(|c| (*c).to_string()).collect();
    h.commit_in(tenant, draft, None).unwrap();
}

/// Publish a skill classified under the action root, so its side-effect class
/// differs from a plain skill's.
fn publish_acting_skill(h: &Harness, tenant: &str, id: &str, capabilities: &[&str]) {
    let mut draft = test_component_draft(tenant, id);
    draft.kind = AgentComponentKind::Skill;
    draft.declared_capabilities = capabilities.iter().map(|c| (*c).to_string()).collect();
    draft.classification = vec!["eg:capability/action".to_string()];
    h.commit_in(tenant, draft, None).unwrap();
}

fn text(name: &str, value: &str) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

fn list(values: &[&str]) -> TypedValue {
    TypedValue::IriList(BoundedVec::new(values.iter().map(|v| (*v).to_string()).collect()).unwrap())
}

fn evolution_request(
    schema: &ComponentDependency,
    tenant: &str,
    extra: Vec<TypedParam>,
) -> DecideRequest {
    let mut params = vec![
        TypedParam {
            name: PARAM_REQUESTED_SKILLS.to_string(),
            value: list(&["skill-a", "skill-missing", "skill-missing", "skill-x"]),
        },
        TypedParam {
            name: PARAM_USED_TERMS.to_string(),
            value: list(&[
                "frobnicate",
                "frobnicate",
                "frobnicate",
                "rarely",
                "eg:capability/retrieval",
                "eg:capability/retrieval",
                "eg:capability/retrieval",
            ]),
        },
    ];
    params.extend(extra);
    let mut request = declared_request(
        schema,
        tenant,
        QuestionKind::SkillEvolution,
        "unused",
        DecisionPolicyRef::Default,
        params,
    );
    request.candidates = CandidateSource::AgentLibrary {
        scope: LibraryCandidateScope {
            kinds: BoundedVec::new(vec![AgentComponentKind::Skill]).unwrap(),
            classification_under: None,
        },
    };
    request
}

fn embedding_graph_param() -> TypedParam {
    TypedParam {
        name: PARAM_EMBEDDING_GRAPH.to_string(),
        value: TypedValue::Text(VECTORS.to_string()),
    }
}

fn options(batch: &DecisionBatch) -> Vec<(String, i64)> {
    match &batch.records.as_slice()[0].outcome {
        StatisticalOutcome::Advisory { scores, .. } => scores
            .as_slice()
            .iter()
            .map(|s| (s.option_id.clone(), s.score.value))
            .collect(),
        other => panic!("expected an advisory answer, got {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R703.3
#[tokio::test]
async fn decide_routes_skill_evolution() {
    // The embedding graph is read through the graph ACL, which needs a provisioned caller.
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let (schema, _) = publish_route_schema(&h);
    // skill-a and skill-b overlap in three of four capabilities and embed
    // alike; skill-c has the same capabilities as skill-a but a different
    // embedding; skill-x belongs to another tenant.
    publish_skill(&h, TENANT, "skill-a", &["cap-1", "cap-2", "cap-3"]);
    publish_skill(&h, TENANT, "skill-b", &["cap-1", "cap-2", "cap-3", "cap-4"]);
    publish_skill(&h, TENANT, "skill-c", &["cap-1", "cap-2", "cap-3"]);
    publish_skill(&h, OTHER_TENANT, "skill-x", &["cap-1", "cap-2", "cap-3"]);
    super::learning_tests::graph_with(
        &h,
        VECTORS,
        "m1",
        3,
        vec![
            ("skill-a".to_string(), vec![1.0, 0.0, 0.0]),
            ("skill-b".to_string(), vec![0.99, 0.1, 0.0]),
            ("skill-c".to_string(), vec![0.0, 1.0, 0.0]),
        ],
    )
    .await;

    let batch = decide(
        &h,
        evolution_request(&schema, TENANT, vec![embedding_graph_param()]),
    )
    .await
    .unwrap();
    let merge_score = 750_000_i64 * (1_i64 << 32) / 1_000_000;
    assert_eq!(
        options(&batch),
        vec![
            ("gap:skill-missing".to_string(), 1_i64 << 32),
            // The other tenant's skill is no skill of this caller: a gap, never a merge.
            ("gap:skill-x".to_string(), 1_i64 << 32),
            ("neologism:frobnicate".to_string(), 1_i64 << 32),
            ("merge:skill-a+skill-b".to_string(), merge_score),
        ]
    );
    let record = &batch.records.as_slice()[0];
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact == "unresolved_capability_iri:frobnicate:3"));

    // Without an embedding graph no skill has a vector, so nothing merges.
    let batch = decide(&h, evolution_request(&schema, TENANT, Vec::new()))
        .await
        .unwrap();
    assert!(options(&batch)
        .iter()
        .all(|(id, _)| !id.starts_with("merge:")));

    // The vectors are engine state: a request that carries them is refused.
    let forged = TypedParam {
        name: PARAM_SKILL_EMBEDDINGS.to_string(),
        value: TypedValue::Text(r#"{"skill-a":[1]}"#.to_string()),
    };
    let refused = decide(&h, evolution_request(&schema, TENANT, vec![forged]))
        .await
        .unwrap_err();
    assert!(refused.contains("written by the engine"), "{refused}");

    // A library scope of another kind is refused.
    let mut wrong = evolution_request(&schema, TENANT, Vec::new());
    wrong.candidates = CandidateSource::AgentLibrary {
        scope: LibraryCandidateScope {
            kinds: BoundedVec::new(vec![AgentComponentKind::Tool]).unwrap(),
            classification_under: None,
        },
    };
    assert!(decide(&h, wrong).await.is_err());

    // Cross tenant: the other tenant sees only its own skill, so skill-x is no
    // gap for it and none of this tenant's skills is.
    let other_schema = publish_route_schema_in(&h, OTHER_TENANT);
    let other = evolution_request(&other_schema, OTHER_TENANT, Vec::new());
    let verified_other =
        VerifiedRequestContext::verified_for_test_in_tenant("decider", OTHER_TENANT);
    let batch: DecisionBatch =
        decode(super::super::statistical::handle_decide(&h.state, 1, &verified_other, other).await)
            .unwrap();
    let seen = options(&batch);
    assert!(seen.iter().any(|(id, _)| id == "gap:skill-a"));
    assert!(seen.iter().any(|(id, _)| id == "gap:skill-missing"));
    assert!(seen.iter().all(|(id, _)| id != "gap:skill-x"));
    assert!(seen.iter().all(|(id, _)| !id.starts_with("merge:")));
}

// spec: MEDULLA-DECISION-ENGINE-R703.4
#[tokio::test]
async fn merge_pair_refusals() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    publish_skill(&h, TENANT, "skill-a", &["cap-1", "cap-2", "cap-3"]);
    publish_skill(&h, TENANT, "skill-b", &["cap-1", "cap-2", "cap-3", "cap-4"]);
    publish_acting_skill(&h, TENANT, "skill-w", &["cap-1", "cap-2", "cap-3"]);
    publish_skill(&h, OTHER_TENANT, "skill-x", &["cap-1", "cap-2", "cap-3"]);
    let revisions = |id: &str| h.store.revisions(TENANT, id).unwrap().len();
    let before = ["skill-a", "skill-b", "skill-w"].map(revisions);

    let pair = |a: &str, b: &str| vec![text(PARAM_PAIR_A, a), text(PARAM_PAIR_B, b)];
    let refusal = |a: &str, b: &str| {
        let request = evolution_request(&schema, TENANT, pair(a, b));
        decide(&h, request)
    };

    // Two skills of different side-effect classes never merge.
    let mismatch = refusal("skill-a", "skill-w").await.unwrap_err();
    assert!(mismatch.contains("MERGE_CLASS_MISMATCH"), "{mismatch}");

    // A skill of another tenant and a skill nobody published refuse alike, so
    // the refusal never confirms that a skill exists.
    let foreign = refusal("skill-a", "skill-x").await.unwrap_err();
    let absent = refusal("skill-a", "skill-nowhere").await.unwrap_err();
    assert!(foreign.contains("MERGE_PAIR_NOT_VISIBLE"), "{foreign}");
    assert_eq!(foreign, absent);
    // An invisible skill is judged before any class comparison.
    let both = refusal("skill-w", "skill-x").await.unwrap_err();
    assert!(both.contains("MERGE_PAIR_NOT_VISIBLE"), "{both}");

    // One id alone, or the same id twice, is not a pair.
    let lone = decide(
        &h,
        evolution_request(&schema, TENANT, vec![text(PARAM_PAIR_A, "skill-a")]),
    )
    .await
    .unwrap_err();
    assert!(lone.contains("PARAMETER_INVALID"), "{lone}");
    assert!(refusal("skill-a", "skill-a").await.is_err());

    // A valid pair is answered, and no candidate mutated the library.
    assert!(refusal("skill-a", "skill-b").await.is_ok());
    assert_eq!(["skill-a", "skill-b", "skill-w"].map(revisions), before);
}

// spec: MEDULLA-DECISION-ENGINE-R703.5
#[tokio::test]
async fn candidate_cites_traces_and_library_rows() {
    // The embedding graph is read through the graph ACL, which needs a provisioned caller.
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let (schema, _) = publish_route_schema(&h);
    let a = h
        .commit_in(
            TENANT,
            skill_draft(TENANT, "skill-a", &["cap-1", "cap-2", "cap-3"]),
            None,
        )
        .unwrap();
    let b = h
        .commit_in(
            TENANT,
            skill_draft(TENANT, "skill-b", &["cap-1", "cap-2", "cap-3", "cap-4"]),
            None,
        )
        .unwrap();
    super::learning_tests::graph_with(
        &h,
        VECTORS,
        "m1",
        3,
        vec![
            ("skill-a".to_string(), vec![1.0, 0.0, 0.0]),
            ("skill-b".to_string(), vec![0.99, 0.1, 0.0]),
        ],
    )
    .await;
    let traces = r#"{"skill-missing":["trace-1","trace-2"],"frobnicate":["trace-3"],"skill-a":["trace-4"],"skill-b":["trace-4","trace-5"]}"#;
    let batch = decide(
        &h,
        evolution_request(
            &schema,
            TENANT,
            vec![
                embedding_graph_param(),
                text(PARAM_CANDIDATE_TRACES, traces),
            ],
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let fact = |name: &str| {
        record
            .premises
            .iter()
            .find(|p| p.fact.starts_with(name))
            .unwrap_or_else(|| panic!("no premise {name}: {:?}", record.premises))
    };

    // Every candidate cites the run traces it rests on, as a claim.
    for (name, cited) in [
        ("candidate:gap:skill-missing:", "trace-1,trace-2"),
        ("candidate:neologism:frobnicate:", "trace-3"),
        ("candidate:merge:skill-a+skill-b:", "trace-4,trace-5"),
    ] {
        let premise = fact(name);
        assert_eq!(premise.fact, format!("{name}traces={cited}"));
        assert_eq!(premise.class, PremiseClass::Claim);
    }
    // Every cited library row is a definition bound to its publisher.
    for (id, dependency) in [("skill-a", &a), ("skill-b", &b)] {
        let premise = fact(&format!("library:{id}"));
        assert_eq!(premise.class, PremiseClass::Definition);
        assert_eq!(
            premise.provenance,
            PremiseProvenance::Publisher {
                component_id: id.to_string(),
                definition_digest: dependency.definition_digest.clone(),
            }
        );
    }
    // The weakest class is claim: the answer is advisory.
    assert_eq!(record.evidence_class, EvidenceClass::Claim);

    // The trace map is typed: text that is not a trace map is refused.
    let bad = decide(
        &h,
        evolution_request(&schema, TENANT, vec![text(PARAM_CANDIDATE_TRACES, "[1]")]),
    )
    .await
    .unwrap_err();
    assert!(bad.contains("PARAMETER_INVALID"), "{bad}");
}

// spec: MEDULLA-DECISION-ENGINE-R703.6
#[tokio::test]
async fn candidates_are_proposals_only() {
    use medulla_decision_types::decision::{EvidenceClass, ResolutionKind};
    // The embedding graph is read through the graph ACL, which needs a provisioned caller.
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let (schema, _) = publish_route_schema(&h);
    publish_skill(&h, TENANT, "skill-a", &["cap-1", "cap-2", "cap-3"]);
    publish_skill(&h, TENANT, "skill-b", &["cap-1", "cap-2", "cap-3", "cap-4"]);
    publish_acting_skill(&h, TENANT, "skill-w", &["cap-1", "cap-2", "cap-3"]);
    publish_skill(&h, OTHER_TENANT, "skill-x", &["cap-1", "cap-2", "cap-3"]);
    super::learning_tests::graph_with(
        &h,
        VECTORS,
        "m1",
        3,
        vec![
            ("skill-a".to_string(), vec![1.0, 0.0, 0.0]),
            ("skill-b".to_string(), vec![0.99, 0.1, 0.0]),
        ],
    )
    .await;
    let library = |tenant: &str| {
        let scope = LibraryCandidateScope {
            kinds: BoundedVec::new(vec![AgentComponentKind::Skill]).unwrap(),
            classification_under: None,
        };
        super::super::candidates::library_candidates(&h.store, tenant, &scope)
            .unwrap()
            .entries
            .iter()
            .map(|e| (e.component_id.clone(), e.definition_digest.clone()))
            .collect::<Vec<_>>()
    };
    let before = (library(TENANT), library(OTHER_TENANT));

    // Requested skills and terms in, gaps, neologisms and merges out.
    let batch = decide(
        &h,
        evolution_request(&schema, TENANT, vec![embedding_graph_param()]),
    )
    .await
    .unwrap();
    let seen = options(&batch);
    for expected in [
        "gap:skill-missing",
        "neologism:frobnicate",
        "merge:skill-a+skill-b",
    ] {
        assert!(seen.iter().any(|(id, _)| id == expected), "{expected}");
    }
    // The outcome is advisory and statistical; a proposal rests on claims.
    let record = &batch.records.as_slice()[0];
    assert!(matches!(
        record.outcome,
        StatisticalOutcome::Advisory { .. }
    ));
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact == "unresolved_capability_iri:frobnicate:3"));

    // A refusal changes nothing either: skills of different side-effect
    // classes never merge.
    let pair = vec![text(PARAM_PAIR_A, "skill-a"), text(PARAM_PAIR_B, "skill-w")];
    let refused = decide(&h, evolution_request(&schema, TENANT, pair))
        .await
        .unwrap_err();
    assert!(refused.contains("MERGE_CLASS_MISMATCH"), "{refused}");

    // Cross tenant: the other tenant's skill is neither a candidate nor a
    // merge partner for this tenant, and this tenant's skills are none for it.
    assert!(seen
        .iter()
        .all(|(id, _)| !id.contains("skill-x") || id == "gap:skill-x"));
    let other_schema = publish_route_schema_in(&h, OTHER_TENANT);
    let verified_other =
        VerifiedRequestContext::verified_for_test_in_tenant("decider", OTHER_TENANT);
    let other: DecisionBatch = decode(
        super::super::statistical::handle_decide(
            &h.state,
            1,
            &verified_other,
            evolution_request(&other_schema, OTHER_TENANT, Vec::new()),
        )
        .await,
    )
    .unwrap();
    assert!(options(&other)
        .iter()
        .all(|(id, _)| id != "gap:skill-x" && !id.starts_with("merge:")));

    // The library is exactly as published: nothing was created, merged or
    // retired by asking.
    assert_eq!(before, (library(TENANT), library(OTHER_TENANT)));
}

fn skill_draft(
    tenant: &str,
    id: &str,
    capabilities: &[&str],
) -> medulla_agent_types::agent_component::AgentComponentDraft {
    let mut draft = test_component_draft(tenant, id);
    draft.kind = AgentComponentKind::Skill;
    draft.declared_capabilities = capabilities.iter().map(|c| (*c).to_string()).collect();
    draft
}
