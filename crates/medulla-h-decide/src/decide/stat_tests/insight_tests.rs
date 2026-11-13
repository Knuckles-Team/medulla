//! MEDULLA-DECISION-ENGINE-R135.12, R135.13, R135.14: run-derived insights are
//! submitted through `DecisionLog` (`RunRecord::Insight`) and selected through
//! `Decide` (`QuestionKind::InsightSelection`). `Decide` writes nothing; every
//! refusal stores nothing; another tenant's insights are never read.

use super::*;
use medulla_decision_types::decision::program_store::insight::{
    insight_key, Insight, INSIGHT_SCHEMA_VERSION,
};
use medulla_decision_types::decision::program_store::{RunRecord, RunRecordStored, StoredRun};
use medulla_decision_types::decision::{
    AbstainReason, EvidenceClass, PremiseClass, ResolutionKind,
};
use medulla_persistence_records::decision_jobs::encode_artifact;

const FOREIGN: &str = "tenant-foreign";

fn insight(id: &str, text: &str, runs: &[&str], context: &str, successes: u32) -> Insight {
    Insight {
        schema_version: INSIGHT_SCHEMA_VERSION,
        insight_id: id.into(),
        text: text.into(),
        source_run_ids: BoundedVec::new(runs.iter().map(|run| run.to_string()).collect()).unwrap(),
        context_signature: context.into(),
        success_count: successes,
        selection_revision: 4,
    }
}

async fn submit(h: &Harness, wire: serde_json::Value) -> Result<RunRecordStored, String> {
    let op = DecisionLogOp::RunRecord {
        tenant_id: TENANT.to_string(),
        record: RunRecord::Insight { insight: wire },
    };
    decode(super::log_tests::log_op(h, "recorder", op).await)
}

fn wire(insight: &Insight) -> serde_json::Value {
    serde_json::to_value(insight).unwrap()
}

fn selection_request(h: &Harness, tenant: &str, signature: Option<&str>) -> DecideRequest {
    let schema = publish_route_schema_in(h, tenant);
    let params = signature
        .map(|text| {
            vec![TypedParam {
                name: "context_signature".to_string(),
                value: TypedValue::Text(text.to_string()),
            }]
        })
        .unwrap_or_default();
    declared_request(
        &schema,
        tenant,
        QuestionKind::InsightSelection,
        "insight",
        DecisionPolicyRef::Default,
        params,
    )
}

fn select_in(
    h: &Harness,
    tenant: &str,
    signature: Option<&str>,
) -> Result<StatisticalDecisionRecord, String> {
    let request = selection_request(h, tenant, signature);
    let ctx = super::super::stat_executor::ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: tenant,
        now_ms: 1_000,
        server_secret: b"decide-stat-test-secret",
    };
    super::super::stat_decide::decide_blocking(
        &ctx,
        &request,
        None,
        "decider".to_string(),
        std::time::Instant::now(),
    )
    .map(|batch| batch.records.as_slice()[0].clone())
}

/// The insight ids an answer selects, or the abstain codes it names.
fn answered(record: &StatisticalDecisionRecord) -> Result<Vec<String>, Vec<String>> {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => Ok(scores
            .iter()
            .map(|score| {
                score
                    .option_id
                    .strip_prefix("insight:")
                    .expect("an insight answer")
                    .to_string()
            })
            .collect()),
        StatisticalOutcome::Abstained { reasons } => Err(reasons
            .iter()
            .map(|reason| match reason {
                AbstainReason::Family { code, .. } => code.as_str().to_string(),
                other => panic!("unexpected abstain reason {other:?}"),
            })
            .collect()),
        other => panic!("unexpected outcome {other:?}"),
    }
}

fn stored_rows(h: &Harness, tenant: &str) -> usize {
    h.store
        .decision_artifacts_with_prefix(tenant, "run_record/insight/", 100)
        .unwrap()
        .len()
}

async fn seeded() -> (Harness, Insight, Insight) {
    let h = Harness::new().await;
    let first = insight(
        "ins-1",
        "retry the export once before escalating",
        &["run-1", "run-2"],
        "export weekly report",
        3,
    );
    let second = insight(
        "ins-2",
        "attach the checksum to the upload",
        &["run-3"],
        "export weekly archive",
        1,
    );
    for item in [&first, &second] {
        let stored = submit(&h, wire(item)).await.unwrap();
        assert!(!stored.replayed);
        assert_eq!(stored.key, insight_key(item));
        assert_eq!(
            stored.stored,
            StoredRun::Insight {
                insight: item.clone()
            }
        );
    }
    (h, first, second)
}

// spec: MEDULLA-DECISION-ENGINE-R135.12
#[tokio::test]
async fn served_selection_returns_insights_with_source_runs() {
    let (h, first, second) = seeded().await;
    assert_eq!(stored_rows(&h, TENANT), 2);

    // The served `Decide` selects both stored insights, strongest context match first, and
    // writes nothing.
    let record = select_in(&h, TENANT, Some("export weekly report")).unwrap();
    assert_eq!(
        answered(&record),
        Ok(vec![first.insight_id.clone(), second.insight_id.clone()])
    );
    assert_eq!(record.question.kind, QuestionKind::InsightSelection);
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(record.evidence_class, EvidenceClass::Observation);
    assert_eq!(stored_rows(&h, TENANT), 2, "Decide wrote nothing");

    // Each selected insight carries its source runs.
    let premise = |id: &str| {
        record
            .premises
            .iter()
            .find(|p| p.fact.starts_with(&format!("insight:id={id};")))
            .unwrap_or_else(|| panic!("premise of {id} is missing"))
            .fact
            .clone()
    };
    assert!(premise("ins-1").contains(";sources=run-1,run-2;"));
    assert!(premise("ins-2").contains(";sources=run-3;"));

    // A retry with equal content answers the stored row; other content under the key refuses.
    let again = submit(&h, wire(&first)).await.unwrap();
    assert!(again.replayed);
    let mut changed = first.clone();
    changed.text = "escalate immediately".into();
    let conflict = submit(&h, wire(&changed)).await.unwrap_err();
    assert!(conflict.starts_with("IDEMPOTENCY_CONFLICT"), "{conflict}");
    assert_eq!(stored_rows(&h, TENANT), 2);

    // Another tenant's insights are never read: the same signature selects none there, and a
    // row stored for that tenant never answers this tenant.
    assert_eq!(
        answered(&select_in(&h, FOREIGN, Some("export weekly report")).unwrap()),
        Err(vec!["UNKNOWN_CONTEXT_SIGNATURE".to_string()])
    );
    let foreign = insight(
        "ins-f",
        "foreign only",
        &["run-f"],
        "foreign only context",
        1,
    );
    h.store
        .put_decision_artifacts(
            FOREIGN,
            &[(insight_key(&foreign), encode_artifact(&foreign).unwrap())],
        )
        .unwrap();
    assert_eq!(
        answered(&select_in(&h, TENANT, Some("foreign only context")).unwrap()),
        Err(vec!["UNKNOWN_CONTEXT_SIGNATURE".to_string()])
    );
}

// spec: MEDULLA-DECISION-ENGINE-R135.13
#[tokio::test]
async fn insight_refusals_store_nothing() {
    let h = Harness::new().await;

    // A submission with no source run refuses MISSING_SOURCE_RUN, however it is empty.
    let item = insight("ins-1", "retry once", &["run-1"], "export weekly report", 1);
    let mut empty = wire(&item);
    empty["source_run_ids"] = serde_json::json!([]);
    let mut absent = wire(&item);
    absent.as_object_mut().unwrap().remove("source_run_ids");
    for refused in [empty, absent] {
        let error = submit(&h, refused).await.unwrap_err();
        assert!(error.starts_with("MISSING_SOURCE_RUN"), "{error}");
        assert_eq!(stored_rows(&h, TENANT), 0);
    }

    // An empty signature and a signature that shares no token with any stored insight abstain
    // UNKNOWN_CONTEXT_SIGNATURE and store nothing.
    submit(&h, wire(&item)).await.unwrap();
    assert_eq!(stored_rows(&h, TENANT), 1);
    for signature in [Some(""), Some("   "), Some("unrelated tokens"), None] {
        let record = select_in(&h, TENANT, signature).unwrap();
        assert_eq!(
            answered(&record),
            Err(vec!["UNKNOWN_CONTEXT_SIGNATURE".to_string()]),
            "{signature:?}"
        );
        assert_eq!(stored_rows(&h, TENANT), 1);
    }

    // The signature stored for this tenant reads as unknown to another tenant.
    assert_eq!(
        answered(&select_in(&h, FOREIGN, Some("export weekly report")).unwrap()),
        Err(vec!["UNKNOWN_CONTEXT_SIGNATURE".to_string()])
    );
    assert_eq!(stored_rows(&h, FOREIGN), 0);
}

// spec: MEDULLA-DECISION-ENGINE-R135.14
#[tokio::test]
async fn selection_premises_link_source_runs() {
    let (h, first, _second) = seeded().await;
    let record = select_in(&h, TENANT, Some("export weekly report")).unwrap();

    // One Observation premise per selected insight: id, source runs, the exact Q32 match and
    // the selection revision; the knob premises of the policy follow.
    let found = record
        .premises
        .iter()
        .find(|p| p.fact.starts_with("insight:id=ins-1;"))
        .expect("the premise of ins-1");
    assert_eq!(found.class, PremiseClass::Observation);
    let expected_match = 1_i64 << 32; // identical signatures match exactly
    assert_eq!(
        found.fact,
        format!(
            "insight:id=ins-1;sources=run-1,run-2;match_q32={expected_match};selection_revision={}",
            first.selection_revision
        )
    );
    assert_eq!(
        record
            .premises
            .iter()
            .filter(|p| p.fact.starts_with("insight:id="))
            .count(),
        2
    );
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact.starts_with("knob:insight.min_match=")));
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact.starts_with("knob:insight.max_selected=")));

    // The record digest recomputes from the sealed record.
    assert_eq!(
        medulla_decision_types::decision::digest::statistical_record_digest(&record),
        record.record_digest
    );

    // Another tenant's record states no insight premise, and the record still seals.
    let foreign = select_in(&h, FOREIGN, Some("export weekly report")).unwrap();
    assert!(foreign
        .premises
        .iter()
        .all(|p| !p.fact.starts_with("insight:id=")));
    assert_eq!(
        medulla_decision_types::decision::digest::statistical_record_digest(&foreign),
        foreign.record_digest
    );
}
