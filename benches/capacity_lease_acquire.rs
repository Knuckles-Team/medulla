//! Hot-path capacity lease acquisition latency (MEDULLA-DURABLE-KERNEL-R867).
//!
//! Provisions one capacity cell through the real durable path
//! (`RedbBackend::record_durable` of `Method::UpdateCapacityCell`), then 32
//! concurrent writers acquire 10,000 one-unit leases through
//! `Method::AcquireCapacity`. Every acquisition is one awaited durable write.
//! Prints p50, p99 and the resident-set growth, and exits non-zero when a
//! budget is exceeded: p50 under 80 microseconds, p99 under 400 microseconds,
//! memory under 8 MiB. The nightly job runs it; it never gates a pull request.
//!
//! Configuration (env): `MEDULLA_CAPACITY_LEASE_DIR` -- store directory
//! (default: the OS temp dir; point it at the disk under test, never at tmpfs).
//!
//! Run: cargo bench --features full --bench capacity_lease_acquire

use std::sync::Arc;
use std::time::Instant;

use medulla_mutation_types::capacity_lease::{CapacityCell, CapacityResourceClass, LeasePriority};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::protocol::Method;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_work_types::native_control::{
    CapacityAcquireRequest, CapacityCellUpdateRequest, CapacityDemand, NativeControlSchemaVersion,
};

#[path = "common/redb_fan_in.rs"]
mod redb_fan_in;
use redb_fan_in::GRAPH;

const TENANT: &str = "tenant-a";
const OWNER: &str = "principal:sha256:bench";
const CELL: &str = "cell-bench";
const LEASES: usize = 10_000;
const WRITERS: usize = 32;
const P50_BUDGET_US: f64 = 80.0;
const P99_BUDGET_US: f64 = 400.0;
const MEMORY_BUDGET_MIB: f64 = 8.0;

fn provision() -> Method {
    Method::UpdateCapacityCell {
        request: CapacityCellUpdateRequest {
            schema_version: NativeControlSchemaVersion::V1,
            cell: CapacityCell {
                cell_id: CELL.to_string(),
                parent_id: None,
                resource_class: CapacityResourceClass::Worker,
                capacity: (LEASES as u64) * 2,
                reserved_floor: 0,
                epoch: 1,
                policy_digest: "sha256:bench".to_string(),
                updated_at_ms: 1,
                throttle: None,
            },
            expected_epoch: None,
            now_ms: 1,
        },
    }
}

fn acquire(index: usize) -> Method {
    Method::AcquireCapacity {
        request: CapacityAcquireRequest {
            schema_version: NativeControlSchemaVersion::V1,
            tenant_ref: TENANT.to_string(),
            work_item_id: format!("wi-lease-{index:05}"),
            owner_digest: OWNER.to_string(),
            idempotency_key: format!("bench-lease-{index:05}"),
            priority: LeasePriority::Orchestration,
            demands: vec![CapacityDemand {
                cell_id: CELL.to_string(),
                resource_class: CapacityResourceClass::Worker,
                amount: 1,
            }],
            lease_id: None,
            ttl_ms: 3_600_000,
            now_ms: 10,
            cost_budget_micros: None,
            token_budget: None,
        },
    }
}

fn percentile_us(sorted: &[f64], fraction: f64) -> f64 {
    sorted
        .get(((sorted.len().max(1) - 1) as f64 * fraction).round() as usize)
        .copied()
        .unwrap_or(0.0)
}

/// The process's resident set in MiB (VmRSS), or NaN off Linux.
fn rss_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmRSS:"))
                .and_then(|rest| rest.split_whitespace().next()?.parse::<f64>().ok())
        })
        .map_or(f64::NAN, |kib| kib / 1024.0)
}

/// One writer acquires the leases congruent to `lane` modulo the writer count
/// and returns its per-acquisition latencies in microseconds.
async fn write_lane(backend: Arc<RedbBackend>, lane: usize) -> Vec<f64> {
    let mut samples = Vec::with_capacity(LEASES / WRITERS + 1);
    for index in (lane..LEASES).step_by(WRITERS) {
        let started = Instant::now();
        backend
            .record_durable(GRAPH, &acquire(index))
            .await
            .expect("acquire write");
        samples.push(started.elapsed().as_secs_f64() * 1_000_000.0);
    }
    samples
}

async fn measure(backend: &Arc<RedbBackend>) -> Vec<f64> {
    let lanes: Vec<_> = (0..WRITERS)
        .map(|lane| tokio::spawn(write_lane(Arc::clone(backend), lane)))
        .collect();
    let mut samples = Vec::with_capacity(LEASES);
    for lane in lanes {
        samples.extend(lane.await.expect("writer task"));
    }
    samples
}

fn main() {
    let root = std::env::var("MEDULLA_CAPACITY_LEASE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = root.join(format!("medulla-capacity-lease-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("bench dir");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(WRITERS)
        .enable_all()
        .build()
        .expect("tokio runtime");
    let backend = Arc::new(
        RedbBackend::open(dir.join("engine").to_string_lossy().to_string(), 4096)
            .expect("open redb backend"),
    );
    runtime
        .block_on(backend.record_durable(GRAPH, &provision()))
        .expect("provision cell");
    let rss_before = rss_mib();
    let mut samples = runtime.block_on(measure(&backend));
    let growth = rss_mib() - rss_before;
    backend.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    samples.sort_by(f64::total_cmp);
    let (p50, p99) = (percentile_us(&samples, 0.50), percentile_us(&samples, 0.99));
    println!(
        "capacity_lease_acquire leases={LEASES} writers={WRITERS} p50={p50:.1}us p99={p99:.1}us rss_growth={growth:.2}MiB (budget p50<{P50_BUDGET_US}us p99<{P99_BUDGET_US}us rss<{MEMORY_BUDGET_MIB}MiB)"
    );
    if p50 >= P50_BUDGET_US || p99 >= P99_BUDGET_US || growth >= MEMORY_BUDGET_MIB {
        eprintln!("capacity_lease_acquire: budget exceeded");
        std::process::exit(1);
    }
}
