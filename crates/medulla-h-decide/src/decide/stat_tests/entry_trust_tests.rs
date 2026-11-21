//! Entry admission wired into `Decide` (MEDULLA-DECISION-ENGINE-R132.3): the
//! retrieved entries of a question are admitted once against the policy's known
//! source digests, an admitted entry is one premise with its trust tier as the
//! class, and an unverified entry that contradicts a verified fact is refused
//! and leaves no premise.

use super::consumer_tests::{declared, declared_request, score_schema};
use super::*;
use medulla_decision_types::decision::entry_types::{
    EntryCheck, EntryTrust, RefusedEntry, RetrievedEntry, TrustTier,
};
use medulla_decision_types::decision::entry_types::POISONED_ENTRY;
use medulla_decision_types::decision::{AbstainReason, PremiseClass, PremiseProvenance};

const SOURCE: &str = "sha256:known-source";

fn entry(id: &str, tier: TrustTier, value: &str) -> RetrievedEntry {
    RetrievedEntry {
        entry_id: id.to_string(),
        subject: "urn:agent:planner".to_string(),
        fact_key: "max_tools".to_string(),
        value: value.to_string(),
        trust: EntryTrust {
            tier,
            source_digest: SOURCE.to_string(),
            observed_at: 10,
        },
    }
}

// spec: MEDULLA-DECISION-ENGINE-R132.3, MEDULLA-DECISION-ENGINE-R132.6
#[tokio::test]
async fn unverified_entry_cannot_override_a_verified_fact() {
    let h = Harness::new().await;
    let schema = h
        .publish_test_component(
            "schema-declared",
            AgentComponentKind::FeatureSchema,
            "declared-option features",
            Some(&score_schema()),
            None,
        )
        .unwrap();
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy.entry_trust.known_source_digests.insert(SOURCE.to_string());
    let pinned = DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-entry-trust", &policy),
    };
    let ask = |entries: Vec<RetrievedEntry>| {
        let mut request = declared_request(
            &schema,
            vec![declared("plan-a", 1 << 31), declared("plan-b", 1 << 32)],
        );
        request.policy = pinned.clone();
        request.params = BoundedVec::new(vec![TypedParam {
            name: "retrieved_entries".to_string(),
            value: TypedValue::RetrievedEntries(BoundedVec::new(entries).unwrap()),
        }])
        .unwrap();
        request
    };

    // A verified and an unverified contradicting entry on one key: one premise.
    let batch = decide(
        &h,
        ask(vec![
            entry("e-verified", TrustTier::Verified, "8"),
            entry("e-unverified", TrustTier::Unverified, "99"),
        ]),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let entries: Vec<_> = record
        .premises
        .iter()
        .filter(|premise| matches!(premise.provenance, PremiseProvenance::RetrievedEntry { .. }))
        .collect();
    assert_eq!(entries.len(), 1, "{:?}", record.premises);
    assert_eq!(entries[0].fact, "max_tools=8");
    assert_eq!(entries[0].class, PremiseClass::Observation);
    assert_eq!(
        entries[0].provenance,
        PremiseProvenance::RetrievedEntry {
            entry_id: "e-verified".to_string(),
            source_digest: SOURCE.to_string(),
        }
    );
    assert!(!record.premises.iter().any(|p| p.fact == "max_tools=99"));
    // The unverified entry is in the sealed record's refused entries, not a premise.
    assert_eq!(
        record.refused_entries.as_slice(),
        [RefusedEntry {
            entry_id: "e-unverified".to_string(),
            tier: TrustTier::Unverified,
            check: EntryCheck::PoisonCheck,
        }]
    );

    // An attested entry is a claim.
    let batch = decide(&h, ask(vec![entry("e-attested", TrustTier::Attested, "5")]))
        .await
        .unwrap();
    let premise = batch.records.as_slice()[0]
        .premises
        .iter()
        .find(|p| p.fact == "max_tools=5")
        .expect("an attested entry is admitted");
    assert_eq!(premise.class, PremiseClass::Claim);

    // A question with no entries is untouched.
    let mut plain = ask(Vec::new());
    plain.params = BoundedVec::default();
    let batch = decide(&h, plain).await.unwrap();
    assert!(!batch.records.as_slice()[0]
        .premises
        .iter()
        .any(|p| matches!(p.provenance, PremiseProvenance::RetrievedEntry { .. })));

    // A second param with the name refuses.
    let mut twice = ask(vec![entry("e1", TrustTier::Verified, "8")]);
    let mut params = twice.params.as_slice().to_vec();
    params.push(params[0].clone());
    twice.params = BoundedVec::new(params).unwrap();
    let error = decide(&h, twice).await.unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");

    // A request for another tenant by a caller of this one is refused.
    let mut foreign = ask(vec![entry("e1", TrustTier::Verified, "8")]);
    foreign.tenant_id = "tenant-other".to_string();
    let other = VerifiedRequestContext::verified_for_test_in_tenant("decider", "tenant-other");
    let error = decode::<DecisionBatch>(
        super::super::statistical::handle_decide(&h.state, 1, &other, foreign).await,
    )
    .unwrap_err();
    assert!(error.starts_with("COMPONENT_PIN_MISMATCH"), "{error}");
}

// spec: MEDULLA-DECISION-ENGINE-R132.4
#[tokio::test]
async fn poisoned_only_required_fact_abstains() {
    let h = Harness::new().await;
    let schema = h
        .publish_test_component(
            "schema-declared",
            AgentComponentKind::FeatureSchema,
            "declared-option features",
            Some(&score_schema()),
            None,
        )
        .unwrap();
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy.entry_trust.known_source_digests.insert(SOURCE.to_string());
    let pinned = DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-entry-poison", &policy),
    };
    let ask = |entries: Vec<RetrievedEntry>, required: Vec<String>| {
        let mut request = declared_request(
            &schema,
            vec![declared("plan-a", 1 << 31), declared("plan-b", 1 << 32)],
        );
        request.policy = pinned.clone();
        request.params = BoundedVec::new(vec![
            TypedParam {
                name: "retrieved_entries".to_string(),
                value: TypedValue::RetrievedEntries(BoundedVec::new(entries).unwrap()),
            },
            TypedParam {
                name: "entry.required_facts".to_string(),
                value: TypedValue::IriList(BoundedVec::new(required).unwrap()),
            },
        ])
        .unwrap();
        request
    };
    let required = || vec!["urn:agent:planner#max_tools".to_string()];
    let mut poisoned = entry("e-poisoned", TrustTier::Verified, "99");
    poisoned.trust.source_digest = "sha256:unknown-source".to_string();

    // The only entry of the required fact has an unknown source: abstain.
    let batch = decide(&h, ask(vec![poisoned.clone()], required())).await.unwrap();
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("a poisoned required fact abstains, got {:?}", record.outcome);
    };
    match &reasons.as_slice()[0] {
        AbstainReason::Family { code, detail, .. } => {
            assert_eq!(*code, POISONED_ENTRY);
            assert_eq!(detail, "entry e-poisoned refused by poison_check");
        }
        other => panic!("expected a family abstention, got {other:?}"),
    }
    assert!(record.answer.is_none());

    // One admitted verified entry on the same fact: no abstention.
    let batch = decide(
        &h,
        ask(
            vec![poisoned.clone(), entry("e-verified", TrustTier::Verified, "8")],
            required(),
        ),
    )
    .await
    .unwrap();
    assert!(!matches!(
        batch.records.as_slice()[0].outcome,
        StatisticalOutcome::Abstained { .. }
    ));

    // A required fact with no entry at all: nothing was refused.
    let batch = decide(
        &h,
        ask(vec![poisoned], vec!["urn:agent:planner#other_key".to_string()]),
    )
    .await
    .unwrap();
    assert!(!matches!(
        batch.records.as_slice()[0].outcome,
        StatisticalOutcome::Abstained { .. }
    ));

    // An item without a `#` refuses.
    let error = decide(
        &h,
        ask(
            vec![entry("e1", TrustTier::Verified, "8")],
            vec!["nohash".to_string()],
        ),
    )
    .await
    .unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
}

// spec: MEDULLA-DECISION-ENGINE-R132.5
#[tokio::test]
async fn refused_entries_are_recorded_and_replay() {
    let h = Harness::new().await;
    let schema = h
        .publish_test_component(
            "schema-declared",
            AgentComponentKind::FeatureSchema,
            "declared-option features",
            Some(&score_schema()),
            None,
        )
        .unwrap();
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy.entry_trust.known_source_digests.insert(SOURCE.to_string());
    let mut request = declared_request(
        &schema,
        vec![declared("plan-a", 1 << 31), declared("plan-b", 1 << 32)],
    );
    request.policy = DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-entry-record", &policy),
    };
    // A verified fact, an attested entry that agrees with it, an attested entry
    // that contradicts it (refused by tier order) and an unverified one that
    // contradicts it (refused by the poison check).
    let entries = vec![
        entry("e-verified", TrustTier::Verified, "8"),
        entry("e-agree", TrustTier::Attested, "8"),
        entry("e-attested", TrustTier::Attested, "5"),
        entry("e-unverified", TrustTier::Unverified, "99"),
    ];
    request.params = BoundedVec::new(vec![TypedParam {
        name: "retrieved_entries".to_string(),
        value: TypedValue::RetrievedEntries(BoundedVec::new(entries).unwrap()),
    }])
    .unwrap();
    let batch = decide(&h, request).await.unwrap();
    let record = &batch.records.as_slice()[0];

    let tiers: Vec<_> = record
        .premises
        .iter()
        .filter_map(|premise| match &premise.provenance {
            PremiseProvenance::RetrievedEntry { entry_id, .. } => {
                Some((entry_id.as_str(), premise.trust_tier))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        tiers,
        [
            ("e-agree", Some(TrustTier::Attested)),
            ("e-verified", Some(TrustTier::Verified)),
        ]
    );
    assert!(
        record
            .premises
            .iter()
            .filter(|premise| !matches!(premise.provenance, PremiseProvenance::RetrievedEntry { .. }))
            .all(|premise| premise.trust_tier.is_none()),
        "only an admitted entry carries a tier"
    );
    assert_eq!(
        record.refused_entries.as_slice(),
        [
            RefusedEntry {
                entry_id: "e-attested".to_string(),
                tier: TrustTier::Attested,
                check: EntryCheck::TierOrder,
            },
            RefusedEntry {
                entry_id: "e-unverified".to_string(),
                tier: TrustTier::Unverified,
                check: EntryCheck::PoisonCheck,
            },
        ]
    );

    // The record replays: the committed entry verifies against its digest.
    #[cfg(feature = "blob")]
    {
        let id = record.record_id.clone();
        let _: DecisionLogCommitted = decode(
            log_op(
                &h,
                "decider",
                DecisionLogOp::Commit {
                    record: Box::new(record.clone()),
                    evaluator: None,
                    approval: None,
                },
            )
            .await,
        )
        .unwrap();
        assert!(matches!(
            super::log_tests::verify_log(&h, &id).await.unwrap(),
            medulla_decision_types::decision::statistical::log::DecisionLogVerification::Verified { .. }
        ));
    }
}
