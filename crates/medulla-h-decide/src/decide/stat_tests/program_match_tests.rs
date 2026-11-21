//! MEDULLA-DECISION-ENGINE-R135.17.1: the replay row of a stored program is committed through
//! `DecisionLog`, stored once under its key, stamped with the engine time and the committer.

use super::*;
use medulla_decision_types::decision::program_store::replay::{
    replay_key, ProgramReplay, ReplayOutcome, REPLAY_SCHEMA_VERSION,
};
use medulla_decision_types::decision::program_store::{RunRecord, RunRecordStored, StoredRun};

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
