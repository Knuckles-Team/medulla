//! `DecisionLog.harness_adoption` served (MEDULLA-DECISION-ENGINE-R803.8): the
//! adopted harness, its contrast and its lesson commit together from one
//! committed contrast decision, a repeat answers the stored refs, a second
//! adoption supersedes the first by edge, and each refusal writes nothing.

use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_decision_types::decision::statistical::log::{
    DecisionLogCommitted, DecisionLogOp, HarnessAdoptionStored, HarnessAdoptionWrite,
};
use medulla_decision_types::decision::statistical::StatisticalDecisionRecord;
use medulla_decision_types::decision::DecisionPolicyRef;
use medulla_types::contract::BoundedVec;
use medulla_work_types::orchestration::improvement::{
    AdoptedHarness, AdoptionSupersession, HarnessContrast, HarnessLesson, ImprovementLibraryRefusal,
};
use medulla_work_types::orchestration::HarnessComponentKind;

use super::harness_contrast_tests::{
    contrast_request, contrast_schema, decide_contrast, graph_view, option, BASELINE, CONDITIONS,
};
use super::log_tests::log_op;
use super::*;

const A: &str = "harness-component:prompt:a";
const B: &str = "harness-component:prompt:b";

async fn committed_contrast(h: &Harness, a_eval: f64, b_eval: f64) -> StatisticalDecisionRecord {
    let schema_pin = h
        .publish_test_component(
            &format!("schema-adoption-{a_eval}-{b_eval}"),
            AgentComponentKind::FeatureSchema,
            "contrast deltas",
            Some(&contrast_schema()),
            None,
        )
        .unwrap();
    let view = graph_view(&[A, B], &[BASELINE, "trace:a", "trace:b"]);
    let options = vec![
        option(A, "trace:a", a_eval, 1),
        option(B, "trace:b", b_eval, 1),
    ];
    let record = decide_contrast(
        h,
        &contrast_request(&schema_pin, DecisionPolicyRef::Default, options),
        &view,
    );
    let _: DecisionLogCommitted = decode(
        log_op(
            h,
            "decider",
            DecisionLogOp::Commit {
                record: Box::new(record.clone()),
                evaluator: None,
            },
        )
        .await,
    )
    .unwrap();
    record
}

fn write_for(record: &StatisticalDecisionRecord, adopted: &str) -> HarnessAdoptionWrite {
    HarnessAdoptionWrite {
        task_ref: "task:deploy".to_string(),
        task_class: "eg:taskclass/deploy".to_string(),
        agent_ref: "agent:deployer".to_string(),
        decision_record_digest: record.record_digest.clone(),
        conditions_digest: CONDITIONS.to_string(),
        adopted_version_ref: adopted.to_string(),
        deficiency: "retries".to_string(),
        situation: "a deploy that retried its tool call".to_string(),
        changed_component_kind: HarnessComponentKind::SystemPrompt,
        what_changed: "the prompt names the tool contract".to_string(),
        applicability: BoundedVec::new(vec!["tool_error".to_string()]).unwrap(),
        confidence_ppm: 800_000,
        evidence_run_ids: BoundedVec::new(vec![BASELINE.to_string(), "trace:b".to_string()])
            .unwrap(),
    }
}

async fn adopt(h: &Harness, write: HarnessAdoptionWrite) -> Result<HarnessAdoptionStored, String> {
    decode(
        log_op(
            h,
            "decider",
            DecisionLogOp::HarnessAdoption {
                tenant_id: TENANT.to_string(),
                write,
            },
        )
        .await,
    )
}

fn rows(h: &Harness, prefix: &str) -> usize {
    h.store
        .decision_artifacts_with_prefix(TENANT, prefix, 1_000)
        .unwrap()
        .len()
}

fn counts(h: &Harness) -> [usize; 6] {
    [
        rows(h, "harness-adopted:"),
        rows(h, "harness-contrast:"),
        rows(h, "harness-lesson:"),
        rows(h, "harness-supersession:adoption:"),
        rows(h, "harness-supersession:lesson:"),
        rows(h, "harness-adoption-write:"),
    ]
}

/// Verifies MEDULLA-DECISION-ENGINE-R803.8: the adoption, contrast and lesson commit together.
// spec: MEDULLA-DECISION-ENGINE-R803.8
#[tokio::test]
async fn adoption_contrast_and_lesson_commit_together() {
    let h = Harness::new().await;
    let first = committed_contrast(&h, 0.05, 0.10).await;
    let write = write_for(&first, B);

    let stored = adopt(&h, write.clone()).await.unwrap();
    assert!(!stored.replayed);
    assert_eq!(stored.supersedes_ref, None);
    // One adoption, one contrast, one lesson, one head edge each, one write row.
    assert_eq!(counts(&h), [1, 1, 1, 1, 1, 1]);

    let adoption_rows = h
        .store
        .decision_artifacts_with_prefix(TENANT, "harness-adopted:", 10)
        .unwrap();
    let adoption: AdoptedHarness = decode_stored(&adoption_rows[0].1);
    assert_eq!(adoption.node_id(), stored.adopted_harness_ref);
    assert_eq!(adoption.decision_ref, first.record_id);
    assert_eq!(
        adoption.contrast_ref.as_deref(),
        Some(stored.contrast_ref.as_str())
    );
    let contrast_rows = h
        .store
        .decision_artifacts_with_prefix(TENANT, "harness-contrast:", 10)
        .unwrap();
    let contrast: HarnessContrast = decode_stored(&contrast_rows[0].1);
    assert_eq!(contrast.node_id(), stored.contrast_ref);
    assert_eq!(contrast.run_a_trace_id, BASELINE);
    assert_eq!(contrast.run_b_trace_id, "trace:b");
    let lesson_rows = h
        .store
        .decision_artifacts_with_prefix(TENANT, "harness-lesson:", 10)
        .unwrap();
    let lesson: HarnessLesson = decode_stored(&lesson_rows[0].1);
    assert_eq!(lesson.node_id(), stored.lesson_ref);
    assert_eq!(lesson.evidence_run_ids.as_slice(), [BASELINE, "trace:b"]);

    // A repeat of the same write answers the stored refs and writes nothing.
    let repeat = adopt(&h, write).await.unwrap();
    assert!(repeat.replayed);
    assert_eq!(repeat.adopted_harness_ref, stored.adopted_harness_ref);
    assert_eq!(repeat.lesson_ref, stored.lesson_ref);
    assert_eq!(counts(&h), [1, 1, 1, 1, 1, 1]);

    // A later adoption of the same agent and task class supersedes the first by
    // edge; the first stays readable.
    let second = committed_contrast(&h, 0.30, 0.10).await;
    let mut next = write_for(&second, A);
    next.evidence_run_ids =
        BoundedVec::new(vec![BASELINE.to_string(), "trace:a".to_string()]).unwrap();
    next.what_changed = "the prompt names the retry budget".to_string();
    let superseding = adopt(&h, next).await.unwrap();
    assert_eq!(
        superseding.supersedes_ref.as_deref(),
        Some(stored.adopted_harness_ref.as_str())
    );
    assert_eq!(counts(&h)[0], 2);
    let edges = h
        .store
        .decision_artifacts_with_prefix(TENANT, "harness-supersession:adoption:", 10)
        .unwrap();
    assert!(edges.iter().any(|(_, bytes)| {
        let edge: AdoptionSupersession = decode_stored(bytes);
        edge.new_version == superseding.adopted_harness_ref
            && edge.superseded_version == stored.adopted_harness_ref
    }));
}

fn decode_stored<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> T {
    medulla_persistence_records::decision_jobs::decode_artifact(bytes, "stored record").unwrap()
}

/// Verifies MEDULLA-DECISION-ENGINE-R803.8: each refusal is typed and writes nothing.
// spec: MEDULLA-DECISION-ENGINE-R803.8
#[tokio::test]
async fn adoption_refusals_are_typed_and_write_nothing() {
    let h = Harness::new().await;
    let record = committed_contrast(&h, 0.05, 0.10).await;
    let refused = |error: String, refusal: ImprovementLibraryRefusal| {
        assert!(
            error.starts_with(refusal.code()),
            "{error} should start with {}",
            refusal.code()
        );
    };

    let mut no_evidence = write_for(&record, B);
    no_evidence.evidence_run_ids = BoundedVec::default();
    refused(
        adopt(&h, no_evidence).await.unwrap_err(),
        ImprovementLibraryRefusal::LessonWithoutEvidence,
    );

    let mut other_conditions = write_for(&record, B);
    other_conditions.conditions_digest = "sha256:other".to_string();
    refused(
        adopt(&h, other_conditions).await.unwrap_err(),
        ImprovementLibraryRefusal::ContrastConditionsMismatch,
    );

    let mut unknown = write_for(&record, B);
    unknown.decision_record_digest = "sha256:not-committed".to_string();
    refused(
        adopt(&h, unknown).await.unwrap_err(),
        ImprovementLibraryRefusal::AdoptedHarnessNoDecision,
    );

    // The decision adopted B; naming A names a decision that did not adopt it.
    refused(
        adopt(&h, write_for(&record, A)).await.unwrap_err(),
        ImprovementLibraryRefusal::AdoptedHarnessNoDecision,
    );
    assert_eq!(counts(&h), [0, 0, 0, 0, 0, 0]);
}
