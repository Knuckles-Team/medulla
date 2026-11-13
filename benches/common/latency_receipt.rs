//! Receipt shapes for the cross-modal query benchmark (MEDULLA-DURABLE-KERNEL-R042.2)
//! and the shape-memoization benchmark (MEDULLA-DURABLE-KERNEL-R064). Both are
//! written through `claim::write_receipt` and carry hardware and build profile.
//! Included by `#[path]` next to `claim.rs` and `wire_receipt.rs`.

use std::io;
use std::path::{Path, PathBuf};

use crate::claim::write_receipt;
use crate::wire_receipt::percentile;

/// Refusal code: a receipt with an empty digest, case or sample set is never written.
pub const RECEIPT_FIELD_MISSING: &str = "BENCH_RECEIPT_FIELD_MISSING";
/// Refusal code: the shape receipt lacks the memoized or the parse-per-write variant.
pub const SHAPE_RECEIPT_VARIANT_MISSING: &str = "SHAPE_RECEIPT_VARIANT_MISSING";
/// Memoization is adopted only when its p50 is at least this fraction lower.
pub const SHAPE_ADOPTION_MIN_P50_WIN: f64 = 0.20;
/// The target recorded after the first nightly run is this multiple of the measured p99.
pub const TARGET_P99_FACTOR: f64 = 1.2;

fn refuse(code: &str, what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("{code}: {what}"))
}

fn sorted(samples: &[f64]) -> Vec<f64> {
    let mut copy = samples.to_vec();
    copy.sort_by(f64::total_cmp);
    copy
}

/// Hardware and build identity every receipt carries.
pub fn hardware_identity() -> serde_json::Value {
    serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "logical_cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
    })
}

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

/// The shape-validation benchmark: memoized and parse-per-write variants over the same writes.
pub struct ShapeBenchCase {
    pub writes: usize,
    pub shapes: usize,
    pub profile: String,
    pub memoized_ms: Vec<f64>,
    pub parse_per_write_ms: Vec<f64>,
}

impl ShapeBenchCase {
    fn p50(samples: &[f64]) -> f64 {
        percentile(&sorted(samples), 0.50)
    }

    /// Fractional p50 reduction of the memoized variant against parse-per-write.
    pub fn p50_win(&self) -> f64 {
        let parse = Self::p50(&self.parse_per_write_ms);
        if parse <= 0.0 {
            return 0.0;
        }
        (parse - Self::p50(&self.memoized_ms)) / parse
    }

    /// Memoization is adopted only with a measured win of at least 20 percent on p50.
    pub fn adopt_memoized(&self) -> bool {
        !self.memoized_ms.is_empty()
            && !self.parse_per_write_ms.is_empty()
            && self.p50_win() >= SHAPE_ADOPTION_MIN_P50_WIN
    }

    pub fn receipt(&self) -> io::Result<serde_json::Value> {
        if self.memoized_ms.is_empty() || self.parse_per_write_ms.is_empty() {
            return Err(refuse(
                SHAPE_RECEIPT_VARIANT_MISSING,
                "both the memoized and the parse-per-write variants are required",
            ));
        }
        if self.profile.is_empty() {
            return Err(refuse(RECEIPT_FIELD_MISSING, "profile is required"));
        }
        let variant = |samples: &[f64]| {
            let s = sorted(samples);
            serde_json::json!({
                "sample_count": s.len(),
                "p50_ms": percentile(&s, 0.50),
                "p99_ms": percentile(&s, 0.99),
            })
        };
        Ok(serde_json::json!({
            "case": "shape_validation",
            "writes": self.writes,
            "shapes": self.shapes,
            "profile": self.profile,
            "hardware": hardware_identity(),
            "memoized": variant(&self.memoized_ms),
            "parse_per_write": variant(&self.parse_per_write_ms),
            "p50_win": self.p50_win(),
            "adopt_memoized": self.adopt_memoized(),
        }))
    }
}

/// Write the shape receipt to `<root>/<release>/shape_validation.json`.
pub fn write_shape_receipt(
    root: &Path,
    release: &str,
    case: &ShapeBenchCase,
) -> io::Result<PathBuf> {
    write_receipt(root, release, "shape_validation", &case.receipt()?)
}
