// CONCEPT:MEDULLA-KG.domains.finance-compute — Rust-Native Finance Compute Suite
//
// Feature-gated financial compute primitives served over the Tokio service
// protocol (MessagePack/UDS), not an in-process Python extension.
// Replaces Python scipy/statsmodels/hmmlearn for hot-path finance ops.

// Numeric linear-algebra/optimization code (Kalman, simplex, covariance, regime
// transition matrices) reads more clearly — and is less error-prone to maintain —
// with explicit `for i in 0..n { m[i][j] }` indexing than with enumerate/zip
// rewrites. Scope the lint to this compute module rather than contorting the math.
#![allow(clippy::needless_range_loop)]

pub mod market;
pub mod signals;
