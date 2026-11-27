//! Receipt shape for the shape-memoization benchmark (MEDULLA-DURABLE-KERNEL-R064), written
//! through `claim::write_receipt` and carrying hardware and build profile. Included by `#[path]`
//! next to `claim.rs`, `latency_stats.rs` and `receipt_fields.rs`.

use std::io;
use std::path::{Path, PathBuf};

use crate::claim::write_receipt;
use crate::latency_stats::{percentile, sorted};
use crate::receipt_fields::{hardware_identity, refuse, RECEIPT_FIELD_MISSING};

/// Refusal code: the shape receipt lacks the memoized or the parse-per-write variant.
pub const SHAPE_RECEIPT_VARIANT_MISSING: &str = "SHAPE_RECEIPT_VARIANT_MISSING";
/// Memoization is adopted only when its p50 is at least this fraction lower.
pub const SHAPE_ADOPTION_MIN_P50_WIN: f64 = 0.20;

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
