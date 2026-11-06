//! Served acceptance of the model media capability on the assembly answer
//! (MEDULLA-DECISION-ENGINE-R815): a `Solved` assembly over a capable model
//! answers its revision and declared input capabilities, and a required
//! media type on a model that declares nothing is refused by code.

#[cfg(feature = "redb")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_agent_types::agent_component::{
    AgentComponentFacts, AgentComponentKind, AgentComponentPublishRequest, ModalityFacts,
    ModelInputCapabilities, ModelInputMediaType, PromptMode,
};
use medulla_decision_types::decision::{
    AssemblyRequest, AssemblyRequirements, AssemblyResult, DecisionOutcome, DecisionPolicyRef,
    LibraryCandidateScope,
};
use medulla_persistence_records::agent_component::test_component_draft;
use medulla_persistence_records::agent_fixtures::mutation_context;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::BoundedVec;

const TENANT: &str = "tenant-media";

struct Harness {
    _dir: tempfile::TempDir,
    state: Arc<RwLock<ServerState>>,
    store: Arc<AgentLibraryStore>,
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let mut server = ServerState::new_for_test(
        "assemble-media-test-secret",
        medulla_isolation::isolation::IsolationLayer::new(),
    );
    server.persist_dir = Some(dir.path().to_string_lossy().into_owned());
    let state = Arc::new(RwLock::new(server));
    let store = state.write().await.ensure_agent_library().unwrap();
    Harness {
        _dir: dir,
        state,
        store,
    }
}

fn model_facts(input_capabilities: Option<ModelInputCapabilities>) -> AgentComponentFacts {
    AgentComponentFacts::ModelProfile {
        provider: "provider".to_string(),
        model_identity: "model-identity".to_string(),
        context_window_tokens: 128_000,
        max_output_tokens: 4_096,
        supports_tools: true,
        supports_structured_output: true,
        supports_vision: true,
        input_capabilities,
        modalities: ModalityFacts {
            input: vec!["eg:modality/text".to_string()],
            output: vec!["eg:modality/text".to_string()],
        },
        cost: None,
        latency_declared: None,
        latency_observed_ref: None,
        shell_proficiency: None,
        stopping_behaviour: None,
    }
}

fn publish(
    store: &AgentLibraryStore,
    nonce: u8,
    (id, kind): (&str, AgentComponentKind),
    facts: Option<AgentComponentFacts>,
) {
    let mut draft = test_component_draft(TENANT, id);
    draft.kind = kind;
    if let Some(facts) = facts {
        draft.facts = facts;
    } else if kind != AgentComponentKind::Tool {
        draft.facts = AgentComponentFacts::Opaque;
    }
    store
        .publish_component(AgentComponentPublishRequest {
            context: mutation_context(
                store,
                TENANT,
                &format!("seed:{id}"),
                nonce,
                0,
                "agent-component:publish",
            ),
            evaluation_receipt_digest: None,
            component: draft,
        })
        .expect("publishes");
}

/// One model (declaring `capabilities`), one prompt, one skill, one ontology.
fn seed(store: &AgentLibraryStore, capabilities: Option<ModelInputCapabilities>) {
    publish(
        store,
        1,
        ("model-a", AgentComponentKind::ModelProfile),
        Some(model_facts(capabilities)),
    );
    publish(
        store,
        2,
        ("prompt-a", AgentComponentKind::SystemPrompt),
        Some(AgentComponentFacts::SystemPrompt {
            prompt_mode: PromptMode::Static,
            token_estimate: 1_000,
            variables: Vec::new(),
            body_sha256: None,
            schema_revision: None,
        }),
    );
    publish(store, 3, ("skill-a", AgentComponentKind::Skill), None);
    publish(store, 4, ("ontology-a", AgentComponentKind::Ontology), None);
}

fn request(required: Vec<ModelInputMediaType>) -> AssemblyRequest {
    AssemblyRequest {
        tenant_id: TENANT.to_string(),
        requirements: AssemblyRequirements {
            required_input_media: BoundedVec::new(required).unwrap(),
            ..AssemblyRequirements::default()
        },
        candidates: LibraryCandidateScope {
            kinds: BoundedVec::new(vec![
                AgentComponentKind::ModelProfile,
                AgentComponentKind::SystemPrompt,
                AgentComponentKind::Skill,
                AgentComponentKind::Ontology,
            ])
            .unwrap(),
            classification_under: None,
        },
        templates: BoundedVec::default(),
        policy: DecisionPolicyRef::Default,
        solver: None,
    }
}

async fn assemble(h: &Harness, request: AssemblyRequest) -> Result<AssemblyResult, String> {
    // Assembly reads the caller's run-scope bound from the request graph
    // (MEDULLA-IDENTITY-R008.4), so the graph it names must exist.
    {
        let mut state = h.state.write().await;
        if state.registry.get("default").is_none() {
            state
                .registry
                .create_graph(
                    "default",
                    medulla_protocol_types::protocol::GraphType::Global,
                    None,
                )
                .expect("create the request graph");
        }
    }
    let verified = VerifiedRequestContext::verified_for_test_in_tenant("asker", TENANT);
    let response =
        super::super::assemble::handle_agent_assemble(&h.state, 1, ("default", &verified), request)
            .await;
    if let Some(error) = response.error {
        return Err(error);
    }
    match response.result {
        Some(ResultPayload::Raw(bytes)) => Ok(rmp_serde::from_slice(&bytes).unwrap()),
        other => panic!("unexpected payload {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R815
/// Verifies MEDULLA-DECISION-ENGINE-R815: Model media capability on the model selection answer.
#[tokio::test]
async fn model_media_capability_and_unknown_code() {
    // A capable model: the answer carries its revision and declaration.
    let capable = ModelInputCapabilities {
        media_types: vec![ModelInputMediaType::Text, ModelInputMediaType::Png],
        max_aggregate_bytes: 1_048_576,
        max_parts: 8,
    };
    let h = harness().await;
    seed(&h.store, Some(capable.clone()));
    let result = assemble(&h, request(vec![ModelInputMediaType::Png]))
        .await
        .expect("a capable model is solved");
    let DecisionOutcome::Solved { slots, .. } = &result.record.outcome else {
        panic!(
            "expected a solved assembly, got {:?}",
            result.record.outcome
        );
    };
    let model = slots
        .iter()
        .find(|slot| slot.component.kind == AgentComponentKind::ModelProfile)
        .expect("a model slot");
    let revision = result.model_revision.as_deref().expect("model_revision");
    assert_eq!(revision.len(), 64);
    assert!(revision
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    assert!(model.component.definition_digest.ends_with(revision));
    assert_eq!(result.model_input, Some(capable));

    // No required media: never refused, and an undeclared profile answers None.
    let bare = harness().await;
    seed(&bare.store, None);
    let result = assemble(&bare, request(Vec::new()))
        .await
        .expect("no required media is never refused");
    assert!(result.model_revision.is_some());
    assert_eq!(result.model_input, None);

    // A required media type on a model without input_capabilities: refused by code.
    let error = assemble(&bare, request(vec![ModelInputMediaType::Png]))
        .await
        .expect_err("an undeclared model is refused");
    assert!(
        error.starts_with("MODEL_MEDIA_CAPABILITY_UNKNOWN"),
        "unexpected refusal {error}"
    );
}
