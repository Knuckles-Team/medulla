//! `DecisionLog.promote_rule` attach point (MEDULLA-DECISION-ENGINE-R129.1):
//! the op reaches the invariant gate with the caller tenant's published
//! invariants only, and a passing rule becomes an active `GuardrailRule`.

use super::*;
use medulla_agent_types::agent_guardrail::{GuardrailRule, RuleInvariant, RulePromotionVerdict};
use medulla_decision_types::decision::statistical::log::RulePromotionResult;
use medulla_persistence_records::agent_component::test_component_draft;
use medulla_persistence_records::agent_fixtures::mutation_context;

const FOREIGN: &str = "tenant-b";

fn invariant(iri: &str, forbidden: &str) -> RuleInvariant {
    RuleInvariant {
        invariant_iri: iri.to_string(),
        forbidden_rule_id: forbidden.to_string(),
        on_class: "eg:task".to_string(),
    }
}

fn publish_invariant(h: &Harness, tenant: &str, nonce: u8, invariant: &RuleInvariant) {
    let mut draft = test_component_draft(tenant, &invariant.invariant_iri);
    draft.kind = AgentComponentKind::RuleInvariant;
    draft.attributes = invariant.to_attributes();
    h.store
        .publish_component(AgentComponentPublishRequest {
            context: mutation_context(
                &h.store,
                tenant,
                &format!("invariant-{}", invariant.invariant_iri),
                nonce,
                0,
                "agent-component:publish",
            ),
            evaluation_receipt_digest: None,
            component: draft,
        })
        .expect("invariant publishes");
}

fn promote_op(rule_id: &str) -> DecisionLogOp {
    DecisionLogOp::PromoteRule {
        tenant_id: TENANT.to_string(),
        rule: GuardrailRule {
            rule_id: rule_id.to_string(),
            applies_to_class: "eg:task".to_string(),
            source_generation: "gen-1".to_string(),
        },
        resolution_ids: BoundedVec::new(vec!["resolution-1".to_string()]).unwrap(),
    }
}

async fn promote(h: &Harness, rule_id: &str) -> Result<RulePromotionResult, String> {
    decode(super::log_tests::log_op(h, "governor", promote_op(rule_id)).await)
}

// spec: MEDULLA-DECISION-ENGINE-R129.1
#[tokio::test]
async fn attach_point_reads_published_invariants() {
    let h = Harness::new().await;
    publish_invariant(
        &h,
        TENANT,
        1,
        &invariant("inv:b-no-skip-review", "rule:skip-review"),
    );
    publish_invariant(
        &h,
        TENANT,
        2,
        &invariant("inv:a-no-unaudited", "rule:unaudited"),
    );
    // Another tenant's invariant forbids a rule this tenant may promote.
    publish_invariant(
        &h,
        FOREIGN,
        3,
        &invariant("inv:foreign", "rule:foreign-only"),
    );

    // The store read is tenant-scoped and in invariant id order.
    let own: Vec<String> = h
        .store
        .published_rule_invariants(TENANT)
        .unwrap()
        .into_iter()
        .map(|inv| inv.invariant_iri)
        .collect();
    assert_eq!(own, ["inv:a-no-unaudited", "inv:b-no-skip-review"]);

    // A rule that contradicts this tenant's invariant is rejected, naming it,
    // and publishes nothing.
    let rejected = promote(&h, "rule:unaudited").await.unwrap();
    assert_eq!(
        rejected.verdict,
        RulePromotionVerdict::RejectedInvariant {
            invariant_iri: "inv:a-no-unaudited".to_string()
        }
    );
    assert!(!rejected.published);
    assert!(h
        .store
        .published_guardrail_rules(TENANT)
        .unwrap()
        .is_empty());

    // A foreign tenant's invariant never reaches the gate.
    let promoted = promote(&h, "rule:foreign-only").await.unwrap();
    assert_eq!(promoted.verdict, RulePromotionVerdict::Promotable);
    assert!(promoted.published && promoted.definition_digest.is_some());
    let rules = h.store.published_guardrail_rules(TENANT).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].rule_id, "rule:foreign-only");

    // Promoting the same rule again is a no-op replay.
    assert_eq!(promote(&h, "rule:foreign-only").await.unwrap(), promoted);

    // A request with no stored resolution is refused.
    let empty = DecisionLogOp::PromoteRule {
        tenant_id: TENANT.to_string(),
        rule: GuardrailRule {
            rule_id: "rule:x".into(),
            applies_to_class: "eg:task".into(),
            source_generation: "gen-1".into(),
        },
        resolution_ids: BoundedVec::default(),
    };
    assert!(
        decode::<RulePromotionResult>(super::log_tests::log_op(&h, "governor", empty).await)
            .is_err()
    );
}

// spec: MEDULLA-DECISION-ENGINE-R129.3, MEDULLA-DECISION-ENGINE-R129.4
#[tokio::test]
async fn contradicting_rule_is_rejected_and_compatible_rule_promotes() {
    let h = Harness::new().await;
    publish_invariant(
        &h,
        TENANT,
        1,
        &invariant("inv:review-required", "rule:skip-review"),
    );
    // The first contradicted invariant, in invariant order, is the one named,
    // however many resolutions back the rule.
    publish_invariant(
        &h,
        TENANT,
        2,
        &invariant("inv:z-also-review", "rule:skip-review"),
    );

    let rejected = promote(&h, "rule:skip-review").await.unwrap();
    assert_eq!(
        rejected.verdict,
        RulePromotionVerdict::RejectedInvariant {
            invariant_iri: "inv:review-required".to_string()
        }
    );
    assert!(!rejected.published && rejected.definition_digest.is_none());
    assert!(h
        .store
        .published_guardrail_rules(TENANT)
        .unwrap()
        .is_empty());

    // A compatible rule promotes and becomes the one active rule.
    let promoted = promote(&h, "rule:add-check").await.unwrap();
    assert_eq!(promoted.verdict, RulePromotionVerdict::Promotable);
    assert!(promoted.published && promoted.definition_digest.is_some());
    let rules = h.store.published_guardrail_rules(TENANT).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].rule_id, "rule:add-check");
}
