//! Hot-path claim latency over a 1,000-row ready queue (MEDULLA-DURABLE-KERNEL-R750.753).
//!
//! Seeds 1,000 ready `agent_turn` WorkItem rows through the real durable path
//! (`RedbBackend::record_durable`), then claims them one at a time through
//! `Method::ClaimWorkItem`, which reaches `apply_claim_work_item_row`. Each
//! claim is one awaited durable write, so every sample pays its own commit.
//! Prints p50 and p99 and exits non-zero when a budget is exceeded: p50 under
//! 1 ms and p99 under 5 ms. The nightly job runs it; it never gates a pull
//! request.
//!
//! Configuration (env): `MEDULLA_WORK_ITEM_CLAIM_DIR` -- store directory
//! (default: the OS temp dir; point it at the disk under test, never at tmpfs).
//!
//! Run: cargo bench --features full --bench work_item_claim

use std::sync::Arc;
use std::time::Instant;

use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::protocol::Method;
use medulla_result_types::epistemic_operations::{
    ClaimWorkItemRequest, ClaimWorkItemRequestSchemaVersion,
};
use medulla_server_api::api::persistence::PersistenceBackend;

const GRAPH: &str = "__commons__";
const TENANT: &str = "tenant-a";
const QUEUE_ROWS: usize = 1_000;
const SEED_WAVE: usize = 250;
const P50_BUDGET_MS: f64 = 1.0;
const P99_BUDGET_MS: f64 = 5.0;

fn item_id(index: usize) -> String {
    format!("wi-claim-{index:05}")
}

fn ready_item(index: usize) -> Method {
    let props = serde_json::json!({
        "node_type": "WorkItem",
        "tenant": TENANT,
        "kind": "agent_turn",
        "status": "ready",
    });
    Method::AddNode {
        node_id: item_id(index),
        properties_msgpack: rmp_serde::to_vec_named(&props).expect("encode WorkItem"),
    }
}

fn claim(index: usize) -> Method {
    Method::ClaimWorkItem {
        request: ClaimWorkItemRequest {
            schema_version: ClaimWorkItemRequestSchemaVersion::V1,
            tenant_ref: TENANT.to_string(),
            work_item_id: Some(item_id(index)),
            queue_ref: None,
            resource_class: None,
            fairness_group: None,
            worker_ref: "bench-worker".to_string(),
            now_ms: 10_000,
            lease_ms: 60_000,
            max_tenant_in_flight: 4_096,
        },
    }
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

async fn seed(backend: &Arc<RedbBackend>) {
    let mut next = 0;
    while next < QUEUE_ROWS {
        let end = QUEUE_ROWS.min(next + SEED_WAVE);
        let tasks: Vec<_> = (next..end)
            .map(|index| {
                let backend = Arc::clone(backend);
                tokio::spawn(async move { backend.record_durable(GRAPH, &ready_item(index)).await })
            })
            .collect();
        for task in tasks {
            task.await.expect("seed task").expect("seed write");
        }
        next = end;
    }
}

async fn measure(backend: &Arc<RedbBackend>) -> Vec<f64> {
    let mut samples = Vec::with_capacity(QUEUE_ROWS);
    for index in 0..QUEUE_ROWS {
        let started = Instant::now();
        backend
            .record_durable(GRAPH, &claim(index))
            .await
            .expect("claim write");
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    samples
}

fn main() {
    let root = std::env::var("MEDULLA_WORK_ITEM_CLAIM_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = root.join(format!("medulla-work-item-claim-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("bench dir");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("tokio runtime");
    let backend = Arc::new(
        RedbBackend::open(dir.join("engine").to_string_lossy().to_string(), 4096)
            .expect("open redb backend"),
    );
    runtime.block_on(seed(&backend));
    let mut samples = runtime.block_on(measure(&backend));
    backend.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    samples.sort_by(f64::total_cmp);
    let p50 = percentile(&samples, 0.50);
    let p99 = percentile(&samples, 0.99);
    println!(
        "work_item_claim queue={QUEUE_ROWS} n={} p50={p50:.3}ms p99={p99:.3}ms (budget p50<{P50_BUDGET_MS}ms p99<{P99_BUDGET_MS}ms)",
        samples.len()
    );
    if p50 >= P50_BUDGET_MS || p99 >= P99_BUDGET_MS {
        eprintln!("work_item_claim: budget exceeded");
        std::process::exit(1);
    }
}
