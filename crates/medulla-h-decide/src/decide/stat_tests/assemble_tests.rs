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
use medulla_decision_types::decision::coding_agent::{
    CodingAgentRequest, SkillTransformation, VerifiedAgentFact,
};
use medulla_decision_types::decision::primitive::{
    EFFECTS_ATTRIBUTE, INPUT_SCHEMA_ATTRIBUTE, OUTPUT_SCHEMA_ATTRIBUTE,
};
use medulla_decision_types::decision::{
    AssemblyRequest, AssemblyRequirements, AssemblyResult, DecisionOutcome, DecisionPolicyRef,
    LibraryCandidateScope, PrimitiveEdge, PrimitiveReplacement, ResolutionKind, Violation,
};
use medulla_persistence_records::agent_component::test_component_draft;
use medulla_persistence_records::agent_fixtures::mutation_context;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::BoundedVec;

const TENANT: &str = "tenant-media";
const ASKER: &str = "asker";

struct Harness {
    _dir: tempfile::TempDir,
    state: Arc<RwLock<ServerState>>,
    store: Arc<AgentLibraryStore>,
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    // The graph ACL admits only a principal the provisioned policy knows
    // (`topology_read::graph_core` checks read on the request graph).
    let mut server = ServerState::new_for_test(
        "assemble-media-test-secret",
        ServerState::test_isolation(ASKER),
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
    publish_with(store, nonce, (id, kind), facts, |_| {});
}

fn publish_with(
    store: &AgentLibraryStore,
    nonce: u8,
    (id, kind): (&str, AgentComponentKind),
    facts: Option<AgentComponentFacts>,
    tweak: impl FnOnce(&mut medulla_agent_types::agent_component::AgentComponentDraft),
) {
    let mut draft = test_component_draft(TENANT, id);
    draft.kind = kind;
    if let Some(facts) = facts {
        draft.facts = facts;
    } else if kind != AgentComponentKind::Tool {
        draft.facts = AgentComponentFacts::Opaque;
    }
    tweak(&mut draft);
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

/// One model (declaring `capabilities`), one prompt, one tool, one skill and
/// one ontology: every slot of a runnable agent (`one:tool` is at least one).
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
    publish(store, 5, ("tool-a", AgentComponentKind::Tool), None);
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
                AgentComponentKind::Tool,
                AgentComponentKind::Skill,
                AgentComponentKind::Ontology,
            ])
            .unwrap(),
            classification_under: None,
        },
        templates: BoundedVec::default(),
        policy: DecisionPolicyRef::Default,
        solver: None,
        primitive_edges: BoundedVec::default(),
        replacements: BoundedVec::default(),
        assistant_profile_ref: None,
        coding_agent: None,
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
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(ASKER, TENANT);
    // The candidate set must lie inside the asker's stored run-scope bound
    // (MEDULLA-IDENTITY-R008.1/R008.4); an absent bound refuses every tool.
    {
        let state = h.state.read().await;
        let core = &state.registry.get("default").expect("request graph").core;
        let policy = serde_json::json!({
            "type": medulla_isolation::isolation::RUN_SCOPE_POLICY_CLASS,
            "allowed_tools": [
                "model-a", "prompt-a", "tool-a", "skill-a", "ontology-a", "p-search", "p-sink",
                "p-bad", "r-ok", "r-kind", "r-schema", "r-caps", "r-effect", "tool-web",
            ],
            "allowed_graphs": ["default"],
        });
        core.add_node(
            medulla_isolation::isolation::run_scope_policy_iri(
                &verified.principal_persistence_id(),
            ),
            rmp_serde::to_vec_named(&policy).expect("encode the run-scope policy"),
        );
    }
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

const SEARCH_CAP: &str = "urn:vendor:search";

fn digest(c: char) -> String {
    std::iter::repeat(c).take(64).collect()
}

/// One tool primitive: `input` -> `output` schema digests, comma-separated
/// effects and declared capabilities.
fn publish_primitive(
    store: &AgentLibraryStore,
    nonce: u8,
    (id, kind): (&str, AgentComponentKind),
    (input, output, effects, capabilities): (char, char, &str, &[&str]),
) {
    let facts =
        (kind == AgentComponentKind::SystemPrompt).then(|| AgentComponentFacts::SystemPrompt {
            prompt_mode: PromptMode::Static,
            token_estimate: 10,
            variables: Vec::new(),
            body_sha256: None,
            schema_revision: None,
        });
    publish_with(store, nonce, (id, kind), facts, |draft| {
        draft
            .attributes
            .insert(INPUT_SCHEMA_ATTRIBUTE.to_string(), digest(input));
        draft
            .attributes
            .insert(OUTPUT_SCHEMA_ATTRIBUTE.to_string(), digest(output));
        draft
            .attributes
            .insert(EFFECTS_ATTRIBUTE.to_string(), effects.to_string());
        draft.declared_capabilities = capabilities.iter().map(|c| c.to_string()).collect();
    });
}

/// `p-search` (a -> b, read only, search) feeds `p-sink` (b -> c); `p-bad`
/// (d -> c) does not accept b. One replacement per rule: `r-ok` is safe,
/// `r-kind` is a prompt, `r-schema` changes the output, `r-caps` drops the
/// capability, `r-effect` adds an effect.
fn seed_primitives(store: &AgentLibraryStore) {
    let tool = AgentComponentKind::Tool;
    let cap = &[SEARCH_CAP][..];
    publish_primitive(store, 10, ("p-search", tool), ('a', 'b', "read_only", cap));
    publish_primitive(store, 11, ("p-sink", tool), ('b', 'c', "read_only", &[]));
    publish_primitive(store, 12, ("p-bad", tool), ('d', 'c', "read_only", &[]));
    publish_primitive(store, 13, ("r-ok", tool), ('a', 'b', "", cap));
    let prompt = AgentComponentKind::SystemPrompt;
    publish_primitive(store, 14, ("r-kind", prompt), ('a', 'b', "read_only", cap));
    publish_primitive(store, 15, ("r-schema", tool), ('a', 'c', "read_only", cap));
    publish_primitive(store, 16, ("r-caps", tool), ('a', 'b', "read_only", &[]));
    publish_primitive(
        store,
        17,
        ("r-effect", tool),
        ('a', 'b', "read_only,external", cap),
    );
}

fn edge(producer: &str, consumer: &str) -> PrimitiveEdge {
    PrimitiveEdge {
        producer: producer.to_string(),
        consumer: consumer.to_string(),
    }
}

fn replacement(old: &str, new: &str) -> PrimitiveReplacement {
    PrimitiveReplacement {
        old: old.to_string(),
        new: new.to_string(),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R036.100.3
/// Verifies MEDULLA-DECISION-ENGINE-R036.100.3: primitive checks are wired into `AgentAssemble`.
#[tokio::test]
async fn primitive_refusals_abstain_with_stable_codes() {
    let h = harness().await;
    seed(&h.store, None);
    seed_primitives(&h.store);

    // A well-typed edge and a safe replacement stay legal; a candidate with no
    // contract (`tool-a`) skips the checks on both sides of an edge.
    let mut legal = request(Vec::new());
    legal.primitive_edges = BoundedVec::new(vec![
        edge("p-search", "p-sink"),
        edge("tool-a", "p-sink"),
        edge("p-search", "tool-a"),
    ])
    .unwrap();
    legal.replacements = BoundedVec::new(vec![replacement("p-search", "r-ok")]).unwrap();
    let result = assemble(&h, legal)
        .await
        .expect("a legal wiring is decided");
    assert!(
        matches!(result.record.outcome, DecisionOutcome::Solved { .. }),
        "expected a solved assembly, got {:?}",
        result.record.outcome
    );

    // Each failed check abstains with its stable code and removes one option.
    let cases: [(
        &str,
        Vec<PrimitiveEdge>,
        Vec<PrimitiveReplacement>,
        &str,
        &str,
    ); 5] = [
        (
            "edge",
            vec![edge("p-search", "p-bad")],
            vec![],
            "PRIMITIVE_EDGE_MISMATCH",
            "p-bad",
        ),
        (
            "kind",
            vec![],
            vec![replacement("p-search", "r-kind")],
            "PRIMITIVE_KIND_MISMATCH",
            "r-kind",
        ),
        (
            "schema",
            vec![],
            vec![replacement("p-search", "r-schema")],
            "PRIMITIVE_SCHEMA_CHANGED",
            "r-schema",
        ),
        (
            "capability",
            vec![],
            vec![replacement("p-search", "r-caps")],
            "PRIMITIVE_CAPABILITY_LOST",
            "r-caps",
        ),
        (
            "effect",
            vec![],
            vec![replacement("p-search", "r-effect")],
            "PRIMITIVE_EFFECT_WIDENED",
            "r-effect",
        ),
    ];
    for (name, edges, replacements, code, removed) in cases {
        let mut refused = request(Vec::new());
        refused.primitive_edges = BoundedVec::new(edges).unwrap();
        refused.replacements = BoundedVec::new(replacements).unwrap();
        let result = assemble(&h, refused).await.expect(name);
        let record = &result.record;
        assert_eq!(record.resolution_kind, ResolutionKind::Abstention, "{name}");
        let DecisionOutcome::Abstained { reasons } = &record.outcome else {
            panic!("{name}: expected an abstention, got {:?}", record.outcome);
        };
        assert_eq!(reasons.len(), 1, "{name}");
        assert_eq!(
            reasons.iter().next().unwrap().code().as_str(),
            code,
            "{name}"
        );
        assert!(
            record.eliminated.iter().any(|elimination| {
                elimination.component_id == removed
                    && elimination.violation
                        == Violation::PrimitiveContract {
                            code: code.to_string(),
                        }
            }),
            "{name}: the removed option is recorded as an eliminated constraint"
        );
        assert!(result.graph.is_none(), "{name}");
    }

    // A graph over the primitive.max_edges knob (default 256) is refused by code.
    let mut large = request(Vec::new());
    large.primitive_edges =
        BoundedVec::new(vec![edge("p-search", "p-sink"); 257]).expect("within the wire bound");
    let error = assemble(&h, large).await.expect_err("an oversized graph");
    assert!(
        error.starts_with("PRIMITIVE_GRAPH_TOO_LARGE"),
        "unexpected refusal {error}"
    );

    // An edge naming something outside the candidate scope is an invalid input.
    let mut unknown = request(Vec::new());
    unknown.primitive_edges = BoundedVec::new(vec![edge("p-search", "nobody")]).unwrap();
    let error = assemble(&h, unknown)
        .await
        .expect_err("an unknown consumer");
    assert!(
        error.starts_with("ASSEMBLY_INPUTS_INVALID"),
        "unexpected refusal {error}"
    );

    // Another tenant's request is refused before any check runs.
    let mut foreign = request(Vec::new());
    foreign.tenant_id = "tenant-other".to_string();
    foreign.primitive_edges = BoundedVec::new(vec![edge("p-search", "p-bad")]).unwrap();
    let error = assemble(&h, foreign).await.expect_err("a foreign tenant");
    assert!(
        error.starts_with("ACCESS_DENIED"),
        "unexpected refusal {error}"
    );
}

const WEB: &str = "eg:capability/retrieval/web-search";

/// A tool declaring the native web-search capability.
fn seed_web_tool(store: &AgentLibraryStore) {
    publish_with(
        store,
        30,
        ("tool-web", AgentComponentKind::Tool),
        None,
        |draft| draft.declared_capabilities = vec![WEB.to_string()],
    );
}

/// Store a profile node the way a saved profile is stored, owned by `owner`
/// and scoped to `tenant_scope`.
async fn store_profile(h: &Harness, (tenant_scope, owner): (&str, &str), id: &str, allow: &[&str]) {
    use medulla_types::assistant_profile::{profile_node_id, AssistantProfile, ModelHints};
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
    let profile = AssistantProfile {
        profile_id: id.to_string(),
        tenant: tenant_scope.to_string(),
        name: id.to_string(),
        instructions: "be careful".to_string(),
        document_set_ids: Vec::new(),
        tool_allowlist: allow.iter().map(|tool| tool.to_string()).collect(),
        model_hints: ModelHints {
            preferred_model_profile_refs: Vec::new(),
            max_context_tokens: None,
        },
        owner_principal: owner.to_string(),
        shares: Vec::new(),
        revision: 3,
        schema_version: 1,
    };
    let blob = rmp_serde::to_vec_named(&serde_json::json!({
        "type": "AssistantProfile",
        "tenant_scope": tenant_scope,
        "json": serde_json::to_string(&profile).unwrap(),
    }))
    .unwrap();
    let state = h.state.read().await;
    state
        .registry
        .get("default")
        .expect("request graph")
        .core
        .add_node(profile_node_id(id), blob);
}

fn web_request(profile: Option<&str>) -> AssemblyRequest {
    let mut asked = request(Vec::new());
    asked.requirements.capabilities = BoundedVec::new(vec![WEB.to_string()]).unwrap();
    asked.assistant_profile_ref = profile.map(str::to_string);
    asked
}

fn abstain_codes(result: &AssemblyResult) -> Vec<String> {
    let DecisionOutcome::Abstained { reasons } = &result.record.outcome else {
        panic!("expected an abstention, got {:?}", result.record.outcome);
    };
    reasons
        .iter()
        .map(|reason| match reason {
            medulla_decision_types::decision::record::outcome::AbstainReason::Surface {
                code,
                ..
            } => code.as_str().to_string(),
            other => format!("{other:?}"),
        })
        .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R036.107
/// Verifies MEDULLA-DECISION-ENGINE-R036.107: the profile tool allowlist narrows the candidates.
#[tokio::test]
async fn allowlist_removes_tools_and_admission_still_runs() {
    let h = harness().await;
    seed(&h.store, None);
    seed_web_tool(&h.store);
    store_profile(&h, (TENANT, ASKER), "profile-web", &["tool-web"]).await;
    store_profile(&h, (TENANT, ASKER), "profile-none", &["tool-a"]).await;

    // Without a profile the web tool covers the requirement.
    let open = assemble(&h, web_request(None)).await.expect("solved");
    assert!(matches!(
        open.record.outcome,
        DecisionOutcome::Solved { .. }
    ));
    assert!(open.record.inputs.profile.is_none());

    // The allowlist keeps the web tool, removes every other tool and lists the
    // profile premise; a definition premise never weakens the evidence.
    let allowed = assemble(&h, web_request(Some("profile-web")))
        .await
        .expect("solved under the allowlist");
    let DecisionOutcome::Solved { slots, .. } = &allowed.record.outcome else {
        panic!("expected solved, got {:?}", allowed.record.outcome);
    };
    assert!(slots
        .iter()
        .any(|slot| slot.component.component_id == "tool-web"));
    let constraint = allowed.record.inputs.profile.as_ref().expect("recorded");
    assert_eq!(
        (constraint.profile_id.as_str(), constraint.revision),
        ("profile-web", 3)
    );
    assert!(constraint.removed.iter().any(|id| id == "tool-a"));
    assert!(!constraint.removed.iter().any(|id| id == "tool-web"));
    assert!(allowed
        .record
        .inputs
        .candidates
        .iter()
        .all(|candidate| candidate.kind != AgentComponentKind::Tool
            || candidate.component_id == "tool-web"));
    assert!(allowed
        .record
        .premises
        .iter()
        .any(|premise| premise.subject == "profile:profile-web@3"));
    assert_eq!(allowed.record.evidence_class, open.record.evidence_class);

    // No remaining tool covers the required capability: the existing infeasible
    // outcome, reported under the profile's code, with the premise.
    let blocked = assemble(&h, web_request(Some("profile-none")))
        .await
        .expect("an abstention is a record");
    assert_eq!(blocked.record.resolution_kind, ResolutionKind::Abstention);
    assert_eq!(abstain_codes(&blocked), vec!["PROFILE_TOOL_NOT_ALLOWED"]);
    assert!(blocked.graph.is_none());
    assert!(blocked
        .record
        .premises
        .iter()
        .any(|premise| premise.subject == "profile:profile-none@3"));

    // A profile that does not exist, another tenant's, and another principal's
    // unshared profile all answer with the one non-disclosing refusal.
    store_profile(
        &h,
        ("tenant-other", ASKER),
        "profile-foreign",
        &["tool-web"],
    )
    .await;
    store_profile(
        &h,
        (TENANT, "someone-else"),
        "profile-private",
        &["tool-web"],
    )
    .await;
    for id in ["profile-missing", "profile-foreign", "profile-private"] {
        let error = assemble(&h, web_request(Some(id)))
            .await
            .expect_err("the profile is not visible");
        assert!(error.starts_with("PROFILE_NOT_VISIBLE"), "{id}: {error}");
    }

    // The profile never bypasses admission: a candidate outside the caller's
    // run-scope policy still refuses the request, although the allowlist
    // removes it from the solve.
    publish(&h.store, 31, ("tool-out", AgentComponentKind::Tool), None);
    let error = assemble(&h, web_request(Some("profile-web")))
        .await
        .expect_err("admission runs over the whole scope");
    assert!(
        error.starts_with("SCOPE_EXCEEDS_POLICY"),
        "unexpected refusal {error}"
    );

    // Another tenant's request is refused before the profile is read.
    let mut foreign = web_request(Some("profile-web"));
    foreign.tenant_id = "tenant-other".to_string();
    let error = assemble(&h, foreign).await.expect_err("a foreign tenant");
    assert!(
        error.starts_with("ACCESS_DENIED"),
        "unexpected refusal {error}"
    );
}

fn skill(id: &str, (input, output): (char, char)) -> SkillTransformation {
    SkillTransformation {
        skill_id: id.to_string(),
        version: "1".to_string(),
        input_schema_digest: digest(input),
        output_schema_digest: digest(output),
    }
}

fn coding_request(
    (receipt, age): (&str, u64),
    skills: Vec<SkillTransformation>,
    artifact: char,
) -> AssemblyRequest {
    let mut asked = request(Vec::new());
    asked.coding_agent = Some(CodingAgentRequest {
        agent: VerifiedAgentFact {
            agent_id: "coder".to_string(),
            revision: "r7".to_string(),
            capabilities: BoundedVec::new(vec![WEB.to_string()]).unwrap(),
            discovery_receipt_digest: receipt.to_string(),
            discovery_age_s: age,
        },
        skills: BoundedVec::new(skills).unwrap(),
        artifact_contract_digest: digest(artifact),
    });
    asked
}

// spec: MEDULLA-DECISION-ENGINE-R036.101
/// Verifies MEDULLA-DECISION-ENGINE-R036.101: a verified agent composes with ordered skills.
#[tokio::test]
async fn coding_agent_plan_orders_skills_and_refuses_stale_receipts() {
    use medulla_decision_types::decision::PremiseClass;
    let h = harness().await;
    seed(&h.store, None);
    let receipt = digest('9');
    let chain = || vec![skill("lint", ('a', 'b')), skill("patch", ('b', 'c'))];

    let planned = assemble(&h, coding_request((&receipt, 10), chain(), 'c'))
        .await
        .expect("a verified plan is decided");
    assert!(matches!(
        planned.record.outcome,
        DecisionOutcome::Solved { .. }
    ));
    let plan = planned.coding_agent_plan.as_ref().expect("the plan");
    assert_eq!(
        (plan.agent_id.as_str(), plan.agent_revision.as_str()),
        ("coder", "r7")
    );
    let order: Vec<&str> = plan.skills.iter().map(|s| s.skill_id.as_str()).collect();
    assert_eq!(order, ["lint", "patch"]);
    assert_eq!(plan.artifact_contract_digest, digest('c'));
    assert_eq!(
        planned.record.evidence_class,
        medulla_types::evidence_class::EvidenceClass::Claim
    );
    let claims: Vec<(String, PremiseClass)> = planned
        .record
        .premises
        .iter()
        .filter(|premise| {
            premise.subject.starts_with("agent:")
                || premise.subject.starts_with("receipt:")
                || premise.subject.starts_with("skill:")
        })
        .map(|premise| (premise.subject.clone(), premise.class))
        .collect();
    let claim = PremiseClass::Claim;
    assert_eq!(
        claims,
        vec![
            ("agent:coder@r7".to_string(), claim),
            (format!("receipt:{receipt}"), claim),
            ("skill:lint@1".to_string(), claim),
            ("skill:patch@1".to_string(), claim),
        ]
    );

    // Each refusal abstains with its stable code and composes no plan.
    let cases = [
        (
            "missing",
            coding_request(("", 0), chain(), 'c'),
            "DISCOVERY_RECEIPT_MISSING",
        ),
        (
            "stale",
            coding_request((&receipt, 3_601), chain(), 'c'),
            "DISCOVERY_RECEIPT_STALE",
        ),
        (
            "mismatch",
            coding_request(
                (&receipt, 10),
                vec![skill("lint", ('a', 'b')), skill("patch", ('x', 'c'))],
                'c',
            ),
            "TRANSFORMATION_SCHEMA_MISMATCH",
        ),
        (
            "artifact",
            coding_request((&receipt, 10), chain(), 'd'),
            "ARTIFACT_DIGEST_CHANGED",
        ),
    ];
    for (name, refused, code) in cases {
        let result = assemble(&h, refused).await.expect(name);
        assert_eq!(
            result.record.resolution_kind,
            ResolutionKind::Abstention,
            "{name}"
        );
        assert_eq!(abstain_codes(&result), vec![code.to_string()], "{name}");
        assert!(
            result.coding_agent_plan.is_none() && result.graph.is_none(),
            "{name}"
        );
    }
    // A receipt exactly at the knob is fresh.
    assemble(&h, coding_request((&receipt, 3_600), chain(), 'c'))
        .await
        .expect("a receipt at the limit is fresh");

    // Another tenant's request is refused before the plan is composed.
    let mut foreign = coding_request((&receipt, 10), chain(), 'c');
    foreign.tenant_id = "tenant-other".to_string();
    let error = assemble(&h, foreign).await.expect_err("a foreign tenant");
    assert!(
        error.starts_with("ACCESS_DENIED"),
        "unexpected refusal {error}"
    );
}
