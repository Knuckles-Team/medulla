//! Receipt shape for the cross-modal query benchmark (MEDULLA-DURABLE-KERNEL-R042.2), written
//! through `claim::write_receipt` and carrying hardware and build profile. Included by `#[path]`
//! next to `claim.rs`, `latency_stats.rs` and `receipt_fields.rs`.

use std::io;
use std::path::{Path, PathBuf};

use crate::claim::write_receipt;
use crate::latency_stats::{percentile, sorted};
use crate::receipt_fields::{hardware_identity, refuse, RECEIPT_FIELD_MISSING};

/// The target recorded after the first nightly run is this multiple of the measured p99.
pub const TARGET_P99_FACTOR: f64 = 1.2;

/// One cross-modal query latency case.
pub struct CrossModalCase {
    pub case: String,
    /// Digest of the benchmark configuration (corpus sizes, dimensions, seed, query program).
    pub configuration_digest: String,
    /// Digest of the generated corpus.
    pub corpus_digest: String,
    pub claims: u64,
    pub edges: u64,
    pub vectors: u64,
    pub dimensions: u32,
    pub p50_budget_ms: f64,
    pub p99_budget_ms: f64,
    /// Per-query latencies in milliseconds.
    pub samples_ms: Vec<f64>,
}

impl CrossModalCase {
    pub fn p50_ms(&self) -> f64 {
        percentile(&sorted(&self.samples_ms), 0.50)
    }

    pub fn p99_ms(&self) -> f64 {
        percentile(&sorted(&self.samples_ms), 0.99)
    }

    /// `true` when there are samples and both percentiles are inside their budgets.
    pub fn within_budget(&self) -> bool {
        !self.samples_ms.is_empty()
            && self.p50_ms() <= self.p50_budget_ms
            && self.p99_ms() <= self.p99_budget_ms
    }

    pub fn receipt(&self) -> io::Result<serde_json::Value> {
        if self.case.is_empty()
            || self.configuration_digest.is_empty()
            || self.corpus_digest.is_empty()
            || self.samples_ms.is_empty()
        {
            return Err(refuse(
                RECEIPT_FIELD_MISSING,
                "case, configuration digest, corpus digest and samples are required",
            ));
        }
        Ok(serde_json::json!({
            "case": self.case,
            "configuration_digest": self.configuration_digest,
            "corpus_digest": self.corpus_digest,
            "corpus": {
                "claims": self.claims,
                "edges": self.edges,
                "vectors": self.vectors,
                "dimensions": self.dimensions,
            },
            "hardware": hardware_identity(),
            "sample_count": self.samples_ms.len(),
            "p50_ms": self.p50_ms(),
            "p99_ms": self.p99_ms(),
            "p50_budget_ms": self.p50_budget_ms,
            "p99_budget_ms": self.p99_budget_ms,
            "within_budget": self.within_budget(),
        }))
    }
}

/// Write one cross-modal receipt to `<root>/<release>/<case>.json`.
pub fn write_cross_modal_receipt(
    root: &Path,
    release: &str,
    case: &CrossModalCase,
) -> io::Result<PathBuf> {
    write_receipt(root, release, &case.case, &case.receipt()?)
}

/// The p99 target recorded after the first nightly run: 1.2 times the measured p99.
pub fn target_from_first_run(p99_ms: f64) -> f64 {
    p99_ms * TARGET_P99_FACTOR
}

/// Write `<root>/targets.json` mapping case to its p99 target, keeping targets already
/// recorded for other cases (a target is set by the first run only).
pub fn record_target(root: &Path, case: &str, p99_ms: f64) -> io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let path = root.join("targets.json");
    let mut targets: serde_json::Map<String, serde_json::Value> =
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(io::Error::other)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => serde_json::Map::new(),
            Err(e) => return Err(e),
        };
    targets
        .entry(case.to_string())
        .or_insert_with(|| serde_json::json!(target_from_first_run(p99_ms)));
    let text = serde_json::to_string_pretty(&targets).map_err(io::Error::other)?;
    std::fs::write(&path, text)?;
    Ok(path)
}
