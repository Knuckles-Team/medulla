//! Latency summary and receipt shape for the wire protocol benchmark
//! (MEDULLA-DURABLE-KERNEL-R042.5). One receipt per protocol carries p50, p99,
//! the concurrent session count, the budget and whether the budget held.

use std::io;
use std::path::{Path, PathBuf};

use crate::claim::write_receipt;

/// One protocol's measured case.
pub struct WireCase {
    /// Receipt case name, also the file stem.
    pub protocol: &'static str,
    /// What one sample measures.
    pub operation: &'static str,
    /// Concurrent client sessions that produced the samples.
    pub sessions: usize,
    /// p99 budget in milliseconds.
    pub p99_budget_ms: f64,
    /// Per-operation latencies in milliseconds.
    pub samples_ms: Vec<f64>,
}

/// The nearest-rank percentile of an ascending-sorted slice.
pub fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

impl WireCase {
    pub fn p50_ms(&self) -> f64 {
        percentile(&sorted(&self.samples_ms), 0.50)
    }

    pub fn p99_ms(&self) -> f64 {
        percentile(&sorted(&self.samples_ms), 0.99)
    }

    /// `true` when there are samples and the p99 is inside the budget.
    pub fn within_budget(&self) -> bool {
        !self.samples_ms.is_empty() && self.p99_ms() <= self.p99_budget_ms
    }

    pub fn receipt(&self) -> serde_json::Value {
        serde_json::json!({
            "case": self.protocol,
            "operation": self.operation,
            "session_count": self.sessions,
            "sample_count": self.samples_ms.len(),
            "p50_ms": self.p50_ms(),
            "p99_ms": self.p99_ms(),
            "p99_budget_ms": self.p99_budget_ms,
            "within_budget": self.within_budget(),
        })
    }
}

fn sorted(samples: &[f64]) -> Vec<f64> {
    let mut copy = samples.to_vec();
    copy.sort_by(f64::total_cmp);
    copy
}

/// Write one receipt per case to `<root>/<release>/<protocol>.json`.
pub fn write_wire_receipts(
    root: &Path,
    release: &str,
    cases: &[WireCase],
) -> io::Result<Vec<PathBuf>> {
    cases
        .iter()
        .map(|case| write_receipt(root, release, case.protocol, &case.receipt()))
        .collect()
}
