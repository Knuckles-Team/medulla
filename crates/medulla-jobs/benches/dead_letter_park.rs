//! Nightly hot-path proof for the dead-letter park (MEDULLA-REPO-INGEST-R002.655).
//!
//! Parks 1,000 ingest tasks through the one writer, `JobStore::ack_task` with
//! `TaskAckOutcome::DeadLetter`, and measures each park: the failed transition and the
//! dead-letter row commit together. Budget: p99 at most 2 ms per park.
//!
//! Run: cargo bench -p medulla-jobs --bench dead_letter_park

// spec: MEDULLA-REPO-INGEST-R002.655

use std::time::{Duration, Instant};

use medulla_jobs::dev_scope_grant::open_dev_store_in_dir;
use medulla_jobs::{TaskAckOutcome, TaskSubmit, TaskSubmitOutcome};

const PARKS: usize = 1_000;
const P99_BUDGET: Duration = Duration::from_millis(2);
const CAPABILITY: &str = "eg:task_tenant:bench";

fn main() {
    let dir = tempfile::tempdir().expect("persist dir");
    let store = open_dev_store_in_dir(dir.path()).expect("open the jobs store");
    let mut ids = Vec::with_capacity(PARKS);
    for n in 0..PARKS {
        let spec = TaskSubmit {
            tenant: "acme".to_string(),
            actor: "bench".to_string(),
            tenant_capability: CAPABILITY.to_string(),
            queue: "ingest".to_string(),
            kind: "document".to_string(),
            payload_ref: "{}".to_string(),
            idempotency_key: format!("park-{n}"),
            priority: 0,
            not_before_ms: 0,
            job_id: None,
            max_attempts: None,
        };
        match store.submit_task(spec, 1_000).expect("enqueue") {
            TaskSubmitOutcome::Enqueued(job) => ids.push(job.job_id),
            TaskSubmitOutcome::Duplicate => panic!("unexpected duplicate"),
        }
    }
    let mut samples = Vec::with_capacity(PARKS);
    for _ in 0..PARKS {
        let claim = store
            .claim_task("bench-worker", CAPABILITY, 2_000, 60_000)
            .expect("claim")
            .expect("a ready task");
        let started = Instant::now();
        store
            .ack_task(
                "acme",
                &claim.job.job_id,
                "bench-worker",
                TaskAckOutcome::DeadLetter {
                    code: "INGEST_RETRIES_EXHAUSTED",
                },
                "bench failure",
                3_000,
            )
            .expect("park");
        samples.push(started.elapsed());
    }
    let parked = store
        .list_dead_letters("acme", "ingest", 1_000)
        .expect("list")
        .len();
    assert_eq!(parked, PARKS, "every park left one dead-letter row");
    samples.sort();
    let p99 = samples[PARKS * 99 / 100];
    println!("ingest/dead_letter_park p99 = {p99:?} (budget {P99_BUDGET:?})");
    assert!(p99 <= P99_BUDGET, "p99 {p99:?} over {P99_BUDGET:?}");
}
