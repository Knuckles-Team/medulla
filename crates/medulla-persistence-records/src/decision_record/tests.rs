//! `DecisionCommit`'s store half: the planted catalog-side inputs of
//! DECIDE-LAYER-DESIGN §11.1 and the outcome link of §4.6 (EH-070).

use medulla_agent_types::agent_component::ComponentDependency;
use medulla_agent_types::agent_graph::AgentGraphPublishRequest;
use medulla_agent_types::agent_library::AgentLibraryPublishRequest;
use medulla_agent_types::{
    agent_component::AgentComponentDraft, agent_component::AgentComponentFacts,
    agent_component::AgentComponentKind, agent_component::AgentComponentPublishRequest,
    agent_component::ModalityFacts, agent_component::PromptMode, agent_component::ToolEffect,
};
use medulla_compute::assemble::{assemble, inputs, replay_check, Assembly, RecordIdentity};
#[cfg(test)]
use medulla_decision_types::decision::DecisionOutcome;
use medulla_decision_types::{
    decision::AssemblyRequest, decision::AssemblyRequirements, decision::DecisionPolicy,
    decision::DecisionPolicyRef, decision::LibraryCandidateScope,
};
use medulla_types::contract::BoundedVec;

use super::super::agent_component::{test_component_draft, test_tool_facts};
use super::super::agent_fixtures::mutation_context;
#[cfg(test)]
use super::super::agent_fixtures::open_agent_store;
use super::*;

pub const TENANT: &str = "tenant-a";

fn publish(
    store: &AgentLibraryStore,
    nonce: u8,
    draft: AgentComponentDraft,
) -> AgentComponentEntry {
    let key = format!("seed:{}", draft.component_id);
    store
        .publish_component(AgentComponentPublishRequest {
            context: mutation_context(store, TENANT, &key, nonce, 0, "agent-component:publish"),
            evaluation_receipt_digest: None,
            component: draft,
        })
        .expect("publishes")
        .result
        .component
}

fn draft(
    id: &str,
    kind: AgentComponentKind,
    facts: AgentComponentFacts,
    class: &str,
) -> AgentComponentDraft {
    AgentComponentDraft {
        kind,
        facts,
        classification: vec![class.to_string()],
        ..test_component_draft(TENANT, id)
    }
}

pub fn seed_library(store: &AgentLibraryStore) {
    let model = AgentComponentFacts::ModelProfile {
        provider: "provider".into(),
        model_identity: "model-a".into(),
        context_window_tokens: 64_000,
        max_output_tokens: 1_000,
        supports_tools: true,
        supports_structured_output: true,
        supports_vision: false,
        input_capabilities: None,
        modalities: ModalityFacts::default(),
        cost: None,
        latency_declared: None,
        latency_observed_ref: None,
        shell_proficiency: None,
        stopping_behaviour: None,
    };
    let prompt = AgentComponentFacts::SystemPrompt {
        prompt_mode: PromptMode::Static,
        token_estimate: 500,
        variables: Vec::new(),
        body_sha256: None,
        schema_revision: None,
    };
    let drafts = [
        draft(
            "model-a",
            AgentComponentKind::ModelProfile,
            model,
            "eg:capability/generation/text",
        ),
        draft(
            "prompt-a",
            AgentComponentKind::SystemPrompt,
            prompt,
            "eg:capability/reasoning/plan",
        ),
        draft(
            "skill-a",
            AgentComponentKind::Skill,
            AgentComponentFacts::Opaque,
            "eg:capability/reasoning/plan",
        ),
        draft(
            "ontology-a",
            AgentComponentKind::Ontology,
            AgentComponentFacts::Opaque,
            "eg:capability/reasoning/plan",
        ),
        draft(
            "tool-web",
            AgentComponentKind::Tool,
            test_tool_facts(ToolEffect::Read),
            "eg:capability/retrieval/web-search",
        ),
    ];
    for (nonce, draft) in drafts.into_iter().enumerate() {
        publish(store, 100 + nonce as u8, draft);
    }
}

fn request() -> AssemblyRequest {
    AssemblyRequest {
        tenant_id: TENANT.to_string(),
        requirements: AssemblyRequirements {
            capabilities: BoundedVec::new(vec!["eg:capability/retrieval".to_string()]).unwrap(),
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
    }
}

pub fn decided(store: &AgentLibraryStore) -> Assembly {
    let entries = store
        .assembly_candidates(TENANT, &request().candidates)
        .expect("reads");
    let candidates = candidate_facts(&entries).expect("facts");
    let inputs = inputs(
        request(),
        candidates,
        Vec::new(),
        DecisionPolicy::engine_default(),
    )
    .expect("inputs");
    let identity = RecordIdentity {
        tenant_id: TENANT.to_string(),
        caller_principal: "caller-a".to_string(),
        created_at_ms: 20,
    };
    let assembly = assemble(inputs, identity).expect("assembles");
    assert!(assembly.record.is_solved(), "{:?}", assembly.record.outcome);
    replay_check(&assembly.record).expect("replays");
    assembly
}

pub fn commit_context(
    store: &AgentLibraryStore,
    key: &str,
    nonce: u8,
) -> AgentLibraryMutationContext {
    mutation_context(store, TENANT, key, nonce, 0, "decision:commit")
}

// spec: MEDULLA-DECISION-ENGINE-R019
// spec: MEDULLA-DECISION-ENGINE-R051
#[test]
fn a_record_commits_once_serves_its_body_and_replays_idempotently() {
    let (_dir, store) = open_agent_store();
    seed_library(&store);
    let record = decided(&store).record;
    let digest = record.inputs.catalog_digest.clone();
    let first = store
        .commit_decision_record(commit_context(&store, "commit-1", 1), &record, &digest)
        .expect("commits");
    assert!(!first.replayed);
    assert_eq!(
        first.result.component.kind,
        AgentComponentKind::DecisionRecord
    );
    assert_eq!(first.result.component.component_id, record.record_id);
    let content = store
        .decision_record_content(TENANT, &record.record_id)
        .expect("the verbatim body is served");
    let served: DecisionRecord = serde_json::from_slice(&content.body).expect("decodes");
    assert_eq!(served, record);
    let again = store
        .commit_decision_record(commit_context(&store, "commit-2", 2), &record, &digest)
        .expect("a repeat commit is a replay");
    assert!(again.replayed);
    assert_eq!(again.result.component, first.result.component);
}

// spec: MEDULLA-DECISION-ENGINE-R019
#[test]
fn a_catalog_that_moved_since_the_decision_is_a_stale_refusal() {
    let (_dir, store) = open_agent_store();
    seed_library(&store);
    let record = decided(&store).record;
    publish(
        &store,
        120,
        draft(
            "tool-vector",
            AgentComponentKind::Tool,
            test_tool_facts(ToolEffect::Read),
            "eg:capability/retrieval/vector-search",
        ),
    );
    let error = store
        .commit_decision_record(
            commit_context(&store, "commit-stale", 3),
            &record,
            &record.inputs.catalog_digest,
        )
        .expect_err("stale");
    assert!(error.starts_with("STALE_CATALOG: "), "{error}");
}

// spec: MEDULLA-DECISION-ENGINE-R019
// spec: MEDULLA-DECISION-ENGINE-R051
#[test]
fn a_fabricated_record_whose_facts_differ_from_the_published_revision_is_refused() {
    let (_dir, store) = open_agent_store();
    seed_library(&store);
    let mut record = decided(&store).record;
    let mut candidates = record.inputs.candidates.as_slice().to_vec();
    candidates[0].classification = BoundedVec::new(vec!["eg:capability".to_string()]).unwrap();
    record.inputs.candidates = BoundedVec::new(candidates).unwrap();
    let error = store
        .commit_decision_record(
            commit_context(&store, "commit-forged", 4),
            &record,
            &record.inputs.catalog_digest,
        )
        .expect_err("forged");
    assert!(error.starts_with("CANDIDATE_FACTS_CHANGED: "), "{error}");
}

// spec: MEDULLA-DECISION-ENGINE-R051
#[test]
fn a_record_from_another_tenant_is_refused() {
    let (_dir, store) = open_agent_store();
    seed_library(&store);
    let mut record = decided(&store).record;
    record.tenant_id = "tenant-b".to_string();
    let error = store
        .commit_decision_record(
            commit_context(&store, "commit-tenant", 5),
            &record,
            &record.inputs.catalog_digest,
        )
        .expect_err("cross-tenant");
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");
}

/// EH-070: the published graph pins the committed record through
/// `synthesis_evidence`, and following the pin from the graph resolves the
/// record that chose its agent.
// spec: MEDULLA-DECISION-ENGINE-R068
#[test]
fn a_published_graph_resolves_to_its_decision_record_through_synthesis_evidence() {
    let (_dir, store) = open_agent_store();
    seed_library(&store);
    let assembly = decided(&store);
    let record = assembly.record.clone();
    let committed = store
        .commit_decision_record(
            commit_context(&store, "commit-link", 6),
            &record,
            &record.inputs.catalog_digest,
        )
        .expect("commits")
        .result
        .component;
    store
        .publish(AgentLibraryPublishRequest {
            context: mutation_context(&store, TENANT, "agent-link", 7, 0, "agent-library:publish"),
            entry: assembly
                .agents
                .first()
                .cloned()
                .expect("solved answers an agent"),
        })
        .expect("the assembled agent publishes");
    let mut graph = assembly.graph.expect("solved answers a graph");
    graph.synthesis_evidence = Some(ComponentDependency {
        component_id: committed.component_id.clone(),
        kind: AgentComponentKind::DecisionRecord,
        definition_digest: committed.definition_digest.clone(),
    });
    let published = store
        .publish_graph(AgentGraphPublishRequest {
            context: mutation_context(&store, TENANT, "graph-link", 8, 0, "agent-graph:publish"),
            graph,
        })
        .expect("the graph publishes with its evidence pin resolved");
    let evidence = published
        .result
        .graph
        .synthesis_evidence
        .expect("the published graph carries its evidence");
    let content = store
        .decision_record_content(TENANT, &evidence.component_id)
        .expect("the evidence resolves");
    let linked: DecisionRecord = serde_json::from_slice(&content.body).expect("decodes");
    assert_eq!(linked.record_id, record.record_id);
    let DecisionOutcome::Solved { graph_digest, .. } = &linked.outcome else {
        panic!("the linked record is the solved one");
    };
    assert!(graph_digest.starts_with("sha256:"));
}

#[cfg(test)]
fn policy_draft(policy: &DecisionPolicy, content_digest: String) -> AgentComponentDraft {
    let mut draft = draft(
        "policy-tight",
        AgentComponentKind::DecisionPolicy,
        AgentComponentFacts::Opaque,
        "eg:capability",
    );
    draft.classification = Vec::new();
    draft.content_digest = content_digest;
    draft.attributes = std::collections::BTreeMap::from([(
        medulla_decision_types::decision::policy::DECISION_POLICY_ATTRIBUTE.to_string(),
        serde_json::to_string(policy).expect("encodes"),
    )]);
    draft
}

/// A pinned `DecisionPolicy` component is served: its verified body decides
/// the assembly, the record stores it, and the commit re-checks the pin.
#[test]
fn a_pinned_policy_component_decides_and_commits() {
    let (_dir, store) = open_agent_store();
    seed_library(&store);
    let mut policy = DecisionPolicy::engine_default();
    policy.node_budget = 5_000;
    let digest = medulla_decision_types::decision::digest::policy_digest(&policy);
    let published = publish(&store, 130, policy_draft(&policy, digest));
    let pin = ComponentDependency {
        component_id: published.component_id.clone(),
        kind: AgentComponentKind::DecisionPolicy,
        definition_digest: published.definition_digest.clone(),
    };
    let read = store
        .pinned_decision_policy(TENANT, &pin)
        .expect("the pinned body verifies");
    assert_eq!(read, policy);
    let mut asked = request();
    asked.policy = DecisionPolicyRef::Pinned { component: pin };
    let entries = store
        .assembly_candidates(TENANT, &asked.candidates)
        .expect("reads");
    let candidates = candidate_facts(&entries).expect("facts");
    let inputs = inputs(asked, candidates, Vec::new(), read).expect("inputs");
    let identity = RecordIdentity {
        tenant_id: TENANT.to_string(),
        caller_principal: "caller-a".to_string(),
        created_at_ms: 21,
    };
    let record = assemble(inputs, identity).expect("assembles").record;
    assert_eq!(record.inputs.solver.node_budget, 5_000);
    store
        .commit_decision_record(
            commit_context(&store, "commit-pinned", 9),
            &record,
            &record.inputs.catalog_digest,
        )
        .expect("the pinned policy re-checks inside the commit");
}

#[test]
fn a_policy_component_whose_body_does_not_match_its_digest_is_refused() {
    let (_dir, store) = open_agent_store();
    let policy = DecisionPolicy::engine_default();
    let wrong = format!("sha256:{}", "0".repeat(64));
    let error = store
        .publish_component(AgentComponentPublishRequest {
            context: mutation_context(
                &store,
                TENANT,
                "bad-policy",
                131,
                0,
                "agent-component:publish",
            ),
            evaluation_receipt_digest: None,
            component: policy_draft(&policy, wrong),
        })
        .expect_err("refused");
    assert!(error.starts_with("POLICY_BODY_UNAVAILABLE: "), "{error}");
}

/// The evidence graph of a solved assembly, pinned to the committed record, with its agent
/// published so the graph's references resolve.
pub fn evidence_request(
    store: &AgentLibraryStore,
    assembly: &Assembly,
    key: &str,
    nonce: u8,
) -> AgentGraphPublishRequest {
    let record = &assembly.record;
    store
        .publish(AgentLibraryPublishRequest {
            context: mutation_context(
                store,
                TENANT,
                "agent-shared",
                nonce,
                0,
                "agent-library:publish",
            ),
            entry: assembly
                .agents
                .first()
                .cloned()
                .expect("solved answers an agent"),
        })
        .expect("the assembled agent publishes");
    let mut graph = assembly.graph.clone().expect("solved answers a graph");
    graph.synthesis_evidence = Some(ComponentDependency {
        component_id: record.record_id.clone(),
        kind: AgentComponentKind::DecisionRecord,
        definition_digest: record.record_digest.clone(),
    });
    AgentGraphPublishRequest {
        context: mutation_context(
            store,
            TENANT,
            key,
            nonce.wrapping_add(1),
            0,
            "agent-graph:publish",
        ),
        graph,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R113.2.5
#[test]
fn shared_commit_writes_record_and_evidence_in_one_transaction() {
    use super::shared_commit::FAIL_AFTER_RECORD_ROWS;
    let (_dir, store) = open_agent_store();
    seed_library(&store);
    let assembly = decided(&store);
    let record = assembly.record.clone();
    let digest = record.inputs.catalog_digest.clone();
    let evidence = evidence_request(&store, &assembly, "shared-graph", 60);
    let graph_id = evidence.graph.graph_id.clone();

    // A failure injected after the first row leaves neither the record nor the evidence.
    FAIL_AFTER_RECORD_ROWS.with(|flag| flag.set(true));
    let error = store
        .commit_topology_decision(
            commit_context(&store, "shared-1", 62),
            &record,
            &digest,
            evidence.clone(),
            "txn-mine",
        )
        .unwrap_err();
    FAIL_AFTER_RECORD_ROWS.with(|flag| flag.set(false));
    assert!(error.contains("injected failure"), "{error}");
    assert!(store
        .current_component(TENANT, &record.record_id)
        .expect("reads")
        .is_none());
    assert!(store
        .current_graph(TENANT, &graph_id)
        .expect("reads")
        .is_none());
    assert!(store
        .decision_record_content(TENANT, &record.record_id)
        .is_err());

    // The same commit then lands both together.
    let parts = store
        .commit_topology_decision(
            commit_context(&store, "shared-2", 64),
            &record,
            &digest,
            evidence.clone(),
            "txn-mine",
        )
        .expect("commits");
    assert!(!parts.record.replayed);
    assert_eq!(parts.evidence_graph_id, graph_id);
    assert_eq!(parts.evidence_revision, 1);
    let graph = store
        .current_graph(TENANT, &graph_id)
        .expect("reads")
        .expect("the evidence graph is published");
    assert_eq!(graph.entry_revision, 1);
    let pin = graph.synthesis_evidence.expect("the graph pins the record");
    assert_eq!(pin.component_id, record.record_id);
    let content = store
        .decision_record_content(TENANT, &record.record_id)
        .expect("the record body is served");
    let served: DecisionRecord = serde_json::from_slice(&content.body).expect("decodes");
    assert_eq!(served, record);

    // A replay is a no-op: same heads, nothing written twice.
    let again = store
        .commit_topology_decision(
            commit_context(&store, "shared-3", 66),
            &record,
            &digest,
            evidence,
            "txn-mine",
        )
        .expect("a repeat is a replay");
    assert!(again.record.replayed);
    assert_eq!(again.evidence_revision, 1);
    assert_eq!(
        store
            .graph_revisions(TENANT, &graph_id)
            .expect("reads")
            .len(),
        1
    );

    // An evidence graph that pins another record is refused before any write.
    let (_dir_b, other) = open_agent_store();
    seed_library(&other);
    let assembly_b = decided(&other);
    let mut wrong = evidence_request(&other, &assembly_b, "shared-wrong", 70);
    if let Some(pin) = wrong.graph.synthesis_evidence.as_mut() {
        pin.definition_digest = format!("sha256:{}", "c".repeat(64));
    }
    let error = other
        .commit_topology_decision(
            commit_context(&other, "shared-4", 72),
            &assembly_b.record,
            &assembly_b.record.inputs.catalog_digest,
            wrong,
            "txn-mine",
        )
        .unwrap_err();
    assert!(error.starts_with("DERIVATION_REJECTED"), "{error}");
    assert!(other
        .current_component(TENANT, &assembly_b.record.record_id)
        .expect("reads")
        .is_none());
}

// spec: MEDULLA-DECISION-ENGINE-R113.2.5
#[test]
fn shared_commit_is_blocked_by_an_admission_fence() {
    use super::super::agent_library::admission::{
        AdmissionIntent, ComponentHead, ADMISSION_WRITE_BLOCKED,
    };
    let (_dir, store) = open_agent_store();
    seed_library(&store);
    let assembly = decided(&store);
    let record = assembly.record.clone();
    let digest = record.inputs.catalog_digest.clone();
    let evidence = evidence_request(&store, &assembly, "fenced-graph", 80);
    let mut heads: Vec<ComponentHead> = record
        .inputs
        .candidates
        .iter()
        .map(|candidate| ComponentHead {
            component_id: candidate.component_id.clone(),
            entry_revision: candidate.entry_revision,
            definition_digest: candidate.definition_digest.clone(),
        })
        .collect();
    heads.sort_by(|left, right| left.component_id.cmp(&right.component_id));
    assert!(!heads.is_empty());
    store
        .reserve_admission(
            TENANT,
            &AdmissionIntent {
                txn_id: "txn-other".into(),
                profile_revision: "policy-v1".into(),
                component_heads: heads,
                visibility_digest: format!("sha256:{}", "a".repeat(64)),
                write_key: "write-other".into(),
            },
        )
        .expect("another transaction reserves the candidates");

    let error = store
        .commit_topology_decision(
            commit_context(&store, "fenced-1", 82),
            &record,
            &digest,
            evidence.clone(),
            "txn-mine",
        )
        .unwrap_err();
    assert!(error.starts_with(ADMISSION_WRITE_BLOCKED), "{error}");
    assert!(store
        .current_component(TENANT, &record.record_id)
        .expect("reads")
        .is_none());
    assert!(store
        .current_graph(TENANT, &evidence.graph.graph_id)
        .expect("reads")
        .is_none());

    // The holder of the fence is not blocked by it.
    store
        .commit_topology_decision(
            commit_context(&store, "fenced-2", 84),
            &record,
            &digest,
            evidence,
            "txn-other",
        )
        .expect("the fence holder commits");
}
