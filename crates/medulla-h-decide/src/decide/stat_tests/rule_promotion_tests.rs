//! The `DecisionLog` `PromoteRule` attach point (MEDULLA-DECISION-ENGINE-R129.1).

use super::*;
use medulla_agent_types::agent_component::AgentComponentPublishRequest;
use medulla_agent_types::agent_guardrail::{GuardrailRule, RuleInvariant, RulePromotionVerdict};
use medulla_persistence_records::agent_fixtures::mutation_context;

const FOREIGN: &str = "tenant-foreign";

fn invariant(iri: &str, rule_id: &str, on_class: &str) -> RuleInvariant {
    RuleInvariant {
        invariant_iri: iri.to_string(),
        forbidden_rule_id: rule_id.to_string(),
        on_class: on_class.to_string(),
    }
}

fn publish_invariant(h: &Harness, tenant: &str, nonce: u8, inv: &RuleInvariant) {
    let mut draft = test_component_draft(tenant, &inv.invariant_iri);
    draft.kind = AgentComponentKind::RuleInvariant;
    draft.attributes = inv.to_attributes();
    let key = format!("invariant:{tenant}:{}", inv.invariant_iri);
    h.store
        .publish_component(AgentComponentPublishRequest {
            context: mutation_context(&h.store, tenant, &key, nonce, 0, "agent-component:publish"),
            evaluation_receipt_digest: None,
            component: draft,
        })
        .expect("invariant publishes");
}

fn promote_op(rule_id: &str, class: &str) -> DecisionLogOp {
    DecisionLogOp::PromoteRule {
        tenant_id: TENANT.to_string(),
        rule: GuardrailRule {
            rule_id: rule_id.to_string(),
            applies_to_class: class.to_string(),
            source_generation: "gen-1".to_string(),
        },
        resolution_ids: BoundedVec::new(vec!["res-1".to_string()]).unwrap(),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R129.1
#[tokio::test]
async fn attach_point_reads_published_invariants() {
    let h = Harness::new().await;
    let own_b = invariant("urn:inv:b", "rule:skip-review", "eg:task/deploy");
    let own_a = invariant("urn:inv:a", "rule:skip-review", "eg:task/deploy");
    publish_invariant(&h, TENANT, 201, &own_b);
    publish_invariant(&h, TENANT, 202, &own_a);
    // A foreign tenant's invariant forbids a rule the caller's tenant allows.
    publish_invariant(
        &h,
        FOREIGN,
        203,
        &invariant("urn:inv:foreign", "rule:fast-path", "eg:task/deploy"),
    );

    assert_eq!(
        h.store.published_rule_invariants(TENANT).unwrap(),
        vec![own_a.clone(), own_b]
    );

    // The op reaches the gate; the first contradicted invariant in IRI order
    // names the rejection.
    let verdict: RulePromotionVerdict = decode(
        log_op(
            &h,
            "admin",
            promote_op("rule:skip-review", "eg:task/deploy"),
        )
        .await,
    )
    .unwrap();
    assert_eq!(
        verdict,
        RulePromotionVerdict::RejectedInvariant {
            invariant_iri: own_a.invariant_iri
        }
    );

    // Only the caller tenant's invariants enter the gate.
    let verdict: RulePromotionVerdict =
        decode(log_op(&h, "admin", promote_op("rule:fast-path", "eg:task/deploy")).await).unwrap();
    assert_eq!(verdict, RulePromotionVerdict::Promotable);

    // A malformed invariant is refused at publish, never stored.
    let mut bad = test_component_draft(TENANT, "urn:inv:bad");
    bad.kind = AgentComponentKind::RuleInvariant;
    assert!(h
        .store
        .publish_component(AgentComponentPublishRequest {
            context: mutation_context(
                &h.store,
                TENANT,
                "invariant:bad",
                204,
                0,
                "agent-component:publish"
            ),
            evaluation_receipt_digest: None,
            component: bad,
        })
        .unwrap_err()
        .contains("MALFORMED_GUARDRAIL_RULE"));
}
