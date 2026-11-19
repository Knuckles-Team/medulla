//! Usage totals over one million traces (MEDULLA-ORCH-VOCAB-R010.101).
//!
//! 32 reader threads run 1,000 `run_trace_usage` queries between them over
//! 1,000,000 `RunTrace` property maps grouped by project, and the run reports
//! the p95 handler time against the 500 ms budget of R178.1.
//!
//! Run: `cargo bench --bench usage_totals` (nightly).

use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla_mutation_types::harness_metrics::{run_trace_usage, RunTraceUsageQuery};
use medulla_types::benchmark_claim::PriceTable;
use serde_json::{json, Map, Value};

const TRACES: usize = 1_000_000;
const READERS: usize = 32;
const QUERIES: usize = 1_000;
const BUDGET: Duration = Duration::from_millis(500);

fn trace(i: usize) -> Map<String, Value> {
    let Value::Object(map) = json!({
        "node_type": "RunTrace",
        "status": "completed",
        "runModel": format!("model-{}", i % 4),
        "projectId": format!("proj-{}", i % 64),
        "agentId": format!("agent-{}", i % 256),
        "completedAtMs": 1_000 + (i % 1_000) as u64,
        "uncachedInputTokens": (i % 9_973) as u64,
        "cachedInputTokens": (i % 811) as u64,
        "outputTokens": (i % 4_001) as u64,
        "reasoningOutputTokens": (i % 157) as u64,
    }) else {
        unreachable!()
    };
    map
}

fn table() -> PriceTable {
    let rates: serde_json::Map<String, Value> = (0..4)
        .map(|m| {
            (
                format!("model-{m}"),
                json!({"uncached_input": 300, "cached_input": 30, "output": 1500, "reasoning_output": 1500}),
            )
        })
        .collect();
    serde_json::from_value(json!({
        "schema_version": 1,
        "version": 1,
        "currency": "USD",
        "minor_unit_exponent": 2,
        "token_rates": rates,
    }))
    .expect("bench price table")
}

fn main() {
    let traces: Arc<Vec<_>> = Arc::new((0..TRACES).map(trace).collect());
    let table = Arc::new(table());
    let text = |name: &str| match name {
        "grouping" => Some("project".to_string()),
        "currency" => Some("USD".to_string()),
        "price_table_revision" => Some("bench".to_string()),
        _ => None,
    };
    let number = |name: &str| match name {
        "window_start_ms" => Some(1_000.0),
        "window_end_ms" => Some(2_000.0),
        "page_size" => Some(100.0),
        _ => None,
    };
    let query = Arc::new(RunTraceUsageQuery::from_params(text, number, 1_000).expect("query"));
    let per_reader = QUERIES / READERS + 1;
    let handles: Vec<_> = (0..READERS)
        .map(|_| {
            let (traces, table, query) = (traces.clone(), table.clone(), query.clone());
            std::thread::spawn(move || {
                (0..per_reader)
                    .map(|_| {
                        let start = Instant::now();
                        run_trace_usage(&query, &traces, Some(&table), 1).expect("usage page");
                        start.elapsed()
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut times: Vec<Duration> = handles
        .into_iter()
        .flat_map(|h| h.join().expect("reader"))
        .take(QUERIES)
        .collect();
    times.sort();
    let p95 = times[times.len() * 95 / 100];
    println!(
        "usage_totals p95={p95:?} budget={BUDGET:?} queries={}",
        times.len()
    );
    assert!(p95 <= BUDGET, "usage_totals p95 {p95:?} exceeds {BUDGET:?}");
}
