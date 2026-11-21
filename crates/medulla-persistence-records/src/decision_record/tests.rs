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
    tenant: &str,
    nonce: u8,
    draft: AgentComponentDraft,
) -> AgentComponentEntry {
    let key = format!("seed:{}", draft.component_id);
    store
        .publish_component(AgentComponentPublishRequest {
            context: mutation_context(store, tenant, &key, nonce, 0, "agent-component:publish"),
            evaluation_receipt_digest: None,
            component: draft,
            prompt_blueprint: None,
        }, None)
        .expect("publishes")
        .result
        .component
}

fn draft(
    tenant: &str,
    id: &str,
    kind: AgentComponentKind,
    facts: AgentComponentFacts,
    class: &str,
) -> AgentComponentDraft {
    AgentComponentDraft {
        kind,
        facts,
        classification: vec![class.to_string()],
        ..test_component_draft(tenant, id)
    }
}

pub fn seed_library(store: &AgentLibraryStore, tenant: &str) {
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
            tenant,
            "model-a",
            AgentComponentKind::ModelProfile,
            model,
            "eg:capability/generation/text",
        ),
        draft(
            tenant,
            "prompt-a",
            AgentComponentKind::SystemPrompt,
            prompt,
            "eg:capability/reasoning/plan",
        ),
        draft(
            tenant,
            "skill-a",
            AgentComponentKind::Skill,
            AgentComponentFacts::Opaque,
            "eg:capability/reasoning/plan",
        ),
        draft(
            tenant,
            "ontology-a",
            AgentComponentKind::Ontology,
            AgentComponentFacts::Opaque,
            "eg:capability/reasoning/plan",
        ),
        draft(
            tenant,
            "tool-web",
            AgentComponentKind::Tool,
            test_tool_facts(ToolEffect::Read),
            "eg:capability/retrieval/web-search",
        ),
    ];
    for (nonce, draft) in drafts.into_iter().enumerate() {
        publish(store, tenant, 100 + nonce as u8, draft);
    }
}

fn request(tenant: &str) -> AssemblyRequest {
    AssemblyRequest {
        tenant_id: tenant.to_string(),
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
        primitive_edges: BoundedVec::default(),
        replacements: BoundedVec::default(),
        assistant_profile_ref: None,
        coding_agent: None,
        evidence_streams: Default::default(),
        patterns: Default::default(),
        prompts: Default::default(),
    }
}

pub fn decided(store: &AgentLibraryStore, tenant: &str) -> Assembly {
    decided_at(store, tenant, 20)
}

/// The same solved assembly, decided at `created_at_ms`: a distinct record id per time.
pub fn decided_at(store: &AgentLibraryStore, tenant: &str, created_at_ms: u64) -> Assembly {
    decided_under(store, tenant, created_at_ms, DecisionPolicy::engine_default())
}

/// The same solved assembly, decided at `created_at_ms` under `policy`.
pub fn decided_under(
    store: &AgentLibraryStore,
    tenant: &str,
    created_at_ms: u64,
    policy: DecisionPolicy,
) -> Assembly {
    let entries = store
        .assembly_candidates(tenant, &request(tenant).candidates)
        .expect("reads");
    let candidates = candidate_facts(&entries).expect("facts");
    let inputs = inputs(
        request(tenant),
        candidates,
        Vec::new(),
        policy,
    )
    .expect("inputs");
    let identity = RecordIdentity {
        tenant_id: tenant.to_string(),
        caller_principal: "caller-a".to_string(),
        created_at_ms,
    };
    let assembly = assemble(inputs, identity).expect("assembles");
    assert!(assembly.record.is_solved(), "{:?}", assembly.record.outcome);
    replay_check(&assembly.record).expect("replays");
    assembly
}

pub fn commit_context(
    store: &AgentLibraryStore,
    tenant: &str,
    key: &str,
    nonce: u8,
) -> AgentLibraryMutationContext {
    mutation_context(store, tenant, key, nonce, 0, "decision:commit")
}

// spec: MEDULLA-DECISION-ENGINE-R019
// spec: MEDULLA-DECISION-ENGINE-R051
#[test]
fn a_record_commits_once_serves_its_body_and_replays_idempotently() {
    let (_dir, store) = open_agent_store();
    seed_library(&store, TENANT);
    let record = decided(&store, TENANT).record;
    let digest = record.inputs.catalog_digest.clone();
    let first = store
        .commit_decision_record(commit_context(&store, TENANT, "commit-1", 1), &record, &digest, None)
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
        .commit_decision_record(commit_context(&store, TENANT, "commit-2", 2), &record, &digest, None)
        .expect("a repeat commit is a replay");
    assert!(again.replayed);
    assert_eq!(again.result.component, first.result.component);
}

// spec: MEDULLA-DECISION-ENGINE-R019
#[test]
fn a_catalog_that_moved_since_the_decision_is_a_stale_refusal() {
    let (_dir, store) = open_agent_store();
    seed_library(&store, TENANT);
    let record = decided(&store, TENANT).record;
    publish(
        &store,
        TENANT,
        120,
        draft(
            TENANT,
            "tool-vector",
            AgentComponentKind::Tool,
            test_tool_facts(ToolEffect::Read),
            "eg:capability/retrieval/vector-search",
        ),
    );
    let error = store
        .commit_decision_record(
            commit_context(&store, TENANT, "commit-stale", 3),
            &record,
            &record.inputs.catalog_digest,
            None,
        )
        .expect_err("stale");
    assert!(error.starts_with("STALE_CATALOG: "), "{error}");
}

// spec: MEDULLA-DECISION-ENGINE-R019
// spec: MEDULLA-DECISION-ENGINE-R051
#[test]
fn a_fabricated_record_whose_facts_differ_from_the_published_revision_is_refused() {
    let (_dir, store) = open_agent_store();
    seed_library(&store, TENANT);
    let mut record = decided(&store, TENANT).record;
    let mut candidates = record.inputs.candidates.as_slice().to_vec();
    candidates[0].classification = BoundedVec::new(vec!["eg:capability".to_string()]).unwrap();
    record.inputs.candidates = BoundedVec::new(candidates).unwrap();
    let error = store
        .commit_decision_record(
            commit_context(&store, TENANT, "commit-forged", 4),
            &record,
            &record.inputs.catalog_digest,
            None,
        )
        .expect_err("forged");
    assert!(error.starts_with("CANDIDATE_FACTS_CHANGED: "), "{error}");
}

// spec: MEDULLA-DECISION-ENGINE-R051
#[test]
fn a_record_from_another_tenant_is_refused() {
    let (_dir, store) = open_agent_store();
    seed_library(&store, TENANT);
    let mut record = decided(&store, TENANT).record;
    record.tenant_id = "tenant-b".to_string();
    let error = store
        .commit_decision_record(
            commit_context(&store, TENANT, "commit-tenant", 5),
            &record,
            &record.inputs.catalog_digest,
            None,
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
    seed_library(&store, TENANT);
    let assembly = decided(&store, TENANT);
    let record = assembly.record.clone();
    let committed = store
        .commit_decision_record(
            commit_context(&store, TENANT, "commit-link", 6),
            &record,
            &record.inputs.catalog_digest,
            None,
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
        TENANT,
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
    seed_library(&store, TENANT);
    let mut policy = DecisionPolicy::engine_default();
    policy.node_budget = 5_000;
    let digest = medulla_decision_types::decision::digest::policy_digest(&policy);
    let published = publish(&store, TENANT, 130, policy_draft(&policy, digest));
    let pin = ComponentDependency {
        component_id: published.component_id.clone(),
        kind: AgentComponentKind::DecisionPolicy,
        definition_digest: published.definition_digest.clone(),
    };
    let read = store
        .pinned_decision_policy(TENANT, &pin)
        .expect("the pinned body verifies");
    assert_eq!(read, policy);
    let mut asked = request(TENANT);
    asked.policy = DecisionPolicyRef::Pinned { component: pin };
    let entries = store
        .assembly_candidates(TENANT, &asked.candidates)
        .expect("reads");
    let candidates = candidate_facts(&entries).expect("facts");
    let inputs = inputs(asked, candidates, Vec::new(), read).expect("inputs");
    let identity = RecordIdentity {
        tenant_id: tenant.to_string(),
        caller_principal: "caller-a".to_string(),
        created_at_ms: 21,
    };
    let record = assemble(inputs, identity).expect("assembles").record;
    assert_eq!(record.inputs.solver.node_budget, 5_000);
    store
        .commit_decision_record(
            commit_context(&store, TENANT, "commit-pinned", 9),
            &record,
            &record.inputs.catalog_digest,
            None,
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
            prompt_blueprint: None,
        }, None)
        .expect_err("refused");
    assert!(error.starts_with("POLICY_BODY_UNAVAILABLE: "), "{error}");
}

/// The evidence graph of a solved assembly, pinned to the committed record, with its agent
/// published so the graph's references resolve.
pub fn evidence_request(
    store: &AgentLibraryStore,
    tenant: &str,
    assembly: &Assembly,
    key: &str,
    nonce: u8,
) -> AgentGraphPublishRequest {
    let record = &assembly.record;
    let agent = assembly
        .agents
        .first()
        .cloned()
        .expect("solved answers an agent");
    // An agent a fixture already published (the same definition) is reused, not re-published.
    let published = store
        .current(tenant, &agent.agent_id)
        .expect("reads the agent head")
        .is_some_and(|entry| entry.as_draft() == agent);
    if !published {
        store
            .publish(AgentLibraryPublishRequest {
                context: mutation_context(
                    store,
                    tenant,
                    "agent-shared",
                    nonce,
                    0,
                    "agent-library:publish",
                ),
                entry: agent,
            })
            .expect("the assembled agent publishes");
    }
    let mut graph = assembly.graph.clone().expect("solved answers a graph");
    graph.synthesis_evidence = Some(ComponentDependency {
        component_id: record.record_id.clone(),
        kind: AgentComponentKind::DecisionRecord,
        definition_digest: record.record_digest.clone(),
    });
    AgentGraphPublishRequest {
        context: mutation_context(
            store,
            tenant,
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
    seed_library(&store, TENANT);
    let assembly = decided(&store, TENANT);
    let record = assembly.record.clone();
    let digest = record.inputs.catalog_digest.clone();
    let evidence = evidence_request(&store, TENANT, &assembly, "shared-graph", 60);
    let graph_id = evidence.graph.graph_id.clone();

    // A failure injected after the first row leaves neither the record nor the evidence.
    FAIL_AFTER_RECORD_ROWS.with(|flag| flag.set(true));
    let error = store
        .commit_topology_decision(
            commit_context(&store, TENANT, "shared-1", 62),
            &record,
            &digest,
            evidence.clone(),
            "txn-mine",
            None,
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
            commit_context(&store, TENANT, "shared-2", 64),
            &record,
            &digest,
            evidence.clone(),
            "txn-mine",
            None,
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
            commit_context(&store, TENANT, "shared-3", 66),
            &record,
            &digest,
            evidence,
            "txn-mine",
            None,
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
    seed_library(&other, TENANT);
    let assembly_b = decided(&other, TENANT);
    let mut wrong = evidence_request(&other, TENANT, &assembly_b, "shared-wrong", 70);
    if let Some(pin) = wrong.graph.synthesis_evidence.as_mut() {
        pin.definition_digest = format!("sha256:{}", "c".repeat(64));
    }
    let error = other
        .commit_topology_decision(
            commit_context(&other, TENANT, "shared-4", 72),
            &assembly_b.record,
            &assembly_b.record.inputs.catalog_digest,
            wrong,
            "txn-mine",
            None,
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
    seed_library(&store, TENANT);
    let assembly = decided(&store, TENANT);
    let record = assembly.record.clone();
    let digest = record.inputs.catalog_digest.clone();
    let evidence = evidence_request(&store, TENANT, &assembly, "fenced-graph", 80);
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
            commit_context(&store, TENANT, "fenced-1", 82),
            &record,
            &digest,
            evidence.clone(),
            "txn-mine",
            None,
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
            commit_context(&store, TENANT, "fenced-2", 84),
            &record,
            &digest,
            evidence,
            "txn-other",
            None,
        )
        .expect("the fence holder commits");
}

use super::retry_allowance::{
    denial_allowance_transition, DenialAllowanceEvent, DenialAllowanceRequest,
    DenialAllowanceState, RetryAllowanceError,
};
fn allowance_context(
    store: &AgentLibraryStore,
    tenant: &str,
    key: &str,
    nonce: u8,
    action: &str,
) -> medulla_agent_types::agent_library::AgentLibraryMutationContext {
    mutation_context(
        store,
        tenant,
        key,
        nonce,
        0,
        &format!("decision-retry-allowance:{action}"),
    )
}

// spec: MEDULLA-DECISION-ENGINE-R113.3.1.1
#[test]
fn retry_allowance_table_issues_once_and_consumes_once() {
    let (dir, store) = open_agent_store();
    let denied = "decision:denied-a";
    assert_eq!(store.denial_allowance_state(TENANT, denied), Ok(None));

    // Issue writes `Issued` once and is idempotent for the same key.
    let issued = store
        .issue_denial_allowance(
            &allowance_context(&store, TENANT, "issue-1", 1, "issue"),
            denied,
        )
        .expect("issues");
    assert_eq!(issued.state, DenialAllowanceState::Issued);
    let again = store
        .issue_denial_allowance(
            &allowance_context(&store, TENANT, "issue-2", 2, "issue"),
            denied,
        )
        .expect("a second issue is idempotent");
    assert_eq!(again.state, DenialAllowanceState::Issued);
    assert_eq!(
        store.denial_allowance_state(TENANT, denied),
        Ok(Some(DenialAllowanceState::Issued))
    );

    // Another tenant finds nothing, and cannot take this tenant's row.
    assert_eq!(store.denial_allowance_state("tenant-b", denied), Ok(None));
    assert_eq!(
        store.take_denial_allowance(
            &allowance_context(&store, "tenant-b", "take-other", 3, "take"),
            denied
        ),
        Err(RetryAllowanceError::NotIssued)
    );
    // A key that was never issued is the same not-found refusal.
    assert_eq!(
        store.take_denial_allowance(
            &allowance_context(&store, TENANT, "take-unknown", 4, "take"),
            "decision:never-denied"
        ),
        Err(RetryAllowanceError::NotIssued)
    );

    // The first take consumes; the second is refused.
    store
        .take_denial_allowance(
            &allowance_context(&store, TENANT, "take-1", 5, "take"),
            denied,
        )
        .expect("the first take consumes the allowance");
    assert_eq!(
        store.take_denial_allowance(
            &allowance_context(&store, TENANT, "take-2", 6, "take"),
            denied
        ),
        Err(RetryAllowanceError::Exhausted)
    );
    // A consumed allowance is never issued again.
    assert_eq!(
        store
            .issue_denial_allowance(
                &allowance_context(&store, TENANT, "issue-3", 7, "issue"),
                denied,
            )
            .unwrap_err(),
        RetryAllowanceError::Exhausted
    );

    // A reopen keeps the tombstone: the allowance is never restored.
    drop(store);
    let store = AgentLibraryStore::open(dir.path().to_str().unwrap()).expect("reopens");
    assert_eq!(
        store.denial_allowance_state(TENANT, denied),
        Ok(Some(DenialAllowanceState::Consumed))
    );
    assert_eq!(
        store.take_denial_allowance(
            &allowance_context(&store, TENANT, "take-3", 8, "take"),
            denied
        ),
        Err(RetryAllowanceError::Exhausted)
    );
    assert_eq!(store.denial_allowance_state("tenant-b", denied), Ok(None));
}

// spec: MEDULLA-DECISION-ENGINE-R113.3.1.1
#[test]
fn the_allowance_machine_has_one_transition_function_over_every_state_and_event() {
    use DenialAllowanceRequest::{Issue, Take};
    use DenialAllowanceState::{Consumed, Issued};
    assert_eq!(denial_allowance_transition(None, Issue), Ok(Issued));
    assert_eq!(denial_allowance_transition(Some(Issued), Issue), Ok(Issued));
    assert_eq!(
        denial_allowance_transition(Some(Issued), Take),
        Ok(Consumed)
    );
    assert_eq!(
        denial_allowance_transition(None, Take),
        Err(RetryAllowanceError::NotIssued)
    );
    assert_eq!(
        denial_allowance_transition(Some(Consumed), Issue),
        Err(RetryAllowanceError::Exhausted)
    );
    assert_eq!(
        denial_allowance_transition(Some(Consumed), Take),
        Err(RetryAllowanceError::Exhausted)
    );
    // The machine edge itself is typed: the tombstone accepts no event.
    let refused = DenialAllowanceState::Consumed
        .apply(DenialAllowanceEvent::Take)
        .unwrap_err();
    assert_eq!(refused.machine, "DenialAllowanceMachine");
    assert_eq!(refused.from, "consumed");
    assert_eq!(refused.event, "Take");
    assert_eq!(
        DenialAllowanceState::Issued.apply(DenialAllowanceEvent::Take),
        Ok(DenialAllowanceState::Consumed)
    );
    // The typed refusals carry stable codes; a storage failure has none.
    assert_eq!(
        RetryAllowanceError::Exhausted.code(),
        Some("RETRY_ALLOWANCE_EXHAUSTED")
    );
    assert_eq!(
        RetryAllowanceError::NotIssued.code(),
        Some("RETRY_ALLOWANCE_NOT_ISSUED")
    );
    assert_eq!(RetryAllowanceError::Storage("x".into()).code(), None);
}

// spec: MEDULLA-DECISION-ENGINE-R113.3.2
#[test]
fn topology_denial_allowance_consumption_survives_restart() {
    let (dir, store) = open_agent_store();
    let denied = "decision:denied-restart";
    store
        .issue_denial_allowance(
            &allowance_context(&store, TENANT, "issue-r", 61, "issue"),
            denied,
        )
        .expect("the denial issues its allowance");

    // The first use consumes it and writes the tombstone.
    store
        .consume_denial_allowance(
            &allowance_context(&store, TENANT, "take-r1", 62, "take"),
            denied,
        )
        .expect("the first use is accepted");
    assert_eq!(
        store.denial_allowance_state(TENANT, denied),
        Ok(Some(DenialAllowanceState::Consumed))
    );

    // A restart never restores it: the refusal is the stable code.
    drop(store);
    let store = AgentLibraryStore::open(dir.path().to_str().unwrap()).expect("reopens");
    let error = store
        .consume_denial_allowance(
            &allowance_context(&store, TENANT, "take-r2", 63, "take"),
            denied,
        )
        .unwrap_err();
    assert_eq!(error, RetryAllowanceError::Exhausted);
    assert_eq!(error.code(), Some("RETRY_ALLOWANCE_EXHAUSTED"));
    assert!(error.to_string().starts_with("RETRY_ALLOWANCE_EXHAUSTED"));

    // A different tenant finds no allowance under the same denied decision id.
    assert_eq!(store.denial_allowance_state("tenant-b", denied), Ok(None));
    assert_eq!(
        store.consume_denial_allowance(
            &allowance_context(&store, "tenant-b", "take-other", 64, "take"),
            denied
        ),
        Err(RetryAllowanceError::NotIssued)
    );
    assert_eq!(
        store.denial_allowance_state(TENANT, denied),
        Ok(Some(DenialAllowanceState::Consumed))
    );

    // The denial's allowance rides the coordinator's ABORT (same transaction): a crash between
    // the durable ABORT and the owners' resolution issues nothing yet, and the recovery that
    // applies the ABORT to the reservation applies the allowance in the same owner transaction.
    use super::super::agent_library::admission::{
        AdmissionEvent, AdmissionIntent, ADMISSION_NOT_RESERVED,
    };
    let intent = |txn: &str, denied: &str| AdmissionIntent {
        txn_id: txn.into(),
        profile_revision: "policy-v1".into(),
        component_heads: Vec::new(),
        visibility_digest: format!("sha256:{}", "a".repeat(64)),
        write_key: format!("decision-record:{denied}"),
    };
    let crashed = "decision:denied-crash";
    store
        .reserve_admission(TENANT, &intent("txn-crash", crashed))
        .expect("the denied commit reserves");
    store
        .stage_abort_allowance(TENANT, "txn-crash", crashed)
        .expect("the denial stages its allowance before the decision");
    assert_eq!(store.denial_allowance_state(TENANT, crashed), Ok(None));
    drop(store);
    let store = AgentLibraryStore::open(dir.path().to_str().unwrap()).expect("reopens");
    assert_eq!(
        store.denial_allowance_state(TENANT, crashed),
        Ok(None),
        "a crash before the resolution leaves nothing issued"
    );
    store
        .resolve_admission(TENANT, "txn-crash", AdmissionEvent::Abort)
        .expect("recovery applies the durable ABORT");
    assert_eq!(
        store.denial_allowance_state(TENANT, crashed),
        Ok(Some(DenialAllowanceState::Issued))
    );
    // The recovery repeats safely.
    store
        .resolve_admission(TENANT, "txn-crash", AdmissionEvent::Abort)
        .expect("a repeated resolution replays");
    assert_eq!(
        store.denial_allowance_state(TENANT, crashed),
        Ok(Some(DenialAllowanceState::Issued))
    );
    store
        .consume_denial_allowance(
            &allowance_context(&store, TENANT, "take-crash", 65, "take"),
            crashed,
        )
        .expect("the recovered allowance is usable once");
    store
        .resolve_admission(TENANT, "txn-crash", AdmissionEvent::Abort)
        .expect("a repeated resolution after use replays");
    assert_eq!(
        store.denial_allowance_state(TENANT, crashed),
        Ok(Some(DenialAllowanceState::Consumed)),
        "the tombstone is never reset"
    );

    // A COMMIT resolution discards the staged allowance, and an unknown transaction refuses.
    let committed = "decision:denied-committed";
    store
        .reserve_admission(TENANT, &intent("txn-commit", committed))
        .expect("reserves");
    store
        .stage_abort_allowance(TENANT, "txn-commit", committed)
        .expect("stages");
    store
        .resolve_admission(TENANT, "txn-commit", AdmissionEvent::Commit)
        .expect("commits");
    assert_eq!(store.denial_allowance_state(TENANT, committed), Ok(None));
    assert!(store
        .stage_abort_allowance(TENANT, "txn-commit", committed)
        .is_err());
    let unknown = store
        .stage_abort_allowance(TENANT, "txn-unknown", committed)
        .unwrap_err();
    assert!(unknown.starts_with(ADMISSION_NOT_RESERVED), "{unknown}");
}
