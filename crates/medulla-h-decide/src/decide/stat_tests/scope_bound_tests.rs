//! MEDULLA-IDENTITY-R008.1: tool-set admission is attached to `assemble_for`
//! exactly once, after the candidate read and before the candidate facts.

// spec: MEDULLA-IDENTITY-R008.1
#[test]
fn attach_point_is_reached() {
    let source = include_str!("../assemble.rs");
    let body_start = source
        .find("async fn assemble_for(")
        .expect("assemble_for is defined");
    let body_end = source[body_start..]
        .find("\n}\n")
        .map(|offset| body_start + offset)
        .expect("assemble_for body ends");
    let body = &source[body_start..body_end];
    assert_eq!(
        body.matches("admit_tool_set(").count(),
        1,
        "enforce_scope runs once per assembly"
    );
    let candidates = body
        .find("store.assembly_candidates(")
        .expect("candidate read");
    let admit = body.find("admit_tool_set(").expect("admission call");
    let facts = body.find("candidate_facts(").expect("candidate facts");
    assert!(candidates < admit && admit < facts);
    let admit_fn = source
        .find("async fn admit_tool_set(")
        .expect("admit_tool_set is defined");
    assert!(source[admit_fn..].contains("medulla_isolation::isolation::enforce_scope("));

    // With no stored policy node, the plan's tool set is outside the empty bound.
    let core = medulla_graph_core::graph::GraphCore::new();
    let refusal =
        medulla_isolation::isolation::enforce_scope(&core, "worker", &["tool:a".to_string()])
            .expect_err("empty bound refuses every tool");
    assert_eq!(refusal.code(), "SCOPE_EXCEEDS_POLICY");
}

use std::sync::Arc;

use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_persistence::redb_backend::RedbBackend;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::contract::BoundedVec;

use super::assemble_tests::{assemble_with_policy, harness, publish, request, ASKER, TENANT};

/// The request of the journey: tools only, nothing required.
fn tools_only() -> medulla_decision_types::decision::request::AssemblyRequest {
    let mut request = request(Vec::new());
    request.candidates.kinds = BoundedVec::new(vec![AgentComponentKind::Tool]).unwrap();
    request
}

// spec: MEDULLA-IDENTITY-R008.6
#[tokio::test]
async fn plan_scope_is_bounded_by_policy() {
    use sha2::Digest;
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(ASKER, TENANT);
    let policy_iri = format!(
        "urn:policy:run-scope:principal:sha256:{}",
        hex::encode(sha2::Sha256::digest(ASKER.as_bytes()))
    );

    // (1) A policy that names only `tool-a` refuses a candidate set that holds `tool-b`.
    let h = harness().await;
    h.state.write().await.persistence = Some(Arc::new(
        RedbBackend::open_with_shards(h._dir.path().to_string_lossy().into_owned(), 16, 1).unwrap(),
    ));
    publish(&h.store, 1, ("tool-a", AgentComponentKind::Tool), None);
    publish(&h.store, 2, ("tool-b", AgentComponentKind::Tool), None);
    let refusal = assemble_with_policy(&h, &verified, Some(&["tool-a"]), tools_only())
        .await
        .expect_err("a tool outside the policy is refused");
    assert!(refusal.starts_with("SCOPE_EXCEEDS_POLICY"), "{refusal}");
    assert!(
        refusal.contains("tool-b") && refusal.contains(&policy_iri),
        "{refusal}"
    );

    #[cfg(feature = "security")]
    {
        // Exactly one operation audit event is stored, naming the tool and the policy.
        let backend = h
            .state
            .read()
            .await
            .persistence
            .clone()
            .expect("persistence");
        let redb = {
            use medulla_persistence::AsRedb;
            backend.as_redb().expect("a redb backend")
        };
        let mut events = Vec::new();
        for seq in 0..4 {
            if let Ok(proof) = redb.audit_read_event("default", TENANT, seq).await {
                events.push((seq, proof));
            }
        }
        assert_eq!(events.len(), 1, "one audit event: {events:?}");
        let line = &events[0].1.event_line;
        assert!(line.contains("|op=AgentAssemble|"), "{line}");
        assert!(line.contains("|subject_iri=tool-b"), "{line}");
        assert!(
            line.contains(&format!("|policy_iri={policy_iri}")),
            "{line}"
        );
    }

    // (2) A fresh harness with only `tool-a` published passes admission.
    let admitted = harness().await;
    publish(
        &admitted.store,
        1,
        ("tool-a", AgentComponentKind::Tool),
        None,
    );
    let result = assemble_with_policy(&admitted, &verified, Some(&["tool-a"]), tools_only()).await;
    assert!(
        !result
            .as_ref()
            .err()
            .is_some_and(|error| error.starts_with("SCOPE_EXCEEDS_POLICY")),
        "admission passed: {result:?}"
    );

    // (3) Another tenant with no policy node in its scope is refused by the empty bound.
    let isolated = harness().await;
    publish(
        &isolated.store,
        1,
        ("tool-a", AgentComponentKind::Tool),
        None,
    );
    let other = VerifiedRequestContext::verified_for_test_in_tenant(ASKER, "tenant-other");
    let refusal = assemble_with_policy(&isolated, &other, None, tools_only())
        .await
        .expect_err("an empty bound refuses every tool");
    assert!(refusal.starts_with("SCOPE_EXCEEDS_POLICY"), "{refusal}");
}
