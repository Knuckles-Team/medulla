//! MEDULLA-DECISION-ENGINE-R818: a served `probe_selection` question reads the
//! caller tenant's red-team attacks and transforms and answers the identifiers
//! to run for a stated goal. Attacks whose category matches come first, ranked
//! by how much of the goal they name; transforms follow. A goal that selects no
//! attack refuses with `NO_CATALOG_ENTRY_SELECTED`, and another tenant's
//! catalog is never read.

use super::*;
use medulla_decision_types::decision::ResolutionKind;

const OTHER: &str = "tenant-other";

fn publish(h: &Harness, tenant: &str, id: &str, kind: AgentComponentKind, attrs: &[(&str, &str)]) {
    let mut draft = test_component_draft(tenant, id);
    draft.kind = kind;
    draft.attributes = attrs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    h.commit_in(tenant, draft, None).unwrap();
}

fn publish_catalog(h: &Harness, tenant: &str) {
    use AgentComponentKind as K;
    for (id, category, goal) in [
        ("a-leak", "injection", "leak the system prompt"),
        ("a-exfil", "injection", "exfiltrate stored secrets"),
        ("a-bypass", "jailbreak", "bypass the safety rules"),
    ] {
        publish(
            h,
            tenant,
            id,
            K::RedTeamAttack,
            &[("category", category), ("goal", goal)],
        );
    }
    for (id, kind) in [("t-base64", "base64"), ("t-leak", "leak encoding")] {
        publish(h, tenant, id, K::RedTeamTransform, &[("kind", kind)]);
    }
}

fn selection_request(
    tenant: &str,
    pin: &ComponentDependency,
    goal: &str,
    categories: Option<&[&str]>,
) -> DecideRequest {
    let mut params = vec![TypedParam {
        name: "goal_text".into(),
        value: TypedValue::Text(goal.into()),
    }];
    if let Some(categories) = categories {
        params.push(TypedParam {
            name: "target_categories".into(),
            value: TypedValue::IriList(
                BoundedVec::new(categories.iter().map(|c| (*c).to_string()).collect()).unwrap(),
            ),
        });
    }
    DecideRequest {
        tenant_id: tenant.to_string(),
        question: StatisticalQuestion {
            question_id: "probe.selection".into(),
            kind: QuestionKind::ProbeSelection,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::AgentLibrary {
            scope: LibraryCandidateScope {
                kinds: BoundedVec::new(vec![
                    AgentComponentKind::RedTeamAttack,
                    AgentComponentKind::RedTeamTransform,
                ])
                .unwrap(),
                classification_under: None,
            },
        },
        feature_schema: pin.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(params).unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

/// The listed identifiers, after asserting each scores 1.0.
fn listed(record: &StatisticalDecisionRecord) -> Vec<String> {
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("an advisory outcome, got {:?}", record.outcome);
    };
    assert!(scores.iter().all(|s| s.score.value == 1i64 << 32));
    scores.iter().map(|s| s.option_id.clone()).collect()
}

// spec: MEDULLA-DECISION-ENGINE-R818
#[tokio::test]
async fn selects_declared_entries_and_refuses_empty() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    publish_catalog(&h, TENANT);

    // Injection attacks only, the one naming the goal first, then transforms
    // by the same order.
    let batch = decide(
        &h,
        selection_request(TENANT, &pin, "leak system prompt", Some(&["injection"])),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(
        listed(record),
        ["a-leak", "a-exfil", "t-leak", "t-base64"].map(String::from)
    );
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(
        medulla_decision_types::decision::digest::statistical_record_digest(record),
        record.record_digest
    );

    // No category named: every attack is legal.
    let batch = decide(
        &h,
        selection_request(TENANT, &pin, "leak system prompt", None),
    )
    .await
    .unwrap();
    assert_eq!(
        listed(&batch.records.as_slice()[0]),
        ["a-leak", "a-bypass", "a-exfil", "t-leak", "t-base64"].map(String::from)
    );

    // A category no attack holds selects nothing.
    let error = decide(
        &h,
        selection_request(TENANT, &pin, "leak system prompt", Some(&["phishing"])),
    )
    .await
    .unwrap_err();
    assert!(error.contains("NO_CATALOG_ENTRY_SELECTED"), "{error}");

    // A blank goal is an invalid request, not an empty selection.
    let error = decide(&h, selection_request(TENANT, &pin, "  ", None))
        .await
        .unwrap_err();
    assert!(!error.contains("NO_CATALOG_ENTRY_SELECTED"), "{error}");

    another_tenants_catalog_is_never_read(&h).await;
}

/// Tenant B sees only its own catalog; a tenant with none refuses by name.
async fn another_tenants_catalog_is_never_read(h: &Harness) {
    // Tenant B holds one attack of its own and none of A's.
    let other_pin = publish_route_schema_in(h, OTHER);
    publish(
        h,
        OTHER,
        "b-only",
        AgentComponentKind::RedTeamAttack,
        &[
            ("category", "injection"),
            ("goal", "leak the system prompt"),
        ],
    );
    let ctx = VerifiedRequestContext::verified_for_test_in_tenant("decider", OTHER);
    let batch: DecisionBatch = decode(
        super::super::statistical::handle_decide(
            &h.state,
            1,
            &ctx,
            selection_request(OTHER, &other_pin, "leak system prompt", None),
        )
        .await,
    )
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.tenant_id, OTHER);
    assert_eq!(listed(record), ["b-only".to_string()]);
    // A tenant without any catalog refuses by name.
    let third = "tenant-empty";
    let third_pin = publish_route_schema_in(h, third);
    let ctx = VerifiedRequestContext::verified_for_test_in_tenant("decider", third);
    let response = super::super::statistical::handle_decide(
        &h.state,
        2,
        &ctx,
        selection_request(third, &third_pin, "leak system prompt", None),
    )
    .await;
    let error = decode::<DecisionBatch>(response).unwrap_err();
    assert!(error.contains("NO_CATALOG_ENTRY_SELECTED"), "{error}");
}
