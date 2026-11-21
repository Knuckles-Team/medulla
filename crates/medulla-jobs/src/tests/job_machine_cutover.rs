//! MEDULLA-CQ-ORCH-R811.3 — the job writers route through the machine.
//!
//! This module is compiled as a child of `store` (see the `#[path]` declaration there), so it
//! reads the store's private writers and test helpers.

use super::tests::{request_batch, spec};
use super::*;
use crate::dev_scope_grant::open_dev_store_in_dir;
use medulla_types::transition_refused::TransitionRefusalReason;

/// The writer half of a source file: everything above its first test module.
fn writer_source(text: &str) -> &str {
    text.split("#[cfg(test)]").next().unwrap_or(text)
}

// spec: MEDULLA-CQ-ORCH-R811.3

#[test]
fn no_site_sets_state_outside_step() {
    for (name, text) in [
        ("store.rs", include_str!("../store.rs")),
        ("store/task_queue.rs", include_str!("../store/task_queue.rs")),
        ("claim.rs", include_str!("../claim.rs")),
        ("model.rs", include_str!("../model.rs")),
        ("intent.rs", include_str!("../intent.rs")),
    ] {
        let writer = writer_source(text);
        assert!(
            !writer.contains(".state ="),
            "{name} assigns the job state outside AnalyticsJob::transition"
        );
        for field in [".cancel_requested =", "cancel_requested:", ".not_before_ms =", "job.lease ="] {
            assert!(
                !writer.contains(field),
                "{name} still writes the deleted field {field}"
            );
        }
    }
    // Every writer of the state fires an event of the machine.
    let store = writer_source(include_str!("../store.rs"));
    for event in [
        "JobEvent::Claim",
        "JobEvent::Publish",
        "JobEvent::Complete",
        "JobEvent::Fail",
        "JobEvent::Cancel",
        "JobEvent::RequestCancel",
        "JobEvent::ConfirmCancel",
        "JobEvent::Reclaim",
    ] {
        assert!(store.contains(event), "store.rs never fires {event}");
    }
}

// spec: MEDULLA-CQ-ORCH-R811.3

#[test]
fn failed_job_never_revives() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_dev_store_in_dir(dir.path()).unwrap();
    let mut single_attempt = spec("g1", 1);
    single_attempt.max_attempts = 1;
    let submitted = store.submit(single_attempt).unwrap();
    let now = now_ms();
    let claim = store
        .claim_next(
            "worker:never-revives",
            &[],
            now,
            30_000,
            TenantJobQuota::default(),
        )
        .unwrap()
        .unwrap();
    let failed = store
        .fail_attempt_fenced(
            &submitted.job_id,
            &claim.lease.worker_ref,
            claim.lease.epoch,
            "boom",
            now,
        )
        .unwrap();
    assert!(matches!(failed.state, JobState::Failed { .. }));

    // A resume is refused by the machine and changes nothing.
    let expected = store.live_version().unwrap();
    let refused = store
        .resume_batch(
            &failed.job_id,
            &request_batch(&failed, "resume-failed", expected),
            now as u64,
        )
        .unwrap_err();
    match refused {
        JobError::TransitionRefused(refused) => {
            assert_eq!(refused.machine, "AnalyticsJobMachine");
            assert_eq!(refused.from, "failed");
            assert_eq!(refused.event, "Claim");
            assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
        }
        other => panic!("expected TRANSITION_REFUSED, got {other:?}"),
    }
    assert_eq!(store.get(&failed.job_id).unwrap().state, failed.state);

    // No worker is ever leased the failed job again.
    assert!(store
        .claim_next(
            "worker:late",
            &[],
            now + 1_000_000,
            30_000,
            TenantJobQuota::default(),
        )
        .unwrap()
        .is_none());
    assert!(matches!(
        store.get(&failed.job_id).unwrap().state,
        JobState::Failed { .. }
    ));
}
