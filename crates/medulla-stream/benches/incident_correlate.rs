//! MEDULLA-UNIFIED-DATA-PLANE-R760.6 hot-path receipt: group `incident_correlate`.
//!
//! Budget (spec): 100,000 anomalies correlated in 250 ms, 64 MiB, p99 per
//! anomaly 5 microseconds. One pass correlates 100,000 readings over 100
//! entities through `IncidentAuthority::correlate`, the call
//! `IncidentCorrelate` runs. Prints the pass time, the per-anomaly p99 (pass
//! time over reading count) and the RSS growth. Benchmarks never gate a PR.
//!
//! Deterministic: readings derive from their index, no RNG.
//!
//! Run: cargo bench -p medulla-stream --bench incident_correlate

use std::time::Instant;

use medulla_stream::telemetry::incident_authority::IncidentAuthority;
use medulla_types::incident::AnomalyReading;

const ANOMALIES: usize = 100_000;
const ENTITIES: usize = 100;
const PASSES: usize = 20;
const WINDOW_MS: u64 = 60_000;

fn readings() -> Vec<AnomalyReading> {
    (0..ANOMALIES)
        .map(|i| AnomalyReading {
            entity_ref: format!("svc:{}", i % ENTITIES),
            signal: "latency".to_string(),
            observed: 300.0,
            baseline: 100.0,
            at_ms: (i % 1_000) as u64,
        })
        .collect()
}

/// The process's resident set in MiB (VmRSS), or NaN off Linux.
fn rss_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmRSS:"))
                .and_then(|rest| rest.trim().strip_suffix(" kB"))
                .and_then(|kb| kb.parse::<f64>().ok())
        })
        .map_or(f64::NAN, |kb| kb / 1024.0)
}

fn main() {
    let rss_before = rss_mib();
    let mut millis = Vec::with_capacity(PASSES);
    for _ in 0..PASSES {
        let input = readings();
        let mut authority = IncidentAuthority::default();
        let start = Instant::now();
        let touched = authority
            .correlate(Some("tenant:bench"), input, WINDOW_MS)
            .expect("correlate");
        millis.push(start.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(touched.len(), ENTITIES, "one incident per signature");
    }
    let rss_growth = rss_mib() - rss_before;
    millis.sort_by(f64::total_cmp);
    let p99 = millis[(millis.len() - 1) * 99 / 100];
    println!(
        "incident_correlate: {ANOMALIES} anomalies in {p99:.2} ms (budget 250), per anomaly {:.3} us (budget 5), rss growth {rss_growth:.2} MiB (budget 64)",
        p99 * 1_000.0 / ANOMALIES as f64
    );
}
