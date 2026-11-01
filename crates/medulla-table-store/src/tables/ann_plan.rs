//! ANN plan types shared by the SQL planner and the table store: the pgvector metric and
//! method, the registered ANN index plan and the hypertable plan, plus the metric mapping.

use medulla_ann::Metric;
use serde::{Deserialize, Serialize};

/// The pgvector distance metric an ANN index / query uses (CONCEPT:MEDULLA-KG.query.real-ann-top-k), keyed off
/// the opclass (`vector_l2_ops` → `L2`, …) and the distance operator (`<->` → `L2`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorMetric {
    /// L2 (Euclidean) — opclass `vector_l2_ops`, operator `<->`.
    L2,
    /// Cosine — opclass `vector_cosine_ops`, operator `<=>`.
    Cosine,
    /// Negative inner product — opclass `vector_ip_ops`, operator `<#>`.
    InnerProduct,
}

/// The ANN index method (CONCEPT:MEDULLA-KG.query.real-ann-top-k). Both lower onto the same `medulla-ann` IVF-PQ /
/// HNSW backend; the spelling is recorded for catalog fidelity + `EXPLAIN`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnnMethod {
    Hnsw,
    IvfFlat,
}

/// A decoded `CREATE INDEX [IF NOT EXISTS] [name] ON table USING hnsw|ivfflat
/// (col opclass)` (CONCEPT:MEDULLA-KG.query.real-ann-top-k) — an ANN index registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnIndexPlan {
    pub name: Option<String>,
    pub table: String,
    pub column: String,
    pub method: AnnMethod,
    pub metric: VectorMetric,
    pub if_not_exists: bool,
}

/// A decoded `SELECT create_hypertable('t','ts'[, …])` (CONCEPT:MEDULLA-KG.query.continuous-aggregate-lowering) — the
/// time-partitioning declaration persisted in the native SQL catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HypertablePlan {
    pub table: String,
    pub time_column: String,
}

/// Map a pgvector [`VectorMetric`] (from the opclass / distance operator) to the
/// medulla-ann [`Metric`] (CONCEPT:MEDULLA-KG.query.real-pgvector-ann-top). `<->`→L2, `<=>`→cosine, `<#>`→(neg)inner.
pub fn metric_to_ann(metric: VectorMetric) -> Metric {
    match metric {
        VectorMetric::L2 => Metric::L2,
        VectorMetric::Cosine => Metric::Cosine,
        VectorMetric::InnerProduct => Metric::InnerProduct,
    }
}

/// Integer square root (floor) — used to size the IVF `nlist` ≈ √N without pulling a
/// nightly `usize::isqrt`.
pub fn isqrt(n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let mut x = (n as f64).sqrt() as usize;
    while x.saturating_mul(x) > n {
        x -= 1;
    }
    while (x + 1).saturating_mul(x + 1) <= n {
        x += 1;
    }
    x
}
