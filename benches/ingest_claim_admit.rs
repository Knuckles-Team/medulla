//! Claim with admission (MEDULLA-INGEST-WORKER-R604.6).
//!
//! Nightly bench group `ingest/claim_admit`: the served claim handler of
//! `MEDULLA-INGEST-WORKER-R604.3` called by 64 workers over 8 lanes against the one jobs store.
//! Each claim runs the one reserved-worker admission rule and then leases the next ready task.
//! Budget: p50 under 150 microseconds and p99 under 1 ms, matching
//! `MEDULLA-DURABLE-KERNEL-R071.600`. A service level, not a pull request gate. The served
//! functional proof is `tests/ingest/test_lane_admission_served.py`.
//!
// spec: MEDULLA-INGEST-WORKER-R604.6
//!
//! Run: cargo bench --bench ingest_claim_admit

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use medulla_h_jobs::task_queue::{claim, enqueue, TaskQueueCall};
use medulla_h_jobs::worker_admission::AdmissionInputs;
use medulla_jobs::JobStore;
use medulla_server_api::knob_config::{scheduler_config_from, KnobConfig};
use medulla_source_types::ingest_job::IngestRetryPolicy;
use medulla_work_types::scheduler_config::IngestRetryLimits;
use medulla_work_types::task_queue::{default_priority, TaskClaim, TaskEnqueue};
use medulla_work_types::worker_registry::{LaneLoad, WorkerRegistration, WorkerRegistry};

const WORKERS: usize = 64;
const LANES: usize = 8;
const CLAIMS_PER_WORKER: usize = 20;
const TENANT: &str = "bench";
const PRINCIPAL: &str = "principal:bench";
const BUDGET_P50: Duration = Duration::from_micros(150);
const BUDGET_P99: Duration = Duration::from_millis(1);

fn lane_id(lane: usize) -> String {
    format!("lane-{lane}")
}

/// 64 live workers, eight per lane, and a policy that reserves one slot on each of the 8 lanes.
fn admission() -> AdmissionInputs {
    let mut registry = WorkerRegistry::default();
    for worker in 0..WORKERS {
        registry
            .register(
                WorkerRegistration {
                    worker_id: format!("worker-{worker}"),
                    host_iri: "host:bench".to_string(),
                    cores: 4,
                    memory_bytes: 8 * 1024 * 1024 * 1024,
                    lanes_supported: BTreeSet::from([lane_id(worker % LANES)]),
                },
                0,
                u64::MAX / 2,
            )
            .expect("register the worker");
    }
    let mut config = scheduler_config_from(&KnobConfig::default()).expect("the default policy");
    config.total_workers = WORKERS as u32;
    config.lane_floors = (0..LANES).map(|lane| (lane_id(lane), 1)).collect();
    // Half the pool runs and every lane has queued work, so admission weighs a loaded pool.
    let lane_load: BTreeMap<String, LaneLoad> = (0..LANES)
        .map(|lane| {
            (
                lane_id(lane),
                LaneLoad {
                    running: 2,
                    queued: 10,
                },
            )
        })
        .collect();
    AdmissionInputs {
        registry,
        config,
        lane_load,
        shard_writer_width: 4,
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
    let admission = admission();
    let policy = Ok(IngestRetryPolicy::from_limits(&IngestRetryLimits::default()));

    let claims = WORKERS * CLAIMS_PER_WORKER;
    for task in 0..claims {
        let call = TaskQueueCall {
            req_id: task as u64,
            tenant: TENANT,
            principal: PRINCIPAL,
            now_ms: 1_000,
        };
        let enqueued = enqueue(
            &store,
            &call,
            TaskEnqueue {
                queue: format!("bench-{}", task % LANES),
                kind: "document".to_string(),
                payload_ref: "{}".to_string(),
                idempotency_key: format!("bench-task-{task}"),
                priority: default_priority(),
                not_before_ms: 0,
            },
        );
        assert!(enqueued.error.is_none(), "enqueue {task}: {:?}", enqueued.error);
    }

    let mut samples = Vec::with_capacity(claims);
    for round in 0..claims {
        let worker = round % WORKERS;
        let now_ms = 2_000 + round as i64;
        let call = TaskQueueCall {
            req_id: round as u64,
            tenant: TENANT,
            principal: PRINCIPAL,
            now_ms,
        };
        let request = TaskClaim {
            worker_id: format!("worker-{worker}"),
            grant_id: format!("grant-{round}"),
            lease_ms: 600_000,
        };
        let began = Instant::now();
        let answer = claim(&store, &call, request, &admission, &policy);
        samples.push(began.elapsed());
        assert!(
            answer.error.is_none(),
            "claim {round} by worker-{worker}: {:?}",
            answer.error
        );
    }
    samples.sort();
    let (p50, p99) = (percentile(&samples, 50), percentile(&samples, 99));
    println!(
        "ingest/claim_admit p50 = {p50:?} (budget {BUDGET_P50:?}), p99 = {p99:?} (budget {BUDGET_P99:?})"
    );
    assert!(p50 < BUDGET_P50, "p50 {p50:?} exceeds {BUDGET_P50:?}");
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
}
