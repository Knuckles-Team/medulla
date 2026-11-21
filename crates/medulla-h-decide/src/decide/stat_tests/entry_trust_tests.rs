//! Entry admission wired into `Decide` (MEDULLA-DECISION-ENGINE-R132.3): the
//! retrieved entries of a question are admitted once against the policy's known
//! source digests, an admitted entry is one premise with its trust tier as the
//! class, and an unverified entry that contradicts a verified fact is refused
//! and leaves no premise.

use super::consumer_tests::{declared, declared_request, score_schema};
use super::*;
use medulla_decision_types::decision::entry_types::{
    EntryTrust, RetrievedEntry, TrustTier,
};
use medulla_decision_types::decision::{PremiseClass, PremiseProvenance};

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

// spec: MEDULLA-DECISION-ENGINE-R132.3
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
