//! Temporal basis served through `Decide` (MEDULLA-DECISION-ENGINE-R146.3,
//! R146.4, R146.5): three clocks disagree, the first present basis is chosen
//! and a widened fallback window is named when the first pass found too few rows.

use super::*;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::temporal_query::{
    TemporalContext, TemporalQueryInput, REFUSALS, TEMPORAL_QUERY_SCHEMA_VERSION,
};
use medulla_decision_types::decision::{PremiseClass, ResolutionKind};
use medulla_types::contract::Digest256;

const DAY: i64 = 86_400_000;

fn option(id: &str) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::default(),
        texts: BoundedVec::default(),
    }
}

fn input(rows: u32, context: TemporalContext) -> TemporalQueryInput {
    TemporalQueryInput {
        schema_version: TEMPORAL_QUERY_SCHEMA_VERSION,
        query_digest: Digest256::sha256(b"when did the audit happen"),
        window_from_ms: 100 * DAY,
        window_to_ms: 110 * DAY,
        first_pass_rows: rows,
        context,
    }
}

fn request(
    schema: &ComponentDependency,
    ids: &[&str],
    query: Option<TemporalQueryInput>,
) -> DecideRequest {
    let mut options: Vec<DeclaredOption> = ids.iter().map(|id| option(id)).collect();
    options.sort_by(|a, b| a.option_id.cmp(&b.option_id));
    let params: Vec<TypedParam> = query
        .map(|q| TypedParam {
            name: "temporal_query".to_string(),
            value: TypedValue::TemporalQuery(q),
        })
        .into_iter()
        .collect();
    DecideRequest {
        tenant_id: TENANT.into(),
        question: StatisticalQuestion {
            question_id: "time.basis".to_string(),
            kind: QuestionKind::TemporalBasis,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(options).unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(params).unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

const BASES: [&str; 3] = ["event", "narrative", "storage"];

fn confused() -> TemporalContext {
    TemporalContext {
        storage_ms: 120 * DAY,
        event_ms: Some(90 * DAY),
        narrative_ms: Some(105 * DAY),
    }
}

fn chosen(record: &StatisticalDecisionRecord) -> String {
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("a basis answers advisory, got {:?}", record.outcome);
    };
    assert_eq!(scores.as_slice().len(), 1, "one basis is chosen");
    scores.as_slice()[0].option_id.clone()
}

fn fact_list(record: &StatisticalDecisionRecord) -> Vec<&str> {
    record.premises.iter().map(|p| p.fact.as_str()).collect()
}

/// Verifies MEDULLA-DECISION-ENGINE-R146.3: three clocks that disagree, a first
/// pass of two rows and a window select the event basis and build the fallback.
// spec: MEDULLA-DECISION-ENGINE-R146.3
#[tokio::test]
async fn three_clock_confusion_selects_a_basis_and_builds_a_fallback() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    let batch = decide(&h, request(&schema, &BASES, Some(input(2, confused()))))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::TemporalBasis);
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    assert_eq!(chosen(record), "event");
    let facts = fact_list(record);
    assert!(facts.contains(&"temporal:basis=event"));
    assert!(facts.contains(&"temporal:confusion=true"));
    assert!(facts.contains(&"temporal:fallback_built=true"));
    // The default widen is 7 days each side of the 100..110 day window.
    let from = format!("temporal:fallback_from_ms={}", 93 * DAY);
    let to = format!("temporal:fallback_to_ms={}", 117 * DAY);
    assert!(facts.contains(&from.as_str()), "{facts:?}");
    assert!(facts.contains(&to.as_str()), "{facts:?}");

    // Without an event date the narrative clock is the first present basis.
    let mut context = confused();
    context.event_ms = None;
    let batch = decide(&h, request(&schema, &BASES, Some(input(2, context))))
        .await
        .unwrap();
    assert_eq!(chosen(&batch.records.as_slice()[0]), "narrative");

    // A first pass with enough rows builds no fallback window.
    let batch = decide(&h, request(&schema, &BASES, Some(input(3, confused()))))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let facts = fact_list(record);
    assert!(facts.contains(&"temporal:fallback_built=false"));
    assert!(!facts
        .iter()
        .any(|f| f.starts_with("temporal:fallback_from_ms")));

    // A foreign tenant cannot reach this tenant's pinned schema.
    let mut foreign = request(&schema, &BASES, Some(input(2, confused())));
    foreign.tenant_id = "tenant-other".to_string();
    let other = VerifiedRequestContext::verified_for_test_in_tenant("decider", "tenant-other");
    let error = decode::<DecisionBatch>(
        super::super::statistical::handle_decide(&h.state, 1, &other, foreign).await,
    )
    .unwrap_err();
    assert!(error.starts_with("COMPONENT_PIN_MISMATCH"), "{error}");
}

fn missing_detail(reason: &medulla_decision_types::decision::AbstainReason) -> String {
    match reason {
        medulla_decision_types::decision::AbstainReason::Family { detail, .. } => detail.clone(),
        other => panic!("expected a family abstention, got {other:?}"),
    }
}

/// Verifies MEDULLA-DECISION-ENGINE-R146.4: each refusal code, and the
/// abstention for missing context.
// spec: MEDULLA-DECISION-ENGINE-R146.4
#[tokio::test]
async fn missing_time_refuses() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    // No temporal_query parameter abstains, naming the missing field.
    let batch = decide(&h, request(&schema, &BASES, None)).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.resolution_kind, ResolutionKind::Abstention);
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("missing context abstains, got {:?}", record.outcome);
    };
    assert_eq!(
        reasons.as_slice()[0].code().as_str(),
        "TEMPORAL_CONTEXT_MISSING"
    );
    assert!(missing_detail(&reasons.as_slice()[0]).contains("params.temporal_query"));

    // A context with no usable date abstains the same way.
    let blank = TemporalContext {
        storage_ms: 0,
        event_ms: None,
        narrative_ms: None,
    };
    let batch = decide(&h, request(&schema, &BASES, Some(input(0, blank))))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.resolution_kind, ResolutionKind::Abstention);
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("an empty context abstains, got {:?}", record.outcome);
    };
    assert_eq!(
        reasons.as_slice()[0].code().as_str(),
        "TEMPORAL_CONTEXT_MISSING"
    );
    assert!(missing_detail(&reasons.as_slice()[0]).contains("temporal_query.context"));

    // An inverted window refuses before any fallback window is built.
    let mut inverted = input(0, confused());
    inverted.window_from_ms = 120 * DAY;
    inverted.window_to_ms = 110 * DAY;
    let error = decide(&h, request(&schema, &BASES, Some(inverted)))
        .await
        .unwrap_err();
    assert!(error.starts_with("TEMPORAL_WINDOW_INVALID"), "{error}");

    // Options that are not the three bases cannot be mapped.
    let error = decide(
        &h,
        request(&schema, &["event", "storage"], Some(input(0, confused()))),
    )
    .await
    .unwrap_err();
    assert!(error.starts_with("TEMPORAL_BASIS_UNMAPPABLE"), "{error}");
    let error = decide(
        &h,
        request(
            &schema,
            &["event", "narrative", "storage", "wall"],
            Some(input(0, confused())),
        ),
    )
    .await
    .unwrap_err();
    assert!(error.starts_with("TEMPORAL_BASIS_UNMAPPABLE"), "{error}");

    // All three codes are registered.
    for code in REFUSALS {
        assert_eq!(
            medulla_decision_types::decision::codes::resolve(code.as_str()),
            Some(*code)
        );
    }

    // A foreign tenant cannot reach this tenant's pinned schema.
    let mut foreign = request(&schema, &BASES, None);
    foreign.tenant_id = "tenant-other".to_string();
    let other = VerifiedRequestContext::verified_for_test_in_tenant("decider", "tenant-other");
    let error = decode::<DecisionBatch>(
        super::super::statistical::handle_decide(&h.state, 1, &other, foreign).await,
    )
    .unwrap_err();
    assert!(error.starts_with("COMPONENT_PIN_MISMATCH"), "{error}");
}

/// Verifies MEDULLA-DECISION-ENGINE-R146.5: every evidence premise for a
/// confused and an unconfused input.
// spec: MEDULLA-DECISION-ENGINE-R146.5
#[tokio::test]
async fn temporal_confusion_evidence() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    let digest = Digest256::sha256(b"when did the audit happen").to_hex();
    let batch = decide(&h, request(&schema, &BASES, Some(input(1, confused()))))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let facts = fact_list(record);
    for expected in [
        format!("temporal:query={digest}"),
        "temporal:basis=event".to_string(),
        "temporal:confusion=true".to_string(),
        "temporal:first_pass_rows=1".to_string(),
        "temporal:fallback_built=true".to_string(),
        "knob:temporal.confusion_gap=1".to_string(),
        "knob:temporal.min_hits=3".to_string(),
        "knob:temporal.widen=7".to_string(),
        "knob:temporal.max_retries=1".to_string(),
    ] {
        assert!(
            facts.contains(&expected.as_str()),
            "{expected} in {facts:?}"
        );
    }
    // Each caller date is a claim; the knobs are definitions.
    assert!(record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("temporal:"))
        .all(|p| p.class == PremiseClass::Claim));
    assert!(record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("knob:temporal."))
        .all(|p| p.class == PremiseClass::Definition));

    // Dates a day apart are no confusion.
    let calm = TemporalContext {
        storage_ms: 100 * DAY,
        event_ms: Some(100 * DAY),
        narrative_ms: Some(100 * DAY + DAY / 2),
    };
    let batch = decide(&h, request(&schema, &BASES, Some(input(5, calm))))
        .await
        .unwrap();
    let facts = fact_list(&batch.records.as_slice()[0]);
    assert!(facts.contains(&"temporal:confusion=false"));
    assert!(facts.contains(&"temporal:fallback_built=false"));
    assert!(facts.contains(&"temporal:first_pass_rows=5"));

    // A foreign tenant cannot reach this tenant's pinned schema.
    let mut foreign = request(&schema, &BASES, Some(input(1, confused())));
    foreign.tenant_id = "tenant-other".to_string();
    let other = VerifiedRequestContext::verified_for_test_in_tenant("decider", "tenant-other");
    let error = decode::<DecisionBatch>(
        super::super::statistical::handle_decide(&h.state, 1, &other, foreign).await,
    )
    .unwrap_err();
    assert!(error.starts_with("COMPONENT_PIN_MISMATCH"), "{error}");
}
