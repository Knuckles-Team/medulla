//! Served `ContextFacts` proofs (MEDULLA-DECISION-ENGINE-R152.3 .. R152.6).

use super::context_facts::{select_facts, serve_context_facts};
use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_types::context_facts::{
    fact_id, ContextFactsRequest, ContextFactsResult, CONTEXT_FACTS_SCHEMA_VERSION,
};
use medulla_types::evidence_class::EvidenceClass;
use serde_json::json;

fn blob(value: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&value).unwrap()
}

fn request(ids: &[&str], budget: u64) -> ContextFactsRequest {
    ContextFactsRequest {
        schema_version: CONTEXT_FACTS_SCHEMA_VERSION,
        node_ids: ids.iter().map(|id| id.to_string()).collect(),
        token_budget: budget,
        max_facts_per_node: None,
    }
}

/// Two cited nodes and one cited edge.
fn seeded() -> GraphCore {
    let core = GraphCore::new();
    core.add_node(
        "tool-a".into(),
        blob(json!({
            "type": "Tool", "name": "Alpha", "version": 3,
            "source_key": "src-a", "evidence_class": "proof",
            "tags": ["ignored", "array"],
        })),
    );
    core.add_node(
        "skill-b".into(),
        blob(json!({
            "type": "Skill", "name": "Beta", "source_key": "src-b",
            "evidence_class": "observation",
        })),
    );
    core.add_typed_edge(
        "tool-a".into(),
        "skill-b".into(),
        "usesSkill".into(),
        blob(json!({"relationship": "usesSkill", "source_key": "src-edge"})),
    )
    .unwrap();
    core
}

fn result_of(response: &Response) -> ContextFactsResult {
    match response.result.as_ref() {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value.clone()).unwrap(),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("expected a bundle, got {other:?} / {:?}", response.error),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R152.1
// spec: MEDULLA-DECISION-ENGINE-R152.2
// spec: MEDULLA-DECISION-ENGINE-R152.6
#[test]
fn served_context_facts_returns_cited_facts_within_budget() {
    let core = seeded();
    let ids = ["tool-a", "skill-b"];

    let full = result_of(&serve_context_facts(1, &core, &request(&ids, 10_000)));
    // tool-a: name, type, version, one edge; skill-b: name, type.
    assert_eq!(full.facts.len(), 6);
    assert_eq!(full.omitted_count, 0);
    assert!(full
        .facts
        .iter()
        .all(|f| f.subject == "tool-a" || f.subject == "skill-b"));
    let first = &full.facts[0];
    assert_eq!(first.subject, "tool-a", "caller order is kept");

    let budget = full.token_estimate_total - 1;
    let tight = result_of(&serve_context_facts(2, &core, &request(&ids, budget)));
    assert!(tight.token_estimate_total <= budget);
    assert!(tight.omitted_count >= 1);
    assert_eq!(
        tight.facts.len() as u64 + tight.omitted_count,
        full.facts.len() as u64,
        "every fact is returned or counted as omitted"
    );
    // The same request answers the same bundle.
    assert_eq!(
        tight,
        result_of(&serve_context_facts(3, &core, &request(&ids, budget)))
    );
}

// spec: MEDULLA-DECISION-ENGINE-R152.3
#[cfg(feature = "security")]
#[test]
fn context_facts_respects_visibility() {
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use medulla_server_api::carrier::GraphReadAuthority;
    use std::sync::Arc;

    let raw = Arc::new(GraphCore::new());
    raw.add_node(
        "bob-private".into(),
        blob(json!({
            "name": "Secret", "source_key": "src-x",
            "_owner": "bob", "_visibility": "private",
        })),
    );
    raw.add_node(
        "shared".into(),
        blob(json!({
            "name": "Shared", "source_key": "src-s",
            "_owner": "bob", "_visibility": "public",
        })),
    );
    let isolation = medulla_server_auth::access::two_agent_isolation();
    let alice = GraphReadAuthority::from_verified(
        &VerifiedRequestContext::verified_for_test("alice"),
        &isolation,
    )
    .unwrap();
    let bob = GraphReadAuthority::from_verified(
        &VerifiedRequestContext::verified_for_test("bob"),
        &isolation,
    )
    .unwrap();
    let ask = request(&["bob-private", "shared"], 1_000);

    let seen_by_alice = select_facts(&alice.project_core(&raw), &ask).unwrap();
    assert!(
        seen_by_alice.facts.iter().all(|f| f.subject == "shared"),
        "a hidden node yields no fact"
    );
    assert!(!seen_by_alice.facts.is_empty());

    let seen_by_bob = select_facts(&bob.project_core(&raw), &ask).unwrap();
    assert!(seen_by_bob.facts.iter().any(|f| f.subject == "bob-private"));

    // Cross-tenant negative: another graph holds no such node.
    let other_tenant = GraphCore::new();
    assert!(select_facts(&other_tenant, &ask).unwrap().facts.is_empty());
}

// spec: MEDULLA-DECISION-ENGINE-R152.4
#[test]
fn context_facts_refusals() {
    let core = seeded();
    let refuse = |ask: ContextFactsRequest, code: &str| {
        let response = serve_context_facts(9, &core, &ask);
        assert!(response.result.is_none(), "{code} leaves an empty result");
        let detail = response.error.clone().unwrap_or_default();
        assert!(detail.starts_with(code), "{code}: {detail}");
    };
    refuse(request(&["tool-a"], 0), "CONTEXT_FACTS_BUDGET_ZERO");
    let many: Vec<String> = (0..513).map(|i| format!("n{i}")).collect();
    refuse(
        ContextFactsRequest {
            node_ids: many,
            ..request(&[], 100)
        },
        "CONTEXT_FACTS_TOO_MANY_NODES",
    );
    core.add_node("bare".into(), blob(json!({"name": "NoSource"})));
    refuse(request(&["bare"], 100), "FACT_PROVENANCE_MISSING");
}

// spec: MEDULLA-DECISION-ENGINE-R152.5
#[test]
fn every_fact_carries_provenance() {
    let core = seeded();
    let bundle = select_facts(&core, &request(&["tool-a", "skill-b"], 10_000)).unwrap();
    assert!(!bundle.facts.is_empty());
    for fact in &bundle.facts {
        assert!(!fact.source_key.is_empty());
        assert_eq!(
            fact.fact_id,
            fact_id(&fact.subject, &fact.predicate, &fact.object)
        );
        assert!(matches!(
            fact.authority,
            EvidenceClass::Proof | EvidenceClass::Observation | EvidenceClass::Claim
        ));
    }
    let edge = bundle
        .facts
        .iter()
        .find(|f| f.predicate == "usesSkill")
        .unwrap();
    assert_eq!(edge.object, "skill-b");
    assert_eq!(edge.source_key, "src-edge");
    let version = bundle
        .facts
        .iter()
        .find(|f| f.predicate == "version")
        .unwrap();
    assert_eq!(version.authority, EvidenceClass::Proof);
}
