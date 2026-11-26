//! Change-envelope commit at 1,000 records (MEDULLA-REPO-INGEST-R002.655).
//!
//! Nightly bench group `ingest/change_envelope_commit`: 1,000 one-node envelopes go through the
//! served `ApplyChangeEnvelopeDraft` dispatch chain (`apply_one_change_envelope`) against one
//! durable graph. Budget per commit: p50 at most 1 ms and p99 at most 6 ms on one writer. A
//! second phase commits another 1,000 records from 8 concurrent writers; the resident
//! high-water mark grows by at most 24 MiB over it. The run exits non-zero when a budget is
//! exceeded. A service level, not a pull request gate. The served journey that gates the pull
//! request is `tests/test_source_ingest_capabilities_served.py`.
//!
// spec: MEDULLA-REPO-INGEST-R002.655
//!
//! Run: cargo bench --bench change_envelope_commit

#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/common/test_support.rs"]
mod test_support;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use medulla_protocol_types::protocol::{GraphType, Method};
use serde_json::json;

const SECRET: &str = "change-envelope-commit-bench-secret";
const TENANT: &str = common::TEST_TENANT;
const GRAPH: &str = "change-envelope-commit-bench";
const RECORDS: usize = 1_000;
const WRITERS: usize = 8;
const BUDGET_P50: Duration = Duration::from_millis(1);
const BUDGET_P99: Duration = Duration::from_millis(6);
const BUDGET_MIB: f64 = 24.0;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Resident high-water mark in MiB (`VmHWM`), 0 where procfs is absent.
fn high_water_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmHWM:"))
                .and_then(|rest| rest.split_whitespace().next()?.parse::<f64>().ok())
        })
        .map_or(0.0, |kib| kib / 1024.0)
}

/// One `AddNode` envelope for record `index` of `phase`: its own node, source object and policies.
fn envelope(phase: &str, index: usize) -> Method {
    let tag = format!("{phase}-{index}");
    let node = format!("doc:{tag}");
    let source = format!("source:{tag}");
    let mut draft = medulla_server_graph::dispatch::change_envelope_draft::tests::draft_for(
        TENANT,
        json!({"type": "Document", "title": format!("record {tag}")}),
    );
    draft.envelope_id = format!("envelope:{tag}");
    draft.mutation.batch_id = format!("batch:{tag}");
    if let Method::AddNode { node_id, .. } = &mut draft.mutation.operations[0].method {
        node_id.clone_from(&node);
    }
    draft.content_version.object_id.clone_from(&source);
    draft.policies[0].object_id.clone_from(&node);
    draft.policies[0].policy_id = format!("policy:{tag}");
    let mut source_policy = draft.policies[0].clone();
    source_policy.object_id = source;
    source_policy.policy_id = format!("policy:{tag}:source");
    draft.policies.push(source_policy);
    Method::ApplyChangeEnvelopeDraft {
        draft: Box::new(draft),
    }
}

async fn commit(state: &test_support::SharedState, method: Method) -> Duration {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let request = test_support::request(SECRET, id, GRAPH, method);
    let began = Instant::now();
    let response = test_support::dispatch(state, request).await;
    let elapsed = began.elapsed();
    assert!(response.error.is_none(), "commit refused: {response:?}");
    elapsed
}

fn percentile(sorted: &[Duration], percent: usize) -> Duration {
    sorted[(sorted.len() * percent / 100).min(sorted.len() - 1)]
}

fn main() {
    common::configure_authority();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(WRITERS)
        .enable_all()
        .build()
        .expect("a tokio runtime");
    runtime.block_on(async {
        let state = test_support::durable_state(SECRET, common::current_isolation());
        let created = test_support::dispatch(
            &state,
            test_support::request(
                SECRET,
                NEXT_ID.fetch_add(1, Ordering::Relaxed),
                GRAPH,
                Method::CreateGraph {
                    graph_name: GRAPH.to_string(),
                    graph_type: GraphType::Global,
                },
            ),
        )
        .await;
        assert!(created.error.is_none(), "{created:?}");

        let mut samples = Vec::with_capacity(RECORDS);
        for index in 0..RECORDS {
            samples.push(commit(&state, envelope("single", index)).await);
        }
        samples.sort();
        let (p50, p99) = (percentile(&samples, 50), percentile(&samples, 99));
        println!(
            "ingest/change_envelope_commit p50 = {p50:?} (budget {BUDGET_P50:?}), p99 = {p99:?} (budget {BUDGET_P99:?})"
        );
        assert!(p50 <= BUDGET_P50, "p50 {p50:?} exceeds {BUDGET_P50:?}");
        assert!(p99 <= BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");

        let before = high_water_mib();
        let per_writer = RECORDS / WRITERS;
        let mut writers = Vec::new();
        for writer in 0..WRITERS {
            let state = state.clone();
            writers.push(tokio::spawn(async move {
                for index in 0..per_writer {
                    commit(&state, envelope(&format!("writer{writer}"), index)).await;
                }
            }));
        }
        for writer in writers {
            writer.await.expect("a writer finished");
        }
        let grown = high_water_mib() - before;
        println!(
            "ingest/change_envelope_commit resident growth with {WRITERS} writers = {grown:.1} MiB (budget {BUDGET_MIB} MiB)"
        );
        assert!(grown <= BUDGET_MIB, "growth {grown:.1} MiB exceeds {BUDGET_MIB} MiB");
    });
}
