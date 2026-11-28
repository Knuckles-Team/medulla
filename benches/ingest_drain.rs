//! Page task overhead of a chunked drain (MEDULLA-INGEST-WORKER-R605.6).
//!
//! Nightly bench group `ingest/drain_page`: one ack-then-enqueue-next cycle of the drain chain
//! over the one jobs store, excluding source reads: claim the page task, decide its ack
//! (`on_ingest_drain_ack`), ack the task as completed and enqueue the next page under its
//! deterministic task id. Budget: p50 under 5 ms and p99 under 25 ms. `route_ingest_graph` is
//! measured on its own and stays under 2 microseconds at p99. A service level, not a pull
//! request gate. The served end-to-end proof is `tests/ingest/test_source_drain_served.py`.
//!
// spec: MEDULLA-INGEST-WORKER-R605.6
//!
//! Run: cargo bench --bench ingest_drain

use std::time::{Duration, Instant};

use medulla_ingestion_types::ingest_drain::{route_ingest_graph, DrainAck, RouteInputs};
use medulla_jobs::{JobStore, TaskAckOutcome, TaskSubmit, TaskSubmitOutcome};
use medulla_source_types::ingest_job::{task_id, IngestJobEnvelope};
use {
    medulla_h_ingest_repository::ingest_drain::on_ingest_drain_ack,
    medulla_h_ingest_repository::ingest_drain::start_drain,
    medulla_h_ingest_repository::ingest_drain::DrainAckOutcome,
};

const PAGES: usize = 500;
const ROUTES: usize = 20_000;
const CAPABILITY: &str = "eg:task_tenant:bench";
const BUDGET_P50: Duration = Duration::from_millis(5);
const BUDGET_P99: Duration = Duration::from_millis(25);
const BUDGET_ROUTE_P99: Duration = Duration::from_micros(2);

fn submit(store: &JobStore, envelope: &IngestJobEnvelope, now_ms: i64) {
    let spec = TaskSubmit {
        tenant: envelope.tenant.clone(),
        actor: "bench".to_string(),
        tenant_capability: CAPABILITY.to_string(),
        queue: "ingest".to_string(),
        kind: "document".to_string(),
        payload_ref: serde_json::to_string(envelope).expect("envelope encodes"),
        idempotency_key: task_id(envelope),
        priority: 5,
        not_before_ms: 0,
        job_id: None,
        max_attempts: None,
    };
    match store
        .submit_task(spec, now_ms)
        .expect("submit the page task")
    {
        TaskSubmitOutcome::Enqueued(_) => {}
        TaskSubmitOutcome::Duplicate => panic!("a fresh page task is never a duplicate"),
    }
}

fn percentile(sorted: &[Duration], fraction: usize) -> Duration {
    sorted[(sorted.len() * fraction / 100).min(sorted.len() - 1)]
}

fn main() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let authority = medulla_kernel::store_authority::process_authority();
    let store = JobStore::open_in_dir(
        dir.path(),
        authority.as_ref(),
        authority.principal(),
        &authority.proof(),
    )
    .expect("open the job store");

    let started =
        start_drain("bench", "crm", "accounts", "full", "main", 1_000).expect("the drain starts");
    let mut record = started.record;
    let mut envelope = started.first_page;
    submit(&store, &envelope, 1_000);

    let ack = DrainAck {
        items: 400,
        next_cursor: Some("cursor".to_string()),
        profile: Vec::new(),
    };
    let mut samples = Vec::with_capacity(PAGES);
    for page in 0..PAGES {
        let now_ms = 2_000 + page as i64 * 10;
        let began = Instant::now();
        let claim = store
            .claim_task("bench-worker", CAPABILITY, now_ms, 60_000)
            .expect("claim")
            .expect("the page task is ready");
        let checkpoint = envelope.page.clone().expect("a drain page");
        let DrainAckOutcome::Advanced {
            record: next_record,
            next,
        } = on_ingest_drain_ack(&record, &checkpoint, &ack, 500, now_ms, true)
            .expect("ack decision")
        else {
            panic!("a cursor advances the drain");
        };
        store
            .ack_task(
                "bench",
                &claim.job.job_id,
                "bench-worker",
                TaskAckOutcome::Succeeded,
                "",
                now_ms,
            )
            .expect("ack");
        submit(&store, &next, now_ms);
        samples.push(began.elapsed());
        record = next_record;
        envelope = next;
    }
    samples.sort();
    let (p50, p99) = (percentile(&samples, 50), percentile(&samples, 99));
    println!(
        "ingest/drain_page p50 = {p50:?} (budget {BUDGET_P50:?}), p99 = {p99:?} (budget {BUDGET_P99:?})"
    );
    assert!(p50 < BUDGET_P50, "p50 {p50:?} exceeds {BUDGET_P50:?}");
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");

    let inputs = RouteInputs {
        source: None,
        repository: Some("repo-graph"),
        tenant: Some("tenant-graph"),
        default: "main",
    };
    let mut routes = Vec::with_capacity(ROUTES);
    for _ in 0..ROUTES {
        let began = Instant::now();
        let route = route_ingest_graph(std::hint::black_box(&inputs)).expect("a valid route");
        routes.push(began.elapsed());
        std::hint::black_box(route);
    }
    routes.sort();
    let route_p99 = percentile(&routes, 99);
    println!(
        "ingest/drain_page route_ingest_graph p99 = {route_p99:?} (budget {BUDGET_ROUTE_P99:?})"
    );
    assert!(
        route_p99 < BUDGET_ROUTE_P99,
        "route p99 {route_p99:?} exceeds {BUDGET_ROUTE_P99:?}"
    );
}
