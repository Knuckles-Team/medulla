//! Source reconcile at one million identifiers (MEDULLA-INGEST-WORKER-R903.6).
//!
//! Nightly bench group `ingest/source_reconcile`: an enumeration of 990,000
//! identifiers is compared with 1,000,000 held records through
//! `plan_reconcile_digests`. Budget: under 2 s for the comparison and a peak
//! resident memory (`VmHWM`) under 256 MiB. The run exits non-zero when a budget
//! is exceeded. A service level, not a pull request gate.
//!
// spec: MEDULLA-INGEST-WORKER-R903.6
//!
//! Run: cargo bench --bench ingest_source_reconcile

use std::time::{Duration, Instant};

use medulla_types::contract::Digest256;
use medulla_types::ingest_rules::plan_reconcile_digests;

const HELD: usize = 1_000_000;
/// Every hundredth held record is missing from the enumeration: 1 percent, below the
/// mass-delete guard.
const MISSING_EVERY: usize = 100;
const BUDGET: Duration = Duration::from_secs(2);
const BUDGET_MIB: f64 = 256.0;

fn digest(index: usize) -> [u8; 32] {
    let framed = Digest256::framed(b"bench/source-reconcile", &[&index.to_be_bytes()])
        .expect("bench digest frames");
    *framed.as_bytes()
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
    let mut held: Vec<[u8; 32]> = (0..HELD).map(digest).collect();
    held.sort_unstable();
    let mut enumerated: Vec<[u8; 32]> = (0..HELD)
        .filter(|index| index % MISSING_EVERY != 0)
        .map(digest)
        .collect();
    enumerated.sort_unstable();
    let expected = HELD / MISSING_EVERY;

    let started = Instant::now();
    let missing = plan_reconcile_digests(&held, &enumerated, true, false)
        .expect("a 1 percent plan is under the mass-delete guard");
    let elapsed = started.elapsed();
    let peak = high_water_mib();

    println!(
        "ingest/source_reconcile: {HELD} held, {} enumerated, {} missing in {:?}; peak {:.1} MiB",
        enumerated.len(),
        missing.len(),
        elapsed,
        peak
    );
    assert_eq!(missing.len(), expected, "the plan names every dropped record");
    assert!(elapsed < BUDGET, "reconcile took {elapsed:?}, budget {BUDGET:?}");
    assert!(
        peak < BUDGET_MIB,
        "peak {peak:.1} MiB exceeds the {BUDGET_MIB} MiB budget"
    );
}
