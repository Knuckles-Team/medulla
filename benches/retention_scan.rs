//! Retention cleanup step latency (MEDULLA-DURABLE-KERNEL-R700.6).
//!
//! Seeds 100,000 aged `Document` nodes through `Method::BatchUpdate` into one graph over the
//! real durable path (`commit_internal_graph_methods` on a redb backend), then times 100
//! steps of 1,000: each step is `cleanup_graph` plus the one `BatchUpdate` that removes what
//! it chose, the work of one `RetentionCleanupRun`. Prints p50, p99 and the resident-set
//! growth, and exits non-zero when a budget is exceeded: p50 under 8 ms, p99 under 50 ms,
//! memory under 64 MiB. The nightly job runs it; it never gates a pull request.
//!
//! Configuration (env): `MEDULLA_RETENTION_SCAN_DIR` -- store directory (default: the OS temp
//! dir; point it at the disk under test, never at tmpfs).
//!
//! Run: cargo bench --features full --bench retention_scan

use std::sync::Arc;
use std::time::Instant;

use medulla_graph_core::graph::GraphCore;
use medulla_h_graph::graph_ops::retention_cleanup::{cleanup_graph, RetentionCursor};
use medulla_mutation_types::retention_policy::{HoldCheck, RetentionPolicy};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::protocol::{Method, ResultPayload};
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_mutation::mutation_batch::{
    commit_internal_graph_methods, CommitOrigin, InternalGraphCommitRequest,
};

const GRAPH: &str = "__commons__";
const NODES: usize = 100_000;
const SEED_BATCH: usize = 1_000;
const STEP: u32 = 1_000;
const STEPS: usize = NODES / STEP as usize;
const NOW_MS: u64 = 10_000_000;
const P50_BUDGET_MS: f64 = 8.0;
const P99_BUDGET_MS: f64 = 50.0;
const MEMORY_BUDGET_MIB: f64 = 64.0;

fn batch_update(operations: &[serde_json::Value]) -> Method {
    Method::BatchUpdate {
        operations_msgpack: rmp_serde::to_vec_named(&operations).expect("encode operations"),
    }
}

fn percentile_ms(sorted: &[f64], fraction: f64) -> f64 {
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

async fn commit(
    backend: &Arc<dyn PersistenceBackend>,
    core: &Arc<GraphCore>,
    batch_id: &str,
    method: Method,
) {
    commit_internal_graph_methods(InternalGraphCommitRequest::new(
        Some(backend),
        core,
        CommitOrigin {
            request_id: 0,
            principal: Some("bench:retention-scan"),
        },
        GRAPH,
        batch_id,
        vec![method],
        &ResultPayload::Bool(true),
    ))
    .await
    .expect("internal commit");
}

async fn seed(backend: &Arc<dyn PersistenceBackend>, core: &Arc<GraphCore>) {
    for chunk in 0..NODES / SEED_BATCH {
        let operations: Vec<serde_json::Value> = (0..SEED_BATCH)
            .map(|offset| {
                let id = chunk * SEED_BATCH + offset;
                serde_json::json!({
                    "op": "add_node",
                    "id": format!("doc-{id:06}"),
                    "properties": { "type": "Document", "updated_at": 1 },
                })
            })
            .collect();
        commit(
            backend,
            core,
            &format!("retention-scan-seed-{chunk:04}"),
            batch_update(&operations),
        )
        .await;
    }
}

async fn measure(backend: &Arc<dyn PersistenceBackend>, core: &Arc<GraphCore>) -> Vec<f64> {
    let policy = RetentionPolicy::new(
        1_000,
        STEP,
        vec![HoldCheck::LiveReference, HoldCheck::LegalHold],
    )
    .expect("policy");
    let mut cursor = RetentionCursor::default();
    let mut samples = Vec::with_capacity(STEPS);
    for step in 0..STEPS {
        let started = Instant::now();
        let report = cleanup_graph(core, &policy, &cursor, NOW_MS);
        if let Some(removal) = report.removal_batch().expect("removal batch") {
            commit(
                backend,
                core,
                &format!("retention-scan-step-{step:04}"),
                removal,
            )
            .await;
        }
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        cursor = report.next_cursor;
    }
    samples
}

fn main() {
    let root = std::env::var("MEDULLA_RETENTION_SCAN_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = root.join(format!("medulla-retention-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("bench dir");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("tokio runtime");
    let redb = Arc::new(
        RedbBackend::open(dir.join("engine").to_string_lossy().to_string(), 4096)
            .expect("open redb backend"),
    );
    let backend: Arc<dyn PersistenceBackend> = redb.clone();
    let core = Arc::new(GraphCore::new());
    runtime.block_on(seed(&backend, &core));
    let rss_before = rss_mib();
    let mut samples = runtime.block_on(measure(&backend, &core));
    let growth = rss_mib() - rss_before;
    redb.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    samples.sort_by(f64::total_cmp);
    let (p50, p99) = (percentile_ms(&samples, 0.50), percentile_ms(&samples, 0.99));
    println!(
        "retention_scan nodes={NODES} step={STEP} steps={STEPS} p50={p50:.2}ms p99={p99:.2}ms rss_growth={growth:.2}MiB (budget p50<{P50_BUDGET_MS}ms p99<{P99_BUDGET_MS}ms rss<{MEMORY_BUDGET_MIB}MiB)"
    );
    if p50 >= P50_BUDGET_MS || p99 >= P99_BUDGET_MS || growth >= MEMORY_BUDGET_MIB {
        eprintln!("retention_scan: budget exceeded");
        std::process::exit(1);
    }
}
