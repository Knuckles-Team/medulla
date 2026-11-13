//! The `skill_evolution` family through the actual Decide handler
//! (MEDULLA-DECISION-ENGINE-R703.3): a requested skill no visible skill carries
//! is a gap, a term the capability IRI resolver rejects and runs used often
//! enough is a neologism, and two near-duplicate skills are a merge candidate
//! scored by their overlap. The answer is advisory and writes nothing, and one
//! tenant never sees another tenant's skills.

use super::*;
use medulla_decision_types::decision::skill_evolution::{
    PARAM_EMBEDDING_GRAPH, PARAM_REQUESTED_SKILLS, PARAM_SKILL_EMBEDDINGS, PARAM_USED_TERMS,
};
use medulla_server_api::authority_context::VerifiedRequestContext;

const OTHER_TENANT: &str = "tenant-other";
const VECTORS: &str = "skill-vectors";

fn publish_skill(h: &Harness, tenant: &str, id: &str, capabilities: &[&str]) {
    let mut draft = test_component_draft(tenant, id);
    draft.kind = AgentComponentKind::Skill;
    draft.declared_capabilities = capabilities.iter().map(|c| (*c).to_string()).collect();
    h.commit_in(tenant, draft, None).unwrap();
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
    let h = Harness::new().await;
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
