//! Sensitive column scan (MEDULLA-TYPED-PACKS-R803.6).
//!
//! Nightly bench group `decide/sensitive_scan`: `scan_values` scans 10,000 values against
//! 20 pattern detectors. Budget per scan: p99 below 15 ms and peak resident memory below
//! 16 MiB over the process baseline. The run exits non-zero when a budget is exceeded. A
//! service level, never a pull request gate; the gate is the served profile journey.
//!
// spec: MEDULLA-TYPED-PACKS-R803.6
//!
//! Run: cargo bench -p medulla-core --bench sensitive_scan

use std::time::{Duration, Instant};

use medulla_core::sensitive_scan::{scan_values, NoResources, ScanLimits};
use medulla_query_types::sensitive_detect::{
    DetectorRule, SensitiveDetectionMethod, SensitiveDetector,
};

const VALUES: usize = 10_000;
const DETECTORS: usize = 20;
const SCANS: usize = 100;
const BUDGET_P99: Duration = Duration::from_millis(15);
const BUDGET_MIB: f64 = 16.0;

fn detectors() -> Vec<SensitiveDetector> {
    (0..DETECTORS)
        .map(|n| SensitiveDetector {
            id: format!("detector:bench-{n}"),
            category: format!("bench_{n}"),
            method: SensitiveDetectionMethod::Pattern,
            rule: DetectorRule::Pattern {
                regex: format!("^[a-z]+{n}@example\\.test$"),
            },
            threshold: 0.8,
        })
        .collect()
}

/// Resident high-water mark in MiB (`VmHWM`), 0 where procfs is absent.
fn high_water_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1)?.parse::<f64>().ok())
        })
        .map_or(0.0, |kib| kib / 1024.0)
}

fn main() {
    let owned: Vec<String> = (0..VALUES)
        .map(|n| format!("person{}@example.test", n % 20))
        .collect();
    let values: Vec<Option<&str>> = owned.iter().map(|value| Some(value.as_str())).collect();
    let detectors = detectors();
    let limits = ScanLimits::default();
    let baseline = high_water_mib();
    let mut samples = Vec::with_capacity(SCANS);
    for _ in 0..SCANS {
        let started = Instant::now();
        let detections = scan_values(
            "col:bench.value",
            &values,
            &detectors,
            &limits,
            &NoResources,
            0,
        )
        .expect("bench detectors are valid");
        samples.push(started.elapsed());
        assert_eq!(detections.len(), DETECTORS);
    }
    samples.sort();
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    let used = high_water_mib() - baseline;
    println!(
        "decide/sensitive_scan p99 = {p99:?} (budget {BUDGET_P99:?}) peak_mib = {used:.1} (budget {BUDGET_MIB})"
    );
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
    assert!(
        used < BUDGET_MIB,
        "peak {used:.1} MiB exceeds {BUDGET_MIB} MiB"
    );
}
