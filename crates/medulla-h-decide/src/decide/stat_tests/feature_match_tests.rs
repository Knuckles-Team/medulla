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
    let h = Harness::new().await;
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
    let StatisticalOutcome::Advisory { scores, calibrated } = &record.outcome else {
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
    assert!(error.starts_with("COMPONENT_PIN_MISMATCH"), "{error}");

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
            closed_by: "change-1".into(),
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
    assert!(stale.starts_with("REVISION_CONFLICT"), "{stale}");
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
            closed_by: "change-1".into(),
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
                closed_by: "change-1".into(),
                expected_revision: 1,
            },
        )
        .await
        .unwrap(),
    );
    assert_eq!(closed.status, LedgerStatus::Closed);
    assert_eq!(closed.closed_by.as_deref(), Some("change-1"));
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
