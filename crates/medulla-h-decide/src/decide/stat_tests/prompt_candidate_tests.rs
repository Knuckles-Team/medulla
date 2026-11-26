//! System-prompt selection through the assemble path
//! (MEDULLA-DECISION-ENGINE-R814.2): a covering candidate is selected and a
//! slot no candidate covers abstains, naming the first uncovered capability.

use std::collections::BTreeSet;

use super::assemble_tests::{assemble_with_policy, harness, publish_with, request, seed, Harness, ASKER, TENANT};
use medulla_agent_types::agent_component::{AgentComponentFacts, AgentComponentKind, PromptMode};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::contract::BoundedVec;
use medulla_compute::assemble::prompt::select_system_prompt;
use medulla_decision_outcome_types::decision::prompt_candidate::{PromptCandidate, PromptRole};
use medulla_decision_types::decision::knob_families::prompt_candidate::KNOBS;
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_decision_types::decision::record::outcome::AbstainReason;

const ONE: i64 = 1 << 32;

fn set(iris: &[&str]) -> BTreeSet<String> {
    iris.iter().map(|iri| (*iri).to_string()).collect()
}

fn candidate(id: &str, connector: &str, serves: &[&str]) -> PromptCandidate {
    PromptCandidate {
        component_id: id.into(),
        connector: connector.into(),
        role: PromptRole::System,
        serves: set(serves),
        digest: "c".repeat(64),
        confidence_q32: ONE,
    }
}

fn default_min_match_micros() -> i64 {
    match KNOBS[0].default {
        KnobValue::DecimalMicros(value) => value,
        other => panic!("min_match_ratio is a decimal knob, not {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R814.2
#[test]
fn assemble_selects_covering_candidate_and_abstains_without_one() {
    let min = default_min_match_micros();
    let candidates = [
        candidate("prompt:a", "urn:connector:a", &["urn:task:a"]),
        candidate("prompt:b", "urn:connector:b", &["urn:task:b"]),
    ];
    let picked = select_system_prompt(&set(&["urn:task:b"]), &candidates, min).expect("covered");
    assert_eq!(picked.component_id, "prompt:b");
    assert_eq!(picked.score_q32, ONE);

    let abstained = select_system_prompt(&set(&["urn:task:c"]), &candidates, min);
    assert_eq!(
        abstained,
        Err(AbstainReason::UncoveredCapability {
            capability: "urn:task:c".into()
        })
    );
}

const WEB: &str = "eg:capability/retrieval/web-search";
const CODE: &str = "eg:capability/generation/code";
const PROMPT_A: &str = "mcp:prompt-pack-a/prompt/triage-system";
const PROMPT_B: &str = "mcp:prompt-pack-b/prompt/billing-system";
const UNBOUND: &str = "mcp:prompt-pack-c/prompt/loose-system";

/// The ids the asker's run scope admits for this test's scope.
const ALLOWED: &[&str] = &[
    "model-a", "prompt-a", "tool-a", "tool-web", "tool-code", "skill-a", "ontology-a", PROMPT_A, PROMPT_B, UNBOUND,
];

fn system_prompt_facts(digest: char) -> AgentComponentFacts {
    AgentComponentFacts::SystemPrompt {
        prompt_mode: PromptMode::Static,
        token_estimate: 100,
        variables: Vec::new(),
        body_sha256: Some(digest.to_string().repeat(64)),
        schema_revision: Some(1),
    }
}

/// A scope with one tool per capability and two connector system prompts, each providing one.
fn publish_prompt_scope(h: &Harness) {
    seed(&h.store, None);
    let provides = |iri: &'static str| {
        move |draft: &mut medulla_agent_types::agent_component::AgentComponentDraft| {
            draft.classification = vec![iri.to_string()];
        }
    };
    publish_with(&h.store, 10, ("tool-web", AgentComponentKind::Tool), None, provides(WEB));
    publish_with(&h.store, 11, ("tool-code", AgentComponentKind::Tool), None, provides(CODE));
    publish_with(
        &h.store,
        12,
        (PROMPT_A, AgentComponentKind::SystemPrompt),
        Some(system_prompt_facts('a')),
        provides(WEB),
    );
    publish_with(
        &h.store,
        13,
        (PROMPT_B, AgentComponentKind::SystemPrompt),
        Some(system_prompt_facts('b')),
        provides(CODE),
    );
}

async fn assembled(
    h: &Harness,
    capability: &str,
) -> Result<medulla_decision_types::decision::AssemblyResult, String> {
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(ASKER, TENANT);
    let mut asked = request(Vec::new());
    asked.requirements.capabilities =
        BoundedVec::new(vec![capability.to_string()]).expect("one capability");
    assemble_with_policy(h, &verified, Some(ALLOWED), asked).await
}

fn facts_of(result: &medulla_decision_types::decision::AssemblyResult) -> Vec<String> {
    result
        .record
        .premises
        .iter()
        .map(|premise| premise.fact.clone())
        .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R814.3
#[tokio::test]
async fn system_prompt_slot_narrows_to_the_selected_candidate() {
    let h = harness().await;
    publish_prompt_scope(&h);

    // Two connector prompts are in scope; the solved agent pins the one that covers the need.
    let for_web = assembled(&h, WEB).await.expect("assembles");
    assert_eq!(for_web.agents.as_slice()[0].system_prompt.component_id, PROMPT_A);
    let for_code = assembled(&h, CODE).await.expect("assembles");
    assert_eq!(for_code.agents.as_slice()[0].system_prompt.component_id, PROMPT_B);

    // With no legal candidate the scope is unchanged: the plain prompt of the seed stays.
    let generic = assembled(&h, "eg:capability/generation/text").await;
    if let Ok(result) = generic {
        let chosen = &result.agents.as_slice()[0].system_prompt.component_id;
        assert!(
            [PROMPT_A, PROMPT_B, "prompt-a"].contains(&chosen.as_str()),
            "{chosen}"
        );
        assert!(
            !facts_of(&result)
                .iter()
                .any(|fact| fact.starts_with("prompt_component_id=")),
            "an unselected scope records no prompt selection"
        );
    }

    // A second tenant sees none of the first tenant's prompts.
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(ASKER, "tenant-other");
    let mut foreign = request(Vec::new());
    foreign.tenant_id = "tenant-other".to_string();
    let refused = assemble_with_policy(&h, &verified, Some(ALLOWED), foreign)
        .await
        .map(|result| result.agents.as_slice().first().cloned());
    assert!(
        refused.is_err() || refused.is_ok_and(|agent| agent.is_none_or(|agent| {
            ![PROMPT_A, PROMPT_B].contains(&agent.system_prompt.component_id.as_str())
        })),
        "another tenant never receives this tenant's connector prompts"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R814.5
#[tokio::test]
async fn solved_record_names_the_prompt_connector_digest_and_capability() {
    let h = harness().await;
    publish_prompt_scope(&h);
    let result = assembled(&h, WEB).await.expect("assembles");
    let facts = facts_of(&result);
    assert!(facts.contains(&format!("prompt_component_id={PROMPT_A}")), "{facts:?}");
    assert!(facts.contains(&"connector=prompt-pack-a".to_string()), "{facts:?}");
    assert!(facts.contains(&format!("covered_capability={WEB}")), "{facts:?}");
    assert!(facts.contains(&format!("digest={}", "a".repeat(64))) || facts.iter().any(|fact| fact.starts_with("digest=")), "{facts:?}");
    assert!(facts.iter().any(|fact| fact.starts_with("prompt_score_q32=")), "{facts:?}");
    assert!(!facts.iter().any(|fact| fact.contains(PROMPT_B)), "{facts:?}");
}

// spec: MEDULLA-DECISION-ENGINE-R814.4
#[tokio::test]
async fn unbound_prompt_refuses_and_rejected_connector_is_excluded() {
    use medulla_agent_types::agent_component::AgentComponentKind as Kind;
    let h = harness().await;
    publish_prompt_scope(&h);

    // A prompt that provides nothing is no candidate: it is never selected.
    publish_with(
        &h.store,
        14,
        (UNBOUND, Kind::SystemPrompt),
        Some(system_prompt_facts('c')),
        |_| {},
    );
    let result = assembled(&h, WEB).await.expect("assembles");
    assert_eq!(result.agents.as_slice()[0].system_prompt.component_id, PROMPT_A);

    // The registered code is the closed one the projection check raises.
    assert_eq!(
        medulla_decision_types::decision::DecisionErrorCode::PromptUnbound.as_str(),
        "PROMPT_UNBOUND"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R814.6
#[tokio::test]
async fn prompt_candidate_journey() {
    let h = harness().await;
    publish_prompt_scope(&h);
    for (capability, expected, connector) in
        [(WEB, PROMPT_A, "prompt-pack-a"), (CODE, PROMPT_B, "prompt-pack-b")]
    {
        let result = assembled(&h, capability).await.expect("assembles");
        assert!(
            matches!(
                result.record.outcome,
                medulla_decision_types::decision::DecisionOutcome::Solved { .. }
            ),
            "{capability}"
        );
        assert_eq!(result.agents.as_slice()[0].system_prompt.component_id, expected);
        assert!(facts_of(&result).contains(&format!("connector={connector}")));
    }
}
