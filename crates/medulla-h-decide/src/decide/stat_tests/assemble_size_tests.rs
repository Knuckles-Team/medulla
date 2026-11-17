//! Served smallest adequate assembly (MEDULLA-DECISION-ENGINE-R014.100.1 and
//! R014.100.2): the size, adequacy and ledger facts a component declares as entry
//! attributes reach the assembly through the real handler. A component a recorded
//! evaluation found inadequate is eliminated, a component nobody evaluated is
//! eliminated when the request requires adequacy, and under the lexicographic
//! `ArtifactBytes` objective the smallest remaining component is chosen.

use medulla_agent_types::agent_component::{
    AgentComponentKind, AgentComponentPublishRequest, ComponentDependency,
};
use medulla_decision_types::decision::size::{
    ADEQUACY_EVALUATION_ATTRIBUTE, ADEQUATE_ATTRIBUTE, SIZE_BYTES_ATTRIBUTE, SIZE_LINES_ATTRIBUTE,
    SIZE_REVISION_ATTRIBUTE,
};
use medulla_decision_types::decision::{
    DecisionOutcome, DecisionPolicy, DecisionPolicyRef, ObjectiveLevelKind, ObjectiveOrder,
    UnknownCostRule,
};
use medulla_persistence_records::agent_component::test_component_draft;
use medulla_persistence_records::agent_fixtures::mutation_context;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_types::contract::BoundedVec;

use super::assemble_tests::{assemble, harness, publish_with, request, seed_with, TENANT};

/// Declare the size and, when `adequate` is known, the evaluation that judged it.
fn declare_size_and_adequacy(
    draft: &mut medulla_agent_types::agent_component::AgentComponentDraft,
    bytes: u64,
    adequate: Option<bool>,
) {
    let id = draft.component_id.clone();
    let attributes = &mut draft.attributes;
    attributes.insert(SIZE_BYTES_ATTRIBUTE.to_string(), bytes.to_string());
    attributes.insert(SIZE_LINES_ATTRIBUTE.to_string(), (bytes / 10).to_string());
    attributes.insert(SIZE_REVISION_ATTRIBUTE.to_string(), "3".to_string());
    if let Some(adequate) = adequate {
        attributes.insert(ADEQUATE_ATTRIBUTE.to_string(), adequate.to_string());
        attributes.insert(
            ADEQUACY_EVALUATION_ATTRIBUTE.to_string(),
            format!("eval-{id}"),
        );
    }
}

/// Publish a tool that declares its size and, when `adequate` is known, the
/// evaluation that judged it.
fn publish_sized_tool(
    store: &AgentLibraryStore,
    nonce: u8,
    id: &str,
    bytes: u64,
    adequate: Option<bool>,
) {
    publish_with(
        store,
        nonce,
        (id, AgentComponentKind::Tool),
        None,
        |draft| declare_size_and_adequacy(draft, bytes, adequate),
    );
}

/// Publish `policy` the way the assembly lane serves it and pin it.
fn publish_policy(
    store: &AgentLibraryStore,
    nonce: u8,
    id: &str,
    policy: &DecisionPolicy,
) -> ComponentDependency {
    let mut draft = test_component_draft(TENANT, id);
    draft.kind = AgentComponentKind::DecisionPolicy;
    draft.content_digest = medulla_decision_types::decision::digest::policy_digest(policy);
    draft.attributes.insert(
        medulla_decision_types::decision::policy::DECISION_POLICY_ATTRIBUTE.to_string(),
        serde_json::to_string(policy).unwrap(),
    );
    let committed = store
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
        .expect("the policy publishes");
    ComponentDependency {
        component_id: id.to_string(),
        kind: AgentComponentKind::DecisionPolicy,
        definition_digest: committed.result.component.definition_digest,
    }
}

/// The tools the solved assembly chose, by id.
fn chosen_tools(outcome: &DecisionOutcome) -> Vec<String> {
    let DecisionOutcome::Solved { slots, .. } = outcome else {
        panic!("expected a solved assembly, got {outcome:?}");
    };
    let mut tools: Vec<String> = slots
        .iter()
        .filter(|slot| slot.component.kind == AgentComponentKind::Tool)
        .map(|slot| slot.component.component_id.clone())
        .collect();
    tools.sort();
    tools
}

// spec: MEDULLA-DECISION-ENGINE-R014.100.1
// spec: MEDULLA-DECISION-ENGINE-R014.100.2
#[tokio::test]
async fn smallest_adequate_composition_is_served() {
    let h = harness().await;
    // `tool-a` declares no size and no adequacy; the others declare both. The
    // smallest artifact, `tool-bad`, was judged inadequate.
    seed_with(&h.store, None, |draft| {
        // Every base component is measured and adequate, so the size objective
        // and the adequacy requirement bear on the tools alone.
        if draft.component_id != "tool-a" {
            declare_size_and_adequacy(draft, 50, Some(true));
        }
    });
    publish_sized_tool(&h.store, 20, "tool-large", 900, Some(true));
    publish_sized_tool(&h.store, 21, "tool-small", 100, Some(true));
    publish_sized_tool(&h.store, 22, "tool-bad", 10, Some(false));

    let mut policy = DecisionPolicy::engine_default();
    policy.objective = ObjectiveOrder::Lexicographic {
        levels: BoundedVec::new(vec![
            ObjectiveLevelKind::Uncovered,
            ObjectiveLevelKind::Components,
            ObjectiveLevelKind::ArtifactBytes,
        ])
        .unwrap(),
    };
    policy.unknown_cost = UnknownCostRule::RankBelowKnown;
    let pinned = publish_policy(&h.store, 23, "policy-size", &policy);

    // The inadequate artifact is eliminated even though it is the smallest, and
    // among the rest the smallest known artifact wins over a larger one and over
    // a component whose size is unknown.
    let mut asked = request(Vec::new());
    asked.policy = DecisionPolicyRef::Pinned {
        component: pinned.clone(),
    };
    let result = assemble(&h, asked).await.expect("the library assembles");
    assert_eq!(chosen_tools(&result.record.outcome), ["tool-small"]);

    // Requiring adequacy also eliminates the component nobody evaluated, so the
    // same tool is chosen from a library that no longer holds `tool-a`.
    let mut strict = request(Vec::new());
    strict.policy = DecisionPolicyRef::Pinned { component: pinned };
    strict.requirements.constraints.require_adequacy = true;
    let result = assemble(&h, strict).await.expect("the library assembles");
    assert_eq!(chosen_tools(&result.record.outcome), ["tool-small"]);

    // With the default objective (no size level) the inadequate component is
    // still never chosen.
    let result = assemble(&h, request(Vec::new()))
        .await
        .expect("the library assembles");
    assert!(!chosen_tools(&result.record.outcome).contains(&"tool-bad".to_string()));
}
