//! Changeset review served through `Decide` (MEDULLA-DECISION-ENGINE-R138.101.2):
//! findings in the five flaw classes enter as declared options, the impact
//! filter drops only the doubted low findings, and a kept finding at or above
//! the blocking severity blocks the change.

use super::*;
use medulla_decision_types::decision::changeset_review::{changed_files_digest, REFUSALS};
use medulla_decision_types::decision::statistical::declared::{
    DeclaredNumber, DeclaredOption, DeclaredText,
};
use medulla_decision_types::decision::DecisionPolicy;

const PATHS: [&str; 3] = ["src/b.rs", "src/a.rs", "src/c.rs"];

struct Finding {
    id: &'static str,
    class: &'static str,
    severity: &'static str,
    path: &'static str,
    confidence: i64,
    doubt: Option<&'static str>,
}

fn option(f: &Finding) -> DeclaredOption {
    let number = |key: &str, value: i64| DeclaredNumber {
        key: key.to_string(),
        q32: value << 32,
    };
    let text = |key: &str, text: &str| DeclaredText {
        key: key.to_string(),
        text: text.to_string(),
    };
    let mut texts = vec![
        text("evidence", "user input reaches the sink unchecked"),
        text("path", f.path),
    ];
    if let Some(doubt) = f.doubt {
        texts.insert(1, text("false_positive_evidence", doubt));
    }
    DeclaredOption {
        option_id: f.id.to_string(),
        classification: BoundedVec::new(vec![
            format!("review:class:{}", f.class),
            format!("review:severity:{}", f.severity),
        ])
        .unwrap(),
        numbers: BoundedVec::new(vec![
            number("confidence_permille", f.confidence),
            number("line_end", 12),
            number("line_start", 10),
        ])
        .unwrap(),
        texts: BoundedVec::new(texts).unwrap(),
    }
}

fn finding(id: &'static str, class: &'static str, severity: &'static str) -> Finding {
    Finding {
        id,
        class,
        severity,
        path: "src/a.rs",
        confidence: 900,
        doubt: None,
    }
}

fn iris(values: &[&str]) -> TypedValue {
    TypedValue::IriList(BoundedVec::new(values.iter().map(|v| v.to_string()).collect()).unwrap())
}

fn review_request(schema: &ComponentDependency, findings: &[Finding]) -> DecideRequest {
    let mut sorted: Vec<DeclaredOption> = findings.iter().map(option).collect();
    sorted.sort_by(|a, b| a.option_id.cmp(&b.option_id));
    let text = |name: &str, value: &str| TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    };
    DecideRequest {
        tenant_id: TENANT.into(),
        question: StatisticalQuestion {
            question_id: "review.changeset".to_string(),
            kind: QuestionKind::ChangesetReview,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(sorted).unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![
            text("base_rev", "rev-base"),
            text("head_rev", "rev-head"),
            text("changed_files_digest", &changed_files_digest(PATHS)),
            TypedParam {
                name: "changed_paths".to_string(),
                value: iris(&PATHS),
            },
            TypedParam {
                name: "classes_reviewed".to_string(),
                value: iris(&[
                    "review:class:injection",
                    "review:class:auth_bypass",
                    "review:class:data_exposure",
                    "review:class:crypto_flaw",
                    "review:class:business_logic",
                ]),
            },
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn verdict(record: &StatisticalDecisionRecord) -> String {
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("a review answers advisory, got {:?}", record.outcome);
    };
    assert_eq!(scores.as_slice().len(), 1, "one option, pass or block");
    scores.as_slice()[0].option_id.clone()
}

fn facts(record: &StatisticalDecisionRecord) -> Vec<&str> {
    record.premises.iter().map(|p| p.fact.as_str()).collect()
}

fn five() -> Vec<Finding> {
    let mut doubted = finding("f1", "injection", "low");
    doubted.confidence = 100;
    doubted.doubt = Some("test fixture, never reachable");
    vec![
        doubted,
        finding("f2", "auth_bypass", "medium"),
        finding("f3", "data_exposure", "low"),
        finding("f4", "crypto_flaw", "medium"),
        finding("f5", "business_logic", "high"),
    ]
}

/// Verifies MEDULLA-DECISION-ENGINE-R138.101.2: one finding per class, a
/// filtered low-impact false positive, a blocked high-impact finding.
// spec: MEDULLA-DECISION-ENGINE-R138.101.2
#[tokio::test]
async fn review_covers_five_classes_and_filters_by_impact() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    let batch = decide(&h, review_request(&schema, &five())).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::ChangesetReview);
    assert_eq!(verdict(record), "block");
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let facts = facts(record);
    assert!(facts.contains(&"review.finding:f1=filtered_low_impact"));
    for kept in ["f2", "f3", "f4", "f5"] {
        assert!(
            facts.contains(&format!("review.finding:{kept}=kept").as_str()),
            "{kept} stays kept"
        );
    }
    assert!(facts.contains(&"knob:review.block_severity=2"));
    assert!(facts.contains(&"knob:review.filter_confidence_max=300"));
    assert!(record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("review.finding:"))
        .all(|p| p.class == medulla_decision_types::decision::PremiseClass::Claim));

    // Without the high finding nothing reaches the blocking severity.
    let mut passing = five();
    passing.pop();
    let batch = decide(&h, review_request(&schema, &passing)).await.unwrap();
    assert_eq!(verdict(&batch.records.as_slice()[0]), "pass");

    // A doubted low finding alone is filtered, and a loud low one is kept.
    let mut loud = finding("f1", "injection", "low");
    loud.doubt = Some("test fixture");
    loud.confidence = 300;
    let batch = decide(&h, review_request(&schema, &[loud])).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(verdict(record), "pass");
    assert!(facts_of(record).contains(&"review.finding:f1=kept"));

    assert_refusals_and_tenant_isolation(&h, &schema).await;

    // A pinned policy can lower nothing: the blocking severity stays fixed.
    assert_eq!(
        medulla_decision_stats::decision::changeset_review::block_severity(
            &DecisionPolicy::engine_default()
        ),
        medulla_decision_types::decision::changeset_review::Severity::High
    );
}

fn facts_of(record: &StatisticalDecisionRecord) -> Vec<&str> {
    facts(record)
}

/// Each of the six refusal codes, and a foreign tenant.
async fn assert_refusals_and_tenant_isolation(h: &Harness, schema: &ComponentDependency) {
    let refused = |request: DecideRequest| async move { decide(h, request).await.unwrap_err() };
    let set_text = |request: &mut DecideRequest, name: &str, value: &str| {
        let params: Vec<TypedParam> = request
            .params
            .iter()
            .map(|p| {
                if p.name == name {
                    TypedParam {
                        name: name.to_string(),
                        value: TypedValue::Text(value.to_string()),
                    }
                } else {
                    p.clone()
                }
            })
            .collect();
        request.params = BoundedVec::new(params).unwrap();
    };

    // REVIEW_REVISION_MISSING
    let mut request = review_request(schema, &five());
    set_text(&mut request, "head_rev", "  ");
    let error = refused(request).await;
    assert!(error.starts_with("REVIEW_REVISION_MISSING"), "{error}");

    // REVIEW_CLASS_UNSUPPORTED: a cited class outside the five.
    let error = refused(review_request(
        &schema,
        &[finding("f1", "race_condition", "high")],
    ))
    .await;
    assert!(error.starts_with("REVIEW_CLASS_UNSUPPORTED"), "{error}");

    // REVIEW_CLASS_UNREVIEWED: the review did not claim to cover the class.
    let mut request = review_request(schema, &[finding("f1", "injection", "high")]);
    let params: Vec<TypedParam> = request
        .params
        .iter()
        .map(|p| {
            if p.name == "classes_reviewed" {
                TypedParam {
                    name: p.name.clone(),
                    value: iris(&["review:class:auth_bypass"]),
                }
            } else {
                p.clone()
            }
        })
        .collect();
    request.params = BoundedVec::new(params).unwrap();
    let error = refused(request).await;
    assert!(error.starts_with("REVIEW_CLASS_UNREVIEWED"), "{error}");

    // REVIEW_PROOF_ABSENT: a finding with no evidence text.
    let mut request = review_request(schema, &[finding("f1", "injection", "high")]);
    let CandidateSource::Declared { options } = &mut request.candidates else {
        unreachable!()
    };
    let mut bare = options.as_slice()[0].clone();
    bare.texts = BoundedVec::new(vec![DeclaredText {
        key: "path".to_string(),
        text: "src/a.rs".to_string(),
    }])
    .unwrap();
    *options = BoundedVec::new(vec![bare]).unwrap();
    let error = refused(request).await;
    assert!(error.starts_with("REVIEW_PROOF_ABSENT"), "{error}");

    // REVIEW_CHANGESET_DIGEST_MISMATCH
    let mut request = review_request(schema, &five());
    set_text(&mut request, "changed_files_digest", &"0".repeat(64));
    let error = refused(request).await;
    assert!(
        error.starts_with("REVIEW_CHANGESET_DIGEST_MISMATCH"),
        "{error}"
    );

    // REVIEW_FINDING_OUTSIDE_CHANGESET
    let mut outside = finding("f1", "injection", "high");
    outside.path = "src/untouched.rs";
    let error = refused(review_request(schema, &[outside])).await;
    assert!(
        error.starts_with("REVIEW_FINDING_OUTSIDE_CHANGESET"),
        "{error}"
    );

    // All six are registered codes.
    for code in REFUSALS {
        assert_eq!(
            medulla_decision_types::decision::codes::resolve(code.as_str()),
            Some(*code)
        );
    }

    // A foreign tenant cannot reach this tenant's pinned schema.
    let mut request = review_request(schema, &five());
    request.tenant_id = "tenant-other".to_string();
    let other = VerifiedRequestContext::verified_for_test_in_tenant("decider", "tenant-other");
    let error = decode::<DecisionBatch>(
        super::super::statistical::handle_decide(&h.state, 1, &other, request).await,
    )
    .unwrap_err();
    assert!(error.starts_with("COMPONENT_PIN_MISMATCH"), "{error}");
}
