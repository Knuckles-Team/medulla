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

/// Store `human` human and `model` model resolutions of one logged abstention
/// and return their ids.
async fn resolutions(h: &Harness, human: usize, model: usize) -> Vec<String> {
    use super::consumer_tests::{logged_abstention, resolution, resolve};
    use medulla_decision_types::decision::statistical::log::AbstentionResolver;
    let record_id = logged_abstention(h).await.record_id;
    let mut ids = Vec::new();
    for i in 0..human + model {
        let (id, resolver) = if i < human {
            (format!("human-{i}"), AbstentionResolver::Human)
        } else {
            (
                format!("model-{i}"),
                AbstentionResolver::Model {
                    producer: "au-escalation".to_string(),
                    prompt_digest: None,
                },
            )
        };
        resolve(h, resolution(&record_id, &id, "plan-deep", resolver))
            .await
            .unwrap();
        ids.push(id);
    }
    ids
}

fn promote_op(rule_id: &str, ids: &[String]) -> DecisionLogOp {
    DecisionLogOp::PromoteRule {
        tenant_id: TENANT.to_string(),
        rule: GuardrailRule {
            rule_id: rule_id.to_string(),
            applies_to_class: "eg:task".to_string(),
            source_generation: "gen-1".to_string(),
        },
        resolution_ids: BoundedVec::new(ids.to_vec()).unwrap(),
    }
}

async fn promote(
    h: &Harness,
    rule_id: &str,
    ids: &[String],
) -> Result<RulePromotionResult, String> {
    decode(super::log_tests::log_op(h, "governor", promote_op(rule_id, ids)).await)
}

// spec: MEDULLA-DECISION-ENGINE-R129.1
#[tokio::test]
async fn attach_point_reads_published_invariants() {
    let h = Harness::new().await;
    let ids = resolutions(&h, 5, 0).await;
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
    let rejected = promote(&h, "rule:unaudited", &ids).await.unwrap_err();
    assert!(
        rejected.starts_with("RULE_CONTRADICTS_INVARIANT")
            && rejected.contains("inv:a-no-unaudited"),
        "{rejected}"
    );
    assert!(h
        .store
        .published_guardrail_rules(TENANT)
        .unwrap()
        .is_empty());

    // A foreign tenant's invariant never reaches the gate.
    let promoted = promote(&h, "rule:foreign-only", &ids).await.unwrap();
    assert_eq!(promoted.verdict, RulePromotionVerdict::Promotable);
    assert!(promoted.published && promoted.definition_digest.is_some());
    let rules = h.store.published_guardrail_rules(TENANT).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].rule_id, "rule:foreign-only");

    // Promoting the same rule again is a no-op replay.
    assert_eq!(
        promote(&h, "rule:foreign-only", &ids).await.unwrap(),
        promoted
    );

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

// spec: MEDULLA-DECISION-ENGINE-R129.3, MEDULLA-DECISION-ENGINE-R129.4, MEDULLA-DECISION-ENGINE-R129.5
#[tokio::test]
async fn contradicting_rule_is_rejected_and_compatible_rule_promotes() {
    use super::super::stat_rule_promotion::RulePromotionRecord;
    let h = Harness::new().await;
    let ids = resolutions(&h, 5, 0).await;
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

    let stored = || {
        h.store
            .decision_artifacts_with_prefix(TENANT, "rule_promotion:", 100)
            .unwrap()
            .into_iter()
            .map(|(_, bytes)| {
                medulla_persistence_records::decision_jobs::decode_artifact::<RulePromotionRecord>(
                    &bytes, "record",
                )
                .unwrap()
            })
            .collect::<Vec<_>>()
    };

    // RULE_CONTRADICTS_INVARIANT: refused, audited, and nothing activated.
    let rejected = promote(&h, "rule:skip-review", &ids).await.unwrap_err();
    assert!(
        rejected.starts_with("RULE_CONTRADICTS_INVARIANT")
            && rejected.contains("inv:review-required"),
        "{rejected}"
    );
    let records = stored();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].verdict,
        RulePromotionVerdict::RejectedInvariant {
            invariant_iri: "inv:review-required".to_string()
        }
    );
    assert_eq!(records[0].rule.rule_id, "rule:skip-review");
    assert_eq!(records[0].agreement, 1.0);
    assert_eq!(records[0].resolution_ids.len(), 5);
    assert!(h
        .store
        .published_guardrail_rules(TENANT)
        .unwrap()
        .is_empty());
    // A replay of the same rejection stores no second record.
    promote(&h, "rule:skip-review", &ids).await.unwrap_err();
    assert_eq!(stored().len(), 1);

    // RULE_PROMOTION_TOO_FEW_OVERRIDES: fewer human resolutions than the knob.
    let few = vec![ids[0].clone(), ids[1].clone()];
    let refused = promote(&h, "rule:add-check", &few).await.unwrap_err();
    assert!(
        refused.starts_with("RULE_PROMOTION_TOO_FEW_OVERRIDES"),
        "{refused}"
    );
    assert!(h
        .store
        .published_guardrail_rules(TENANT)
        .unwrap()
        .is_empty());

    // Agreement below the knob refuses without publishing.
    let mixed = resolutions(&h, 0, 5).await;
    let mixed: Vec<String> = ids.iter().cloned().chain(mixed).collect();
    let low = promote(&h, "rule:add-check", &mixed).await.unwrap_err();
    assert!(low.starts_with("RULE_PROMOTION_AGREEMENT_LOW"), "{low}");

    // A named resolution that does not exist is refused.
    let ghost = vec!["no-such".to_string()];
    assert!(promote(&h, "rule:add-check", &ghost)
        .await
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));

    // A compatible rule promotes, is recorded, and becomes the one active rule.
    let promoted = promote(&h, "rule:add-check", &ids).await.unwrap();
    assert_eq!(promoted.verdict, RulePromotionVerdict::Promotable);
    assert!(promoted.published && promoted.definition_digest.is_some());
    let rules = h.store.published_guardrail_rules(TENANT).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].rule_id, "rule:add-check");
    assert_eq!(stored().len(), 2);
}

// spec: MEDULLA-DECISION-ENGINE-R129.5
#[tokio::test]
async fn rejected_rule_is_audited_not_active() {
    use super::super::stat_rule_promotion::RulePromotionRecord;
    let h = Harness::new().await;
    let ids = resolutions(&h, 5, 0).await;
    publish_invariant(
        &h,
        TENANT,
        1,
        &invariant("inv:review-required", "rule:skip-review"),
    );

    // Full human agreement does not override the invariant.
    let rejected = promote(&h, "rule:skip-review", &ids).await.unwrap_err();
    assert!(
        rejected.starts_with("RULE_CONTRADICTS_INVARIANT")
            && rejected.contains("inv:review-required"),
        "{rejected}"
    );

    // The rejection is audited as one stored record naming the invariant.
    let records: Vec<RulePromotionRecord> = h
        .store
        .decision_artifacts_with_prefix(TENANT, "rule_promotion:", 100)
        .unwrap()
        .into_iter()
        .map(|(_, bytes)| {
            medulla_persistence_records::decision_jobs::decode_artifact::<RulePromotionRecord>(
                &bytes, "record",
            )
            .unwrap()
        })
        .collect();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].verdict,
        RulePromotionVerdict::RejectedInvariant {
            invariant_iri: "inv:review-required".to_string()
        }
    );
    assert_eq!(records[0].resolution_ids.len(), 5);

    // The rejected rule is not active.
    assert!(h
        .store
        .published_guardrail_rules(TENANT)
        .unwrap()
        .iter()
        .all(|rule| rule.rule_id != "rule:skip-review"));
}
