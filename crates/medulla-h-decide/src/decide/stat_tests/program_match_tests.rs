//! MEDULLA-DECISION-ENGINE-R135.17.1: the replay row of a stored program is committed through
//! `DecisionLog`, stored once under its key, stamped with the engine time and the committer.

use super::*;
use medulla_decision_types::decision::program_store::replay::{
    replay_key, ProgramReplay, ReplayOutcome, REPLAY_SCHEMA_VERSION,
};
use medulla_decision_types::decision::program_store::semantic::{
    match_key, phrasing_node_id, MatchKind, ProgramCandidates, ProgramMatch, ProgramMatchIndex,
    ProgramPhrasing,
};
use medulla_decision_types::decision::program_store::{
    program_node_id, RunRecord, RunRecordStored, StoredRun,
};
use medulla_decision_types::decision::run_governance::RunOutcome;
use medulla_decision_types::decision::StatisticalOutcome;
use medulla_persistence_records::decision_jobs::decode_artifact;

use medulla_decision_types::decision::program_store::TraceSource;
use super::program_store_tests::{commit_op, lookup_request, trace, Fixture, GRAPH};

fn replay(outcome: ReplayOutcome) -> ProgramReplay {
    ProgramReplay {
        schema_version: REPLAY_SCHEMA_VERSION,
        run_id: "run-7".into(),
        task_signature: "summarize-report".into(),
        decision_record_digest: "ab".repeat(32),
        outcome,
        program_digest: Some("cd".repeat(32)),
        failed_step: None,
        refusal_code: None,
    }
}

async fn commit(h: &Harness, who: &str, replay: ProgramReplay) -> Result<RunRecordStored, String> {
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(who, TENANT);
    decode::<RunRecordStored>(
        super::super::log::handle_decision_log(
            &h.state,
            9,
            &verified,
            DecisionLogOp::RunRecord {
                tenant_id: TENANT.to_string(),
                record: RunRecord::ProgramReplay { replay },
            },
        )
        .await,
    )
}

// spec: MEDULLA-DECISION-ENGINE-R135.17.1
#[tokio::test]
async fn replay_row_is_stored_once() {
    let h = Harness::new().await;
    let first = commit(&h, "decider", replay(ReplayOutcome::Hit)).await.unwrap();
    assert_eq!(first.key, replay_key("summarize-report", "run-7"));
    assert!(!first.replayed);
    let StoredRun::ProgramReplay { replay: stored } = &first.stored else {
        panic!("a replay row, got {:?}", first.stored);
    };
    assert_eq!(stored.replay, replay(ReplayOutcome::Hit));
    assert!(stored.recorded_at_ms > 0, "the engine time, not the caller's");
    assert_eq!(
        stored.principal,
        VerifiedRequestContext::verified_for_test_in_tenant("decider", TENANT)
            .principal_persistence_id()
    );
    let bytes = h.store.decision_artifact(TENANT, &first.key).unwrap();
    assert!(bytes.is_some(), "one decision artifact");

    // A retry with equal content answers the stored row.
    let again = commit(&h, "decider", replay(ReplayOutcome::Hit)).await.unwrap();
    assert!(again.replayed);
    assert_eq!(again.stored, first.stored);

    // Other content under the key refuses IDEMPOTENCY_CONFLICT.
    let mut changed = replay(ReplayOutcome::Fallback);
    changed.failed_step = Some(2);
    let refused = commit(&h, "decider", changed).await.unwrap_err();
    assert!(refused.starts_with("IDEMPOTENCY_CONFLICT"), "{refused}");

    // A broken rule refuses and stores nothing.
    let mut broken = replay(ReplayOutcome::Hit);
    broken.run_id = "run-8".into();
    broken.program_digest = None;
    let refused = commit(&h, "decider", broken).await.unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
    assert!(refused.contains("REPLAY_RECORD_INVALID"), "{refused}");
    let key = replay_key("summarize-report", "run-8");
    assert!(h.store.decision_artifact(TENANT, &key).unwrap().is_none());
}

fn run_rows(h: &Harness) -> usize {
    h.store
        .decision_artifacts_with_prefix(TENANT, "run_record/", 100)
        .unwrap()
        .len()
}

fn with_text(mut request: DecideRequest, signature: &str, text: &str) -> DecideRequest {
    let param = |name: &str, value: &str| TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    };
    request.params = BoundedVec::new(vec![
        param("task_signature", signature),
        param("request_text", text),
        param("program_graph", GRAPH),
    ])
    .unwrap();
    request
}

// spec: MEDULLA-DECISION-ENGINE-R135.16.3
#[tokio::test]
async fn commit_writes_vectors_and_lookup_reads_them() {
    let f = Fixture::new().await;
    let run = trace("run-1", RunOutcome::Succeeded, 3, "weekly");
    f.add_trace_node(&run, "recorder").await;
    let source = TraceSource {
        graph: GRAPH.to_string(),
        node_id: format!("trace:{}", run.run_id),
    };
    let committed = commit_op(
        &f,
        DecisionLogOp::RunRecord {
            tenant_id: TENANT.to_string(),
            record: RunRecord::CompiledProgram {
                trace: run,
                policy: DecisionPolicyRef::Default,
                source,
                reusable: false,
                plan_binding: None,
                preplan_receipt: None,
                phrasings: BoundedVec::new(vec![ProgramPhrasing {
                    text: "summarize the {p0} report".into(),
                }])
                .unwrap(),
                match_threshold_micros: Some(500_000),
            },
        },
    )
    .await
    .unwrap();

    // The stored index, the phrasing node, its `phrasing_of` edge and one vector per node.
    let bytes = f
        .h
        .store
        .decision_artifact(TENANT, &match_key("summarize-report"))
        .unwrap()
        .expect("every commit stores an index");
    let index: ProgramMatchIndex = decode_artifact(&bytes, "program match index").unwrap();
    assert_eq!(index.slots.as_slice(), ["p0"]);
    assert!(index.space_digest.is_some());
    let program_node = program_node_id("summarize-report");
    let phrasing_node = phrasing_node_id("summarize-report", 0);
    assert!(f.core.get_node_properties(&phrasing_node).is_some());
    assert!(f.core.get_successors(&phrasing_node).unwrap().contains(&program_node));
    let store = f.core.semantic_store.read();
    assert!(store.get_embedding(&program_node).is_some());
    assert!(store.get_embedding(&phrasing_node).is_some());
    drop(store);
    assert_eq!(run_rows(&f.h), 2, "the program and its index");
    assert!(!committed.replayed);

    // The semantic answer through `Decide`: a nearest phrasing, parameters bound, nothing written.
    let verified = VerifiedRequestContext::verified_for_test_in_tenant("decider", TENANT);
    let request = with_text(
        lookup_request(&f.h, None),
        "another-task",
        "summarize the weekly report",
    );
    let batch = decide(&f.h, request).await.unwrap();
    let StatisticalOutcome::Advisory { scores, .. } = &batch.records.as_slice()[0].outcome else {
        panic!("a semantic match, got {:?}", batch.records.as_slice()[0].outcome);
    };
    let body = scores.as_slice()[0]
        .option_id
        .strip_prefix("program_match:")
        .expect("a program_match answer");
    let matched: ProgramMatch = serde_json::from_str(body).unwrap();
    assert_eq!(matched.task_signature, "summarize-report");
    assert_eq!(matched.match_kind, MatchKind::Semantic);
    assert_eq!(matched.matched_node.as_deref(), Some(phrasing_node.as_str()));
    assert_eq!(run_rows(&f.h), 2, "Decide wrote nothing");

    // An exact hit records no hits and makes zero embedding calls.
    let exact = with_text(
        lookup_request(&f.h, None),
        "summarize-report",
        "summarize the weekly report",
    );
    let recorded = super::super::family::with_program_candidates(
        &f.h.state,
        f.h.store.as_ref(),
        &verified,
        exact,
    )
    .await
    .unwrap();
    let candidates: ProgramCandidates = match &recorded
        .params
        .as_slice()
        .iter()
        .find(|param| param.name == "program_candidates")
        .unwrap()
        .value
    {
        TypedValue::Text(json) => serde_json::from_str(json).unwrap(),
        other => panic!("candidates are text, got {other:?}"),
    };
    assert_eq!(candidates.embedding_calls, 0);
    assert!(candidates.hits.is_empty());
}
