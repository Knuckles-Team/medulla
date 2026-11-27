//! Feature matching served through `Decide` and the feature ledger served by
//! `Method::FeatureLedger` (MEDULLA-DECISION-ENGINE-R707.3): two projects'
//! features match, the unmatched one becomes a plan, the ledger records the gap,
//! carries it through the machine and refuses what the machine refuses, and
//! another tenant sees none of it.

use super::kernel_family_support::decide_as_other_tenant;
use super::path_preference_tests::pins;
use super::*;
use crate::decide::feature_ledger::handle_feature_ledger;
use medulla_decision_types::decision::feature_match::{
    match_option_id, plan_option_id, FeatureLedgerOp, FeatureLedgerView, LedgerStatus,
};
use medulla_decision_types::decision::statistical::declared::{
    DeclaredNumber, DeclaredOption, DeclaredText,
};
use medulla_decision_types::decision::ResolutionKind;

const ONE: i64 = 1 << 32;
/// A commit the repository index holds, readable by the ledger writer.
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
/// A commit only another agent may see.
const HIDDEN_COMMIT: &str = "fedcba9876543210fedcba9876543210fedcba98";

/// Install the real ingest seam and write two commit nodes of the repository
/// index into a graph: one the ledger writer sees, one it does not.
async fn index_commits(h: &Harness) {
    assert!(medulla_server_ops::api::ingest::install_ingest_seam(
        &h.state.read().await.seams,
        Arc::new(medulla_h_ingest::repository_index::IngestSeam),
    ));
    let mut guard = h.state.write().await;
    guard
        .registry
        .create_graph(
            "repo-index",
            medulla_protocol_types::protocol::GraphType::Team,
            None,
        )
        .unwrap();
    let core = guard.registry.get("repo-index").unwrap().core.clone();
    for (digest, owner, visibility) in [
        (COMMIT, "ledger-writer", "public"),
        (HIDDEN_COMMIT, "someone-else", "private"),
    ] {
        let props = serde_json::json!({
            "type": medulla_source_types::source_changeset::COMMIT_TYPE,
            "_owner": owner,
            "_visibility": visibility,
        });
        core.add_node(
            medulla_source_types::source_changeset::commit_node_id(digest),
            rmp_serde::to_vec_named(&props).unwrap(),
        );
    }
}
const OTHER_TENANT: &str = "tenant-ledger-other";

fn feature(
    id: &str,
    project: &str,
    name: &str,
    label: &str,
    cosine: &[(&str, i64)],
) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::new(vec![label.to_string()]).unwrap(),
        numbers: BoundedVec::new(
            cosine
                .iter()
                .map(|(peer, q32)| DeclaredNumber {
                    key: format!("cos:{peer}"),
                    q32: *q32,
                })
                .collect(),
        )
        .unwrap(),
        texts: BoundedVec::new(
            [("name", name), ("project", project)]
                .into_iter()
                .map(|(key, text)| DeclaredText {
                    key: key.to_string(),
                    text: text.to_string(),
                })
                .collect(),
        )
        .unwrap(),
    }
}

fn request(
    schema: &ComponentDependency,
    options: Vec<DeclaredOption>,
    local: Option<&str>,
) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.into(),
        question: StatisticalQuestion {
            question_id: "features.match".into(),
            kind: QuestionKind::FeatureMatch,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(options).unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(
            local
                .map(|project| TypedParam {
                    name: "local_project".to_string(),
                    value: TypedValue::Text(project.to_string()),
                })
                .into_iter()
                .collect(),
        )
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: Default::default(),
    }
}

/// Two projects: `f-e1` and `f-l1` are the same capability, `f-e2` has no
/// local counterpart.
fn features() -> Vec<DeclaredOption> {
    vec![
        feature(
            "f-e1",
            "ext",
            "Rate Limiter",
            "eg:cap/limits",
            &[("f-l1", ONE)],
        ),
        feature("f-e2", "ext", "Quantum Annealer", "eg:cap/quantum", &[]),
        feature("f-l1", "local", "Rate Limiter", "eg:cap/limits", &[]),
    ]
}

async fn ledger(
    h: &Harness,
    tenant: &str,
    op: FeatureLedgerOp,
) -> Result<FeatureLedgerView, String> {
    let verified = VerifiedRequestContext::verified_for_test_in_tenant("ledger-writer", tenant);
    decode(handle_feature_ledger(&h.state, 3, &verified, op).await)
}

fn only(view: FeatureLedgerView) -> medulla_decision_types::decision::feature_match::LedgerEntry {
    assert_eq!(view.entries.len(), 1, "{view:?}");
    view.entries.into_iter().next().unwrap()
}

/// Verifies MEDULLA-DECISION-ENGINE-R707.3: a `FeatureMatch` question answers
/// one option per match and per plan, `Method::FeatureLedger` opens, plans and
/// closes an entry through the machine, and another tenant finds nothing.
// spec: MEDULLA-DECISION-ENGINE-R707.3
#[tokio::test]
async fn match_and_ledger_reach_their_handlers() {
    // A provisioned, non-system ledger writer: the hidden commit stays hidden.
    let h = Harness::with_isolation(reader_isolation("ledger-writer", &["repo-index"])).await;
    index_commits(&h).await;
    let pins = pins(&h, TENANT);

    // The match option, the plan option and the typed answer.
    let batch = decide(&h, request(&pins.0, features(), Some("local")))
        .await
        .unwrap();
    let record = batch.records.as_slice()[0].clone();
    assert_eq!(record.question.kind, QuestionKind::FeatureMatch);
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let answer = record
        .answer
        .as_ref()
        .and_then(|a| a.feature_match())
        .expect("the family answers a feature comparison");
    assert_eq!(answer.matches.len(), 1);
    assert_eq!(
        (
            answer.matches[0].local.as_str(),
            answer.matches[0].external.as_str()
        ),
        ("f-e1", "f-l1")
    );
    assert_eq!(answer.plans.len(), 1);
    let plan = answer.plans[0].clone();
    assert_eq!(plan.feature_ids, vec!["f-e2".to_string()]);
    let StatisticalOutcome::Advisory {
        scores, calibrated, ..
    } = &record.outcome
    else {
        panic!("feature_match answers an advisory: {:?}", record.outcome);
    };
    assert!(!calibrated);
    let ids: Vec<&str> = scores.iter().map(|s| s.option_id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            match_option_id("f-e1", "f-l1").as_str(),
            plan_option_id(&plan.plan_id).as_str()
        ]
    );

    // The same question again names the same plan.
    let again = decide(&h, request(&pins.0, features(), Some("local")))
        .await
        .unwrap();
    let again = again.records.as_slice()[0].clone();
    assert_eq!(
        again
            .answer
            .as_ref()
            .and_then(|a| a.feature_match())
            .unwrap()
            .plans[0],
        plan
    );

    // Malformed questions are refused: no local project, a number that is no cosine.
    assert!(decide(&h, request(&pins.0, features(), None))
        .await
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));
    let mut bad = features();
    bad[0].numbers = BoundedVec::new(vec![DeclaredNumber {
        key: "gain".into(),
        q32: ONE,
    }])
    .unwrap();
    assert!(decide(&h, request(&pins.0, bad, Some("local")))
        .await
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));
    let error = decide_as_other_tenant(&h, request(&pins.0, features(), Some("local"))).await;
    // The served tenant boundary refuses a request naming a tenant the
    // caller does not belong to, before any pin is read.
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");

    // The ledger: open the gap with the match it came from.
    let open = |id: &str| FeatureLedgerOp::Open {
        feature_id: id.to_string(),
        match_ids: vec![match_option_id("f-e1", "f-l1")],
    };
    let opened = only(ledger(&h, TENANT, open("f-e2")).await.unwrap());
    assert_eq!(opened.status, LedgerStatus::Open);
    assert_eq!(opened.revision, 0);
    assert_eq!(opened.match_ids, vec![match_option_id("f-e1", "f-l1")]);
    assert!(ledger(&h, TENANT, open("f-e2"))
        .await
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));

    // A close before a plan is the machine's refusal, and changes nothing.
    let refused = ledger(
        &h,
        TENANT,
        FeatureLedgerOp::Close {
            feature_id: "f-e2".into(),
            closed_by: COMMIT.into(),
            expected_revision: 0,
        },
    )
    .await
    .unwrap_err();
    assert!(refused.starts_with("TRANSITION_REFUSED"), "{refused}");
    assert!(refused.contains("FeatureLedgerMachine"), "{refused}");

    // A plan against a stale revision is refused; against the read revision it plans.
    let stale = ledger(
        &h,
        TENANT,
        FeatureLedgerOp::Plan {
            plan: plan.clone(),
            expected_revision: 7,
        },
    )
    .await
    .unwrap_err();
    assert!(stale.starts_with("LEDGER_REVISION_CONFLICT"), "{stale}");
    let planned = only(
        ledger(
            &h,
            TENANT,
            FeatureLedgerOp::Plan {
                plan: plan.clone(),
                expected_revision: 0,
            },
        )
        .await
        .unwrap(),
    );
    assert_eq!(planned.status, LedgerStatus::Planned);
    assert_eq!(planned.revision, 1);
    assert_eq!(planned.plan_id.as_deref(), Some(plan.plan_id.as_str()));
    let replanned = ledger(
        &h,
        TENANT,
        FeatureLedgerOp::Plan {
            plan: plan.clone(),
            expected_revision: 1,
        },
    )
    .await
    .unwrap_err();
    assert!(replanned.starts_with("TRANSITION_REFUSED"), "{replanned}");

    // Another tenant reads none of it and cannot close it.
    let foreign = ledger(
        &h,
        OTHER_TENANT,
        FeatureLedgerOp::Get {
            feature_ids: vec![],
        },
    )
    .await
    .unwrap();
    assert!(foreign.entries.is_empty());
    assert!(ledger(
        &h,
        OTHER_TENANT,
        FeatureLedgerOp::Close {
            feature_id: "f-e2".into(),
            closed_by: COMMIT.into(),
            expected_revision: 1,
        },
    )
    .await
    .unwrap_err()
    .starts_with("PARAMETER_INVALID"));

    // The close lands on the planned entry, and the ledger reads it back.
    let closed = only(
        ledger(
            &h,
            TENANT,
            FeatureLedgerOp::Close {
                feature_id: "f-e2".into(),
                closed_by: COMMIT.into(),
                expected_revision: 1,
            },
        )
        .await
        .unwrap(),
    );
    assert_eq!(closed.status, LedgerStatus::Closed);
    assert_eq!(closed.closed_by.as_deref(), Some(COMMIT));
    assert_eq!(closed.revision, 2);
    let read = ledger(
        &h,
        TENANT,
        FeatureLedgerOp::Get {
            feature_ids: vec!["f-e2".into(), "f-missing".into()],
        },
    )
    .await
    .unwrap();
    assert_eq!(read.entries, vec![closed]);
}

/// Verifies MEDULLA-DECISION-ENGINE-R707.4: a close naming no known change
/// (absent, or one the caller may not see) is `LEDGER_CLOSE_NO_CHANGE`, a stale
/// revision is `LEDGER_REVISION_CONFLICT`, two features linked only by a shared
/// high-entropy name are `MATCH_HIGH_ENTROPY_NAME_ONLY`, and each refusal
/// leaves the ledger as it was.
// spec: MEDULLA-DECISION-ENGINE-R707.4
#[tokio::test]
async fn ledger_refusals_change_no_state() {
    // A provisioned, non-system ledger writer: the hidden commit stays hidden.
    let h = Harness::with_isolation(reader_isolation("ledger-writer", &["repo-index"])).await;
    index_commits(&h).await;
    let pins = pins(&h, TENANT);

    // The shared high-entropy name: no merge, no plan, a typed refusal.
    let random = "q8Zx7vK2mP9wL4tR1nB6cY3hJ5dG0sFa";
    let named = vec![
        feature("f-a", "ext", random, "eg:cap/limits", &[]),
        feature("f-b", "local", random, "eg:cap/limits", &[]),
    ];
    let refused = decide(&h, request(&pins.0, named, Some("local")))
        .await
        .unwrap_err();
    assert!(
        refused.starts_with("MATCH_HIGH_ENTROPY_NAME_ONLY"),
        "{refused}"
    );

    // An entry planned and waiting for its change.
    let planned_op = FeatureLedgerOp::Plan {
        plan: medulla_decision_types::decision::feature_match::PlanProposal {
            plan_id: "plan-1".into(),
            feature_ids: vec!["f-e2".into()],
            rationale_digest: "d".into(),
        },
        expected_revision: 0,
    };
    ledger(
        &h,
        TENANT,
        FeatureLedgerOp::Open {
            feature_id: "f-e2".into(),
            match_ids: vec![],
        },
    )
    .await
    .unwrap();
    ledger(&h, TENANT, planned_op).await.unwrap();
    let before = ledger(
        &h,
        TENANT,
        FeatureLedgerOp::Get {
            feature_ids: vec![],
        },
    )
    .await
    .unwrap();

    let close = |closed_by: &str, revision: u64| FeatureLedgerOp::Close {
        feature_id: "f-e2".into(),
        closed_by: closed_by.into(),
        expected_revision: revision,
    };
    for closed_by in ["change-unknown", HIDDEN_COMMIT] {
        let error = ledger(&h, TENANT, close(closed_by, 1)).await.unwrap_err();
        assert!(error.starts_with("LEDGER_CLOSE_NO_CHANGE"), "{error}");
    }
    let stale = ledger(&h, TENANT, close(COMMIT, 9)).await.unwrap_err();
    assert!(stale.starts_with("LEDGER_REVISION_CONFLICT"), "{stale}");

    let after = ledger(
        &h,
        TENANT,
        FeatureLedgerOp::Get {
            feature_ids: vec![],
        },
    )
    .await
    .unwrap();
    assert_eq!(before, after, "a refusal changes no state");
}

// spec: MEDULLA-DECISION-ENGINE-R707.5
#[tokio::test]
async fn ledger_rows_cite_matches_and_plan() {
    let h = Harness::with_isolation(reader_isolation("ledger-writer", &["repo-index"])).await;
    index_commits(&h).await;
    let pins = pins(&h, TENANT);

    // The record cites each match with its three similarity parts.
    let batch = decide(&h, request(&pins.0, features(), Some("local")))
        .await
        .unwrap();
    let record = batch.records.as_slice()[0].clone();
    let cited = record
        .premises
        .iter()
        .find(|p| p.fact.starts_with("feature_match:match=f-e1/f-l1;"))
        .expect("a claim per match");
    assert_eq!(
        cited.class,
        medulla_decision_types::decision::PremiseClass::Claim
    );
    for part in ["token_ppm=", "label_ppm=", "embedding_ppm="] {
        assert!(cited.fact.contains(part), "{}", cited.fact);
    }
    // The label is shared and the declared cosine is one: those parts are full.
    assert!(cited.fact.contains("label_ppm=1000000"), "{}", cited.fact);
    assert!(
        cited.fact.contains("embedding_ppm=1000000"),
        "{}",
        cited.fact
    );
    let plan = record
        .answer
        .as_ref()
        .and_then(|a| a.feature_match())
        .expect("a feature comparison")
        .plans[0]
        .clone();

    // The ledger stores the match ids and the plan id on the entry.
    let match_id = match_option_id("f-e1", "f-l1");
    only(
        ledger(
            &h,
            TENANT,
            FeatureLedgerOp::Open {
                feature_id: "f-e2".to_string(),
                match_ids: vec![match_id.clone()],
            },
        )
        .await
        .unwrap(),
    );
    ledger(
        &h,
        TENANT,
        FeatureLedgerOp::Plan {
            plan: plan.clone(),
            expected_revision: 0,
        },
    )
    .await
    .unwrap();

    // A read-only principal reads the entry with its match ids and plan id.
    let reader = VerifiedRequestContext::verified_for_test_in_tenant("ledger-reader", TENANT);
    let read: FeatureLedgerView = decode(
        handle_feature_ledger(
            &h.state,
            4,
            &reader,
            FeatureLedgerOp::Get {
                feature_ids: vec!["f-e2".to_string()],
            },
        )
        .await,
    )
    .unwrap();
    let entry = only(read);
    assert_eq!(entry.match_ids, vec![match_id]);
    assert_eq!(entry.plan_id.as_deref(), Some(plan.plan_id.as_str()));

    // Another tenant finds nothing.
    let foreign = ledger(
        &h,
        OTHER_TENANT,
        FeatureLedgerOp::Get {
            feature_ids: vec!["f-e2".to_string()],
        },
    )
    .await
    .unwrap();
    assert!(foreign.entries.is_empty());
}

/// Verifies MEDULLA-DECISION-ENGINE-R707.6: the hot-path journey. Two feature
/// sets are matched, the unmatched feature is opened and planned on the ledger,
/// a close without a known change is refused and leaves the entry planned, and
/// a close naming a change the caller sees lands.
// spec: MEDULLA-DECISION-ENGINE-R707.6
#[tokio::test]
async fn closeout_requires_linked_change() {
    let h = Harness::with_isolation(reader_isolation("ledger-writer", &["repo-index"])).await;
    index_commits(&h).await;
    let pins = pins(&h, TENANT);

    let batch = decide(&h, request(&pins.0, features(), Some("local")))
        .await
        .unwrap();
    let record = batch.records.as_slice()[0].clone();
    let answer = record
        .answer
        .as_ref()
        .and_then(|a| a.feature_match())
        .expect("the family answers a feature comparison");
    assert_eq!(answer.matches.len(), 1);
    assert_eq!(answer.plans.len(), 1);
    let plan = answer.plans[0].clone();
    assert_eq!(plan.feature_ids, vec!["f-e2".to_string()]);

    let opened = only(
        ledger(
            &h,
            TENANT,
            FeatureLedgerOp::Open {
                feature_id: "f-e2".into(),
                match_ids: vec![match_option_id("f-e1", "f-l1")],
            },
        )
        .await
        .unwrap(),
    );
    assert_eq!(opened.status, LedgerStatus::Open);
    let planned = only(
        ledger(
            &h,
            TENANT,
            FeatureLedgerOp::Plan {
                plan: plan.clone(),
                expected_revision: 0,
            },
        )
        .await
        .unwrap(),
    );
    assert_eq!(planned.status, LedgerStatus::Planned);

    // A close naming an absent change, then one only another agent may see,
    // is refused and leaves the entry planned.
    for change in ["f".repeat(40), HIDDEN_COMMIT.to_string()] {
        let refused = ledger(
            &h,
            TENANT,
            FeatureLedgerOp::Close {
                feature_id: "f-e2".into(),
                closed_by: change,
                expected_revision: 1,
            },
        )
        .await
        .unwrap_err();
        assert!(refused.starts_with("LEDGER_CLOSE_NO_CHANGE"), "{refused}");
        let still = ledger(
            &h,
            TENANT,
            FeatureLedgerOp::Get {
                feature_ids: vec!["f-e2".into()],
            },
        )
        .await
        .unwrap();
        assert_eq!(only(still).status, LedgerStatus::Planned);
    }

    // Another tenant sees none of it.
    let foreign = ledger(
        &h,
        OTHER_TENANT,
        FeatureLedgerOp::Get {
            feature_ids: vec![],
        },
    )
    .await
    .unwrap();
    assert!(foreign.entries.is_empty());

    // The close naming a known change lands.
    let closed = only(
        ledger(
            &h,
            TENANT,
            FeatureLedgerOp::Close {
                feature_id: "f-e2".into(),
                closed_by: COMMIT.into(),
                expected_revision: 1,
            },
        )
        .await
        .unwrap(),
    );
    assert_eq!(closed.status, LedgerStatus::Closed);
    assert_eq!(closed.closed_by.as_deref(), Some(COMMIT));
}
