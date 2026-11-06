//! Hot-path effect-receipt latency (MEDULLA-DURABLE-KERNEL-R820.6).
//!
//! Seeds 10,000 started WorkItem rows through the real durable path
//! (`RedbBackend::record_durable`, the store setup of `common/redb_fan_in.rs`),
//! then 32 concurrent writers record one `Method::RecordEffectReceipt` each per
//! item. Every receipt is one awaited durable write. Prints p50, p99 and the
//! resident-set growth, and exits non-zero when a budget is exceeded: p50 under
//! 150 microseconds, p99 under 700 microseconds, memory under 16 MiB. The
//! nightly job runs it; it never gates a pull request.
//!
//! Configuration (env): `MEDULLA_WORK_ITEM_EFFECT_DIR` -- store directory
//! (default: the OS temp dir; point it at the disk under test, never at tmpfs).
//!
//! Run: cargo bench --features full --bench work_item_effect_receipt

use std::sync::Arc;
use std::time::Instant;

use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::protocol::Method;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_work_types::work_item_read::{
    EffectOutcome, EffectReceipt, RecordEffectReceiptRequest,
};

#[path = "common/redb_fan_in.rs"]
mod redb_fan_in;
use redb_fan_in::GRAPH;

const TENANT: &str = "tenant-a";
const WORKER: &str = "bench-worker";
const REQUEST_REF: &str = "sha256:bench-request";
const ITEMS: usize = 10_000;
const WRITERS: usize = 32;
const SEED_WAVE: usize = 250;
const P50_BUDGET_US: f64 = 150.0;
const P99_BUDGET_US: f64 = 700.0;
const MEMORY_BUDGET_MIB: f64 = 16.0;

fn item_id(index: usize) -> String {
    format!("wi-effect-{index:05}")
}

fn started_item(index: usize) -> Method {
    let props = serde_json::json!({
        "node_type": "WorkItem",
        "tenant": TENANT,
        "kind": "agent_turn",
        "status": "running",
        "attempt": 1,
        "lease_owner": WORKER,
        "payload_ref": REQUEST_REF,
        "effect_phase": "started",
    });
    Method::AddNode {
        node_id: item_id(index),
        properties_msgpack: rmp_serde::to_vec_named(&props).expect("encode WorkItem"),
    }
}

fn receipt(index: usize) -> Method {
    Method::RecordEffectReceipt {
        request: RecordEffectReceiptRequest {
            tenant: TENANT.to_string(),
            work_item_id: item_id(index),
            attempt: 1,
            expected_revision: 1,
            receipt: EffectReceipt {
                job_id: item_id(index),
                request_ref: REQUEST_REF.to_string(),
                principal_ref: WORKER.to_string(),
                outcome: EffectOutcome::Applied,
                receipt_ref: format!("sha256:bench-receipt-{index}"),
                committed_at: 7,
            },
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

async fn seed(backend: &Arc<RedbBackend>) {
    for start in (0..ITEMS).step_by(SEED_WAVE) {
        let tasks: Vec<_> = (start..ITEMS.min(start + SEED_WAVE))
            .map(|index| {
                let backend = Arc::clone(backend);
                tokio::spawn(
                    async move { backend.record_durable(GRAPH, &started_item(index)).await },
                )
            })
            .collect();
        for task in tasks {
            task.await.expect("seed task").expect("seed write");
        }
    }
}

/// One writer records the receipts of the items congruent to `lane` modulo the
/// writer count and returns its per-receipt latencies in microseconds.
async fn write_lane(backend: Arc<RedbBackend>, lane: usize) -> Vec<f64> {
    let mut samples = Vec::with_capacity(ITEMS / WRITERS + 1);
    for index in (lane..ITEMS).step_by(WRITERS) {
        let started = Instant::now();
        backend
            .record_durable(GRAPH, &receipt(index))
            .await
            .expect("receipt write");
        samples.push(started.elapsed().as_secs_f64() * 1_000_000.0);
    }
    samples
}

async fn measure(backend: &Arc<RedbBackend>) -> Vec<f64> {
    let lanes: Vec<_> = (0..WRITERS)
        .map(|lane| tokio::spawn(write_lane(Arc::clone(backend), lane)))
        .collect();
    let mut samples = Vec::with_capacity(ITEMS);
    for lane in lanes {
        samples.extend(lane.await.expect("writer task"));
    }
    samples
}

fn main() {
    let root = std::env::var("MEDULLA_WORK_ITEM_EFFECT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = root.join(format!("medulla-work-item-effect-{}", std::process::id()));
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
    runtime.block_on(seed(&backend));
    let rss_before = rss_mib();
    let mut samples = runtime.block_on(measure(&backend));
    let growth = rss_mib() - rss_before;
    backend.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    samples.sort_by(f64::total_cmp);
    let (p50, p99) = (percentile_us(&samples, 0.50), percentile_us(&samples, 0.99));
    println!(
        "work_item_effect_receipt items={ITEMS} writers={WRITERS} p50={p50:.1}us p99={p99:.1}us rss_growth={growth:.2}MiB (budget p50<{P50_BUDGET_US}us p99<{P99_BUDGET_US}us rss<{MEMORY_BUDGET_MIB}MiB)"
    );
    if p50 >= P50_BUDGET_US || p99 >= P99_BUDGET_US || growth >= MEMORY_BUDGET_MIB {
        eprintln!("work_item_effect_receipt: budget exceeded");
        std::process::exit(1);
    }
}
