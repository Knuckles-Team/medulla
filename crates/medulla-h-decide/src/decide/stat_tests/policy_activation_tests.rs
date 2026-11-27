//! Policy activation (MEDULLA-ORCH-VOCAB-R012.7.3, R012.7.4, R012.8, R806): a review is recorded
//! against the indexed candidates, the promotion gate admits it only from a second principal,
//! and the pointer of `policy:<repository>` moves only behind the invariant guard and the
//! revision the review was made against. Every refusal keeps the prior set.

use super::learning_tests::{activate, pointer_state};
use super::path_preference_tests::pins;
use super::promotion_tests::{commit, promotion_request};
use super::rule_promotion_tests::publish_invariant;
use super::*;
use medulla_agent_types::agent_guardrail::RuleInvariant;
use medulla_decision_stats::decision::promotion::{subject_gate, PromotionGate};
use medulla_decision_types::decision::held_out::{ReceiptKind, RevisionSubject};
use medulla_decision_types::decision::statistical::retrieval::PointerRef;
use medulla_decision_types::decision::statistical::{TypedParam, TypedValue};
use medulla_decision_types::decision::DecisionPolicy;
use medulla_persistence_records::decision_jobs::read_pointer;
use medulla_types::contract::Digest256;
use medulla_types::policy_document::{PolicyBinding, PolicyReview};

use super::super::policy_activation::{
    active_policy, check_activation, invariant_generation, load_review, policy_premises,
    record_review, review_evidence,
};
use super::super::stat_executor::ExecutionContext;
use super::super::stat_vectors::GraphVectors;

const REPO: &str = "org/repo";
const PATH: &str = ".specify/memory/constitution.md";
const SCOPE: &str = "https://example.invalid/class/Repo";

fn sha256_hex(text: &str) -> String {
    Digest256::sha256(text.as_bytes()).to_hex()
}

/// One candidate node of the repository index: `(id, properties)`.
fn candidate(rule: &str, status: &str, set: &str) -> (String, serde_json::Value) {
    let mut properties = serde_json::json!({
        "type": "PolicyCandidate",
        "repo": REPO,
        "path": PATH,
        "status": status,
        "clause_digest": sha256_hex(rule),
        "candidate_set_digest": set,
        "commit": "c0ffee",
        "blob_digest": "ab".repeat(32),
        "start_line": 3,
        "end_line": 3,
        "grammar_revision": 1,
        "_owner_id": "decider",
    });
    if status == "proposed" {
        properties["rule_id"] = format!("policy-{rule}").into();
        properties["scope"] = SCOPE.into();
        properties["severity"] = "block".into();
    } else {
        properties["reason"] = "CLAUSE_UNPARSED".into();
    }
    (format!("policycandidate:{REPO}:{rule}"), properties)
}

/// The candidate-set digest of R012.7.1 over `rules`, computed here independently of the guard.
fn set_of(rules: &[&str]) -> String {
    let mut digests: Vec<String> = rules.iter().map(|rule| sha256_hex(rule)).collect();
    digests.sort();
    sha256_hex(&digests.join("\n"))
}

/// A graph holding the candidates `(rule, status)`, all of one candidate set.
async fn graph_of(h: &Harness, rules: &[(&str, &str)]) -> (String, GraphVectors) {
    let names: Vec<&str> = rules.iter().map(|(rule, _)| *rule).collect();
    let set = set_of(&names);
    let core = std::sync::Arc::new(medulla_graph_core::graph::GraphCore::new());
    for (rule, status) in rules {
        let (id, properties) = candidate(rule, status, &set);
        core.add_node(id, rmp_serde::to_vec_named(&properties).unwrap());
    }
    let isolation = h.state.read().await.isolation.clone();
    (
        set,
        GraphVectors::from_parts(core, isolation, "decider".to_string()),
    )
}

fn ctx<'a>(h: &'a Harness) -> ExecutionContext<'a> {
    ExecutionContext {
        store: &h.store,
        tenant_id: TENANT,
        now_ms: 1_000,
        server_secret: b"",
        matched: &[],
        designation: None,
        reader: None,
        assistant_profile: None,
        observation_windows: None,
        type_digest: None,
        recommend_catalog: None,
        defect_triage: None,
    }
}

fn review(
    set: &str,
    generation: &str,
    expected: Option<&str>,
    receipt_seed: &str,
    (author, reviewer): (&str, &str),
) -> PolicyReview {
    PolicyReview {
        binding: PolicyBinding {
            repository: REPO.to_string(),
            path: PATH.to_string(),
            commit: "c0ffee".to_string(),
            blob_digest: "ab".repeat(32),
            candidate_set_digest: set.to_string(),
            review_receipt_digest: format!("sha256:{}", sha256_hex(receipt_seed)),
            invariant_generation: generation.to_string(),
            expected_policy_revision: expected.map(str::to_string),
        },
        author: author.to_string(),
        reviewer: reviewer.to_string(),
    }
}

fn scope_subject() -> RevisionSubject {
    RevisionSubject::GovernedSet {
        scope_ref: format!("policy:{REPO}"),
    }
}

// spec: MEDULLA-ORCH-VOCAB-R012.8
// spec: MEDULLA-ORCH-VOCAB-R012.7.3
#[tokio::test]
async fn activation_refuses_contradiction_and_stale_review() {
    let h = Harness::new().await;
    let ctx = ctx(&h);
    let (set, graph) = graph_of(&h, &[("no-secrets", "proposed")]).await;
    let generation = invariant_generation(&h.store, TENANT).unwrap();

    // The review is recorded once; the gate admits it only from a second principal.
    let approved = review(&set, &generation, None, "one", ("alice", "bob"));
    let evidence = record_review(&ctx, &graph, approved.clone()).unwrap();
    assert_eq!(evidence.receipt_kind, ReceiptKind::ReviewApproval);
    assert_eq!(evidence.subject, scope_subject());
    assert_eq!(evidence.challenger_ref, format!("policyset:{set}"));
    let policy = DecisionPolicy::engine_default();
    assert_eq!(
        subject_gate(&evidence.subject, &evidence, &policy),
        PromotionGate::Promote
    );
    assert_eq!(
        record_review(&ctx, &graph, approved.clone()).unwrap(),
        evidence
    );
    let mut other = approved.clone();
    other.reviewer = "carol".to_string();
    let mismatch = record_review(&ctx, &graph, other).unwrap_err();
    assert!(mismatch.starts_with("POLICY_REVIEW_MISMATCH"), "{mismatch}");
    let selfie = record_review(
        &ctx,
        &graph,
        review(&set, &generation, None, "self", ("alice", "alice")),
    )
    .unwrap();
    assert!(matches!(
        subject_gate(&selfie.subject, &selfie, &policy),
        PromotionGate::Abstain(_)
    ));

    // A source indexed again since the review no longer yields the reviewed set; an inactive
    // clause cannot be activated; neither is recorded.
    let (new_set, reindexed) = graph_of(
        &h,
        &[("no-secrets", "proposed"), ("no-force-push", "proposed")],
    )
    .await;
    assert_ne!(new_set, set);
    let stale = record_review(
        &ctx,
        &reindexed,
        review(&set, &generation, None, "stale", ("alice", "bob")),
    )
    .unwrap_err();
    assert!(stale.starts_with("POLICY_REVIEW_STALE"), "{stale}");
    let (inactive_set, with_inactive) =
        graph_of(&h, &[("no-secrets", "proposed"), ("garbled", "inactive")]).await;
    let unsupported = record_review(
        &ctx,
        &with_inactive,
        review(
            &inactive_set,
            &generation,
            None,
            "inactive",
            ("alice", "bob"),
        ),
    )
    .unwrap_err();
    assert!(
        unsupported.starts_with("POLICY_CLAUSE_UNSUPPORTED"),
        "{unsupported}"
    );
    assert!(load_review(
        &h.store,
        TENANT,
        &review(&set, &generation, None, "stale", ("a", "b"))
            .binding
            .review_receipt_digest
    )
    .unwrap()
    .is_none());

    // The review holds against the store as it was reviewed.
    let stored = load_review(&h.store, TENANT, &approved.binding.review_receipt_digest)
        .unwrap()
        .unwrap();
    let target = format!("policyset:{set}");
    check_activation(&h.store, TENANT, &stored, Some(&target)).unwrap();
    let other_target =
        check_activation(&h.store, TENANT, &stored, Some("policyset:other")).unwrap_err();
    assert_eq!(other_target.code, "POLICY_REVIEW_MISMATCH");

    // An invariant published after the review moves the generation: the review is stale.
    let forbids = RuleInvariant {
        invariant_iri: "inv:no-secrets".to_string(),
        forbidden_rule_id: "policy-no-secrets".to_string(),
        on_class: SCOPE.to_string(),
    };
    publish_invariant(&h, TENANT, 1, &forbids);
    let moved = check_activation(&h.store, TENANT, &stored, Some(&target)).unwrap_err();
    assert_eq!(moved.code, "POLICY_REVIEW_STALE");

    // A review of the new generation still cannot activate a rule that invariant forbids; the
    // refusal names the invariant, the rule and the clause.
    let generation = invariant_generation(&h.store, TENANT).unwrap();
    record_review(
        &ctx,
        &graph,
        review(&set, &generation, None, "two", ("alice", "bob")),
    )
    .unwrap();
    let again = load_review(
        &h.store,
        TENANT,
        &review(&set, &generation, None, "two", ("alice", "bob"))
            .binding
            .review_receipt_digest,
    )
    .unwrap()
    .unwrap();
    let contradiction = check_activation(&h.store, TENANT, &again, Some(&target)).unwrap_err();
    assert_eq!(contradiction.code, "POLICY_CONTRADICTS_INVARIANT");
    for named in [
        "inv:no-secrets",
        "policy-no-secrets",
        format!("{PATH}:3-3").as_str(),
    ] {
        assert!(
            contradiction.detail.contains(named),
            "{}",
            contradiction.detail
        );
    }
    // Each refusal kept the prior set: nothing is active.
    assert!(active_policy(&h.store, TENANT, REPO).unwrap().is_none());
}

// spec: MEDULLA-ORCH-VOCAB-R012.7.3
// spec: MEDULLA-ORCH-VOCAB-R012.7.4
#[tokio::test]
async fn reviewed_set_promotes_through_decide_and_moves_the_pointer() {
    let h = Harness::new().await;
    let ctx = ctx(&h);
    let pins = pins(&h, TENANT);
    let champion = PointerRef::Champion {
        subject: scope_subject(),
    };
    let (first_set, first_graph) = graph_of(&h, &[("no-secrets", "proposed")]).await;
    let generation = invariant_generation(&h.store, TENANT).unwrap();
    let first = review(&first_set, &generation, None, "first", ("alice", "bob"));
    record_review(&ctx, &first_graph, first.clone()).unwrap();
    let first_target = first.binding.target();

    // The review is evidence only for its own scope.
    let evidence = review_evidence(
        &h.store,
        TENANT,
        None,
        "policy:org/other",
        &first.binding.review_receipt_digest,
    )
    .unwrap()
    .unwrap();
    assert!(evidence.is_none());

    // Decide (kind Promotion) judges the review; the committed record moves the pointer.
    let decided = decide(
        &h,
        promotion_request(
            &pins.0,
            &scope_subject(),
            &first.binding.review_receipt_digest,
            &first_target,
        ),
    )
    .await
    .unwrap()
    .records
    .as_slice()[0]
        .clone();
    commit(&h, &decided).await;
    let state = pointer_state(
        &h,
        champion.clone(),
        activate(&first_target, &decided.record_id),
    )
    .await
    .unwrap();
    assert_eq!(state.target(), Some(first_target.as_str()));
    let active = active_policy(&h.store, TENANT, REPO).unwrap().unwrap();
    assert_eq!(active.revision, first_target);
    assert_eq!(active.review.review, first);

    // A Decide that loads the active policy records its revision and candidate-set digest, and
    // an equal revision reproduces the digest (R012.7.4).
    let mut request = promotion_request(
        &pins.0,
        &scope_subject(),
        &first.binding.review_receipt_digest,
        &first_target,
    );
    let mut params: Vec<TypedParam> = request.params.iter().cloned().collect();
    params.push(TypedParam {
        name: "policy_repository".to_string(),
        value: TypedValue::Text(REPO.to_string()),
    });
    request.params = BoundedVec::new(params).unwrap();
    let facts_of = |premises: Vec<medulla_decision_types::decision::PremiseRef>| -> Vec<String> {
        premises.into_iter().map(|premise| premise.fact).collect()
    };
    let first_facts = facts_of(policy_premises(&ctx, &request).unwrap());
    assert_eq!(
        first_facts,
        [
            format!("policy.revision={first_target}"),
            format!("policy.candidate_set_digest={first_set}"),
        ]
    );
    assert_eq!(
        facts_of(policy_premises(&ctx, &request).unwrap()),
        first_facts
    );
    let sealed = decide(&h, request.clone())
        .await
        .unwrap()
        .records
        .as_slice()[0]
        .clone();
    let sealed_facts: Vec<&str> = sealed.premises.iter().map(|p| p.fact.as_str()).collect();
    for fact in &first_facts {
        assert!(sealed_facts.contains(&fact.as_str()), "{sealed_facts:?}");
    }
    // A request that names no repository adds nothing; an unknown repository adds `none`.
    let plain = promotion_request(&pins.0, &scope_subject(), "x", &first_target);
    assert!(policy_premises(&ctx, &plain).unwrap().is_empty());
    let mut unknown = request;
    let mut params: Vec<TypedParam> = unknown
        .params
        .iter()
        .filter(|param| param.name != "policy_repository")
        .cloned()
        .collect();
    params.push(TypedParam {
        name: "policy_repository".to_string(),
        value: TypedValue::Text("org/none".to_string()),
    });
    unknown.params = BoundedVec::new(params).unwrap();
    assert_eq!(
        facts_of(policy_premises(&ctx, &unknown).unwrap()),
        ["policy.revision=none", "policy.candidate_set_digest=none"]
    );

    // A second review made against no active policy cannot take the seat: the revision raced.
    let (second_set, second_graph) = graph_of(
        &h,
        &[("no-secrets", "proposed"), ("no-force-push", "proposed")],
    )
    .await;
    let raced = review(&second_set, &generation, None, "raced", ("alice", "bob"));
    record_review(&ctx, &second_graph, raced.clone()).unwrap();
    let second_target = raced.binding.target();
    let decided = decide(
        &h,
        promotion_request(
            &pins.0,
            &scope_subject(),
            &raced.binding.review_receipt_digest,
            &second_target,
        ),
    )
    .await
    .unwrap()
    .records
    .as_slice()[0]
        .clone();
    commit(&h, &decided).await;
    let conflict = pointer_state(
        &h,
        champion.clone(),
        activate(&second_target, &decided.record_id),
    )
    .await
    .unwrap_err();
    assert!(conflict.starts_with("PROMOTION_CAS_CONFLICT"), "{conflict}");

    // An invariant that forbids a reviewed rule refuses the move; the first set stays active.
    let forbids = RuleInvariant {
        invariant_iri: "inv:no-force-push".to_string(),
        forbidden_rule_id: "policy-no-force-push".to_string(),
        on_class: SCOPE.to_string(),
    };
    publish_invariant(&h, TENANT, 2, &forbids);
    let generation = invariant_generation(&h.store, TENANT).unwrap();
    let forbidden = review(
        &second_set,
        &generation,
        Some(&first_target),
        "forbidden",
        ("alice", "bob"),
    );
    record_review(&ctx, &second_graph, forbidden.clone()).unwrap();
    let decided = decide(
        &h,
        promotion_request(
            &pins.0,
            &scope_subject(),
            &forbidden.binding.review_receipt_digest,
            &second_target,
        ),
    )
    .await
    .unwrap()
    .records
    .as_slice()[0]
        .clone();
    commit(&h, &decided).await;
    let refused = pointer_state(&h, champion, activate(&second_target, &decided.record_id))
        .await
        .unwrap_err();
    assert!(
        refused.starts_with("POLICY_CONTRADICTS_INVARIANT"),
        "{refused}"
    );
    assert!(refused.contains("inv:no-force-push"), "{refused}");
    let (_, kept) = read_pointer(&h.store, TENANT, &scope_subject().pointer_key()).unwrap();
    assert_eq!(kept.target(), Some(first_target.as_str()));
}
