//! MEDULLA-UNIFIED-DATA-PLANE-R702.6 hot-path receipt: group `lineage_drain`.
//!
//! Budget (spec): p50 3 ms, p99 18 ms, peak memory 64 MiB per request. One
//! request drains 3 valid events, replays them and drains 2 malformed events
//! through `LineageDrain`, the same drain `LineageIngest` runs. Prints p50/p99
//! beside the process RSS growth across the measured loop. Benchmarks never
//! gate a PR.
//!
//! Deterministic: fixed dataset names, no RNG.
//!
//! Run: cargo bench --bench lineage_drain

use std::time::Instant;

use medulla_catalog_types::lineage::{DatasetRef, EtlRun, LineageDrain, LineageEvent};

const WARMUP: usize = 200;
const ITERATIONS: usize = 5_000;

fn event(id: &str, input: &str, output: &str) -> LineageEvent {
    LineageEvent {
        event_id: id.into(),
        run: EtlRun {
            run_id: format!("run-{id}"),
            job: "materialize.engine.t".into(),
            started_at_ms: 1,
            ended_at_ms: Some(2),
        },
        inputs: vec![DatasetRef {
            namespace: "ns".into(),
            name: input.into(),
        }],
        outputs: vec![DatasetRef {
            namespace: "ns".into(),
            name: output.into(),
        }],
        sql_text: None,
        dialect: None,
        catalog_source: None,
    }
}

fn request() -> usize {
    let valid = vec![
        event("a", "x", "y"),
        event("b", "y", "z"),
        event("c", "z", "w"),
    ];
    let malformed = vec![event("d", "bad name", "y"), event("e", "x", "")];
    let mut drain = LineageDrain::new(8);
    let mut pending = valid.clone();
    let first = drain.drain(&mut pending);
    let mut pending = valid;
    let replay = drain.drain(&mut pending);
    assert!(replay.new_edges.is_empty());
    let mut pending = malformed;
    let bad = drain.drain(&mut pending);
    assert_eq!(bad.quarantined.len(), 2);
    first.new_edges.len()
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

/// The process's resident set in MiB (VmRSS), or NaN off Linux.
fn rss_mib() -> f64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return f64::NAN;
    };
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|rest| rest.trim().strip_suffix(" kB"))
        .and_then(|kilobytes| kilobytes.parse::<f64>().ok())
        .map(|kilobytes| kilobytes / 1024.0)
        .unwrap_or(f64::NAN)
}

fn main() {
    for _ in 0..WARMUP {
        assert_eq!(request(), 3);
    }
    let rss_before = rss_mib();
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let edges = request();
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(edges, 3);
    }
    let rss_growth = rss_mib() - rss_before;
    samples.sort_by(f64::total_cmp);
    println!(
        "lineage_drain: p50 {:.4} ms (budget 3), p99 {:.4} ms (budget 18), rss growth {:.2} MiB (budget 64)",
        percentile(&samples, 0.50),
        percentile(&samples, 0.99),
        rss_growth
    );
}
