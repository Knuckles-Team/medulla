//! A published performance-superiority claim (MEDULLA-DURABLE-KERNEL-R042): a
//! benchmark report is accepted as a claim that this engine outperforms a
//! named comparison system only when it publishes its configuration AND
//! carries a passing measured result for that exact comparison. This is the
//! typed-model slice (`.1`): the named comparisons, the claim record, and
//! the refusal for an unpublished-configuration or non-passing claim.
//! Running the benchmark suites themselves is a later child.

use std::fmt;

/// One of the named comparisons MEDULLA-DURABLE-KERNEL-R042 requires a published
/// benchmark suite for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BenchmarkComparison {
    /// YCSB workloads A-F against the async durability class.
    YcsbAsyncDurability,
    /// redis-benchmark p99 against the ephemeral durability class.
    RedisBenchmarkEphemeral,
    /// pgbench and HammerDB TPC-C.
    PgbenchHammerDbTpcc,
    /// LDBC SNB Interactive.
    LdbcSnbInteractive,
    /// A converged graph, vector, SQL and time-series benchmark against
    /// PostgreSQL with AGE, pgvector and TimescaleDB.
    ConvergedGraphVectorSqlTimeseries,
}

/// A benchmark report for one named comparison. Never itself runs a
/// benchmark -- it is the record a benchmark run produces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchmarkClaim {
    pub comparison: BenchmarkComparison,
    /// The exact benchmark configuration (workload parameters, hardware,
    /// versions of the compared systems). Must be non-empty: a claim with no
    /// recorded configuration cannot be reproduced or checked.
    pub configuration: String,
    /// The measured result text (e.g. throughput/latency numbers) for this
    /// exact comparison. Must be non-empty.
    pub measured_result: String,
    /// Whether the measured result actually outperformed the compared
    /// system under the published configuration.
    pub passed: bool,
}

impl BenchmarkClaim {
    /// Accept this report as a superiority claim, or refuse it with the
    /// reason. Never accepts a claim missing its configuration or measured
    /// result, and never accepts a non-passing result as a superiority
    /// claim regardless of how complete its record is.
    pub fn accept_as_superiority_claim(&self) -> Result<(), RefusedBenchmarkClaim> {
        if self.configuration.trim().is_empty() {
            return Err(RefusedBenchmarkClaim::MissingConfiguration);
        }
        if self.measured_result.trim().is_empty() {
            return Err(RefusedBenchmarkClaim::MissingMeasuredResult);
        }
        if !self.passed {
            return Err(RefusedBenchmarkClaim::DidNotPass);
        }
        Ok(())
    }
}

/// Why a benchmark report was refused as a superiority claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusedBenchmarkClaim {
    MissingConfiguration,
    MissingMeasuredResult,
    DidNotPass,
}

impl fmt::Display for RefusedBenchmarkClaim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::MissingConfiguration => "no published configuration",
            Self::MissingMeasuredResult => "no recorded measured result",
            Self::DidNotPass => "measured result did not pass the comparison",
        };
        write!(
            f,
            "benchmark report refused as a superiority claim: {reason}"
        )
    }
}

impl std::error::Error for RefusedBenchmarkClaim {}

// ---------------------------------------------------------------------------
// MEDULLA-DURABLE-KERNEL-R042.100: a benchmark measurement carries the inputs
// that make a throughput and cost figure reproducible, and its arithmetic is
// integer-only so two machines recompute the same number.
// ---------------------------------------------------------------------------

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Schema version carried by every [`BenchmarkMeasurement`].
pub const MEASUREMENT_SCHEMA_VERSION: u32 = 1;

pub const MEASUREMENT_TOKENS_MISSING: &str = "MEASUREMENT_TOKENS_MISSING";
pub const MEASUREMENT_TOKENIZER_MISSING: &str = "MEASUREMENT_TOKENIZER_MISSING";
pub const MEASUREMENT_PRICE_TABLE_MISSING: &str = "MEASUREMENT_PRICE_TABLE_MISSING";
pub const MEASUREMENT_ELAPSED_NOT_POSITIVE: &str = "MEASUREMENT_ELAPSED_NOT_POSITIVE";
pub const MEASUREMENT_CURRENCY_EXPONENT_INVALID: &str = "MEASUREMENT_CURRENCY_EXPONENT_INVALID";
pub const MEASUREMENT_PROFILE_INCOMPARABLE: &str = "MEASUREMENT_PROFILE_INCOMPARABLE";
/// An identity digest field is not 64 lowercase hex characters.
pub const MEASUREMENT_DIGEST_INVALID: &str = "MEASUREMENT_DIGEST_INVALID";

/// Largest minor-unit exponent a price table may declare.
pub const MAX_CURRENCY_EXPONENT: u8 = 6;

/// A resource a benchmark run consumes and the price table prices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    CpuMs,
    BytesRead,
    BytesWritten,
    NetworkBytes,
    ModelTokens,
    StorageByteHours,
}

/// One priced resource: its unit and the price of one unit in millionths of a
/// currency minor unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceEntry {
    pub unit: String,
    pub unit_price: u64,
}

/// A versioned declarative price table, `{resource_kind -> unit, unit_price}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceTable {
    pub version: u32,
    pub currency: String,
    pub minor_unit_exponent: u8,
    pub prices: BTreeMap<ResourceKind, PriceEntry>,
}

impl PriceTable {
    /// Load a table from configuration text and return it with the SHA-256
    /// digest of that exact text (the digest recorded on every receipt).
    pub fn load(config: &str) -> Result<(Self, String), MeasurementRefusal> {
        let table: Self =
            serde_json::from_str(config).map_err(|_| MeasurementRefusal::PriceTableMissing)?;
        if table.minor_unit_exponent > MAX_CURRENCY_EXPONENT {
            return Err(MeasurementRefusal::CurrencyExponentInvalid);
        }
        Ok((table, sha256_hex(config.as_bytes())))
    }

    /// Total cost in minor units: the sum over resource kinds of usage times
    /// `unit_price`, rounded half to even. Resources without a price cost zero.
    pub fn cost_minor_units(&self, usage: &BTreeMap<ResourceKind, u64>) -> u128 {
        let micro: u128 = usage
            .iter()
            .map(|(kind, used)| {
                let price = self.prices.get(kind).map_or(0, |p| p.unit_price);
                u128::from(*used) * u128::from(price)
            })
            .sum();
        round_half_even_div(micro, 1_000_000)
    }
}

fn round_half_even_div(numerator: u128, denominator: u128) -> u128 {
    let quotient = numerator / denominator;
    let twice_rem = (numerator % denominator) * 2;
    if twice_rem > denominator || (twice_rem == denominator && quotient % 2 == 1) {
        quotient + 1
    } else {
        quotient
    }
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Why a measurement was refused admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasurementRefusal {
    TokensMissing,
    TokenizerMissing,
    PriceTableMissing,
    ElapsedNotPositive,
    CurrencyExponentInvalid,
    ProfileIncomparable,
    DigestInvalid,
}

impl MeasurementRefusal {
    /// The stable refusal code printed by `bench-claim admit`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::TokensMissing => MEASUREMENT_TOKENS_MISSING,
            Self::TokenizerMissing => MEASUREMENT_TOKENIZER_MISSING,
            Self::PriceTableMissing => MEASUREMENT_PRICE_TABLE_MISSING,
            Self::ElapsedNotPositive => MEASUREMENT_ELAPSED_NOT_POSITIVE,
            Self::CurrencyExponentInvalid => MEASUREMENT_CURRENCY_EXPONENT_INVALID,
            Self::ProfileIncomparable => MEASUREMENT_PROFILE_INCOMPARABLE,
            Self::DigestInvalid => MEASUREMENT_DIGEST_INVALID,
        }
    }
}

impl fmt::Display for MeasurementRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for MeasurementRefusal {}

/// One measured benchmark run with the identity and revision fields that make
/// it reproducible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BenchmarkMeasurement {
    pub schema_version: u32,
    // Identity.
    pub benchmark_case: String,
    pub workload: String,
    pub profile: String,
    pub configuration_digest: String,
    pub dataset_digest: String,
    pub query_digest: String,
    pub release_commit: String,
    pub hardware_digest: String,
    // Revision.
    pub model: String,
    pub provider: String,
    pub tokenizer: String,
    pub price_table_digest: String,
    pub minor_unit_exponent: u8,
    // Timing.
    pub request_start_ms: u64,
    pub request_end_ms: u64,
    pub model_ready_ms: u64,
    pub inference_elapsed_ms: u64,
    // Count.
    pub full_input_tokens: u64,
    // Usage priced by the table.
    pub usage: BTreeMap<ResourceKind, u64>,
}

impl BenchmarkMeasurement {
    /// Admit this measurement or refuse it with the first failing code.
    pub fn validate(&self) -> Result<(), MeasurementRefusal> {
        if self.full_input_tokens == 0 {
            return Err(MeasurementRefusal::TokensMissing);
        }
        if self.tokenizer.trim().is_empty() {
            return Err(MeasurementRefusal::TokenizerMissing);
        }
        if !is_digest(&self.price_table_digest) {
            return Err(MeasurementRefusal::PriceTableMissing);
        }
        if self.inference_elapsed_ms == 0 {
            return Err(MeasurementRefusal::ElapsedNotPositive);
        }
        if self.minor_unit_exponent > MAX_CURRENCY_EXPONENT {
            return Err(MeasurementRefusal::CurrencyExponentInvalid);
        }
        let identity = [
            &self.configuration_digest,
            &self.dataset_digest,
            &self.query_digest,
            &self.hardware_digest,
        ];
        if !identity.into_iter().all(|d| is_digest(d)) {
            return Err(MeasurementRefusal::DigestInvalid);
        }
        Ok(())
    }

    /// Full input tokens per second, `full_input_tokens * 1000 / elapsed`
    /// (integer, truncated).
    pub fn throughput_tokens_per_second(&self) -> u64 {
        (u128::from(self.full_input_tokens) * 1000 / u128::from(self.inference_elapsed_ms.max(1)))
            as u64
    }

    /// Total cost in minor units under `table`; the table must be the one this
    /// measurement names by digest.
    pub fn cost_minor_units(&self, table: &PriceTable) -> u128 {
        table.cost_minor_units(&self.usage)
    }

    /// Two measurements compare only when benchmark, workload and profile match.
    pub fn comparable_with(&self, other: &Self) -> Result<(), MeasurementRefusal> {
        if self.benchmark_case == other.benchmark_case
            && self.workload == other.workload
            && self.profile == other.profile
        {
            Ok(())
        } else {
            Err(MeasurementRefusal::ProfileIncomparable)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(configuration: &str, measured_result: &str, passed: bool) -> BenchmarkClaim {
        BenchmarkClaim {
            comparison: BenchmarkComparison::YcsbAsyncDurability,
            configuration: configuration.to_string(),
            measured_result: measured_result.to_string(),
            passed,
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R042.1
    #[test]
    fn a_published_passing_report_is_accepted() {
        claim("workload A, 8 shards", "p99 3.2ms vs 5.1ms", true)
            .accept_as_superiority_claim()
            .unwrap();
    }

    // spec: MEDULLA-DURABLE-KERNEL-R042.1
    #[test]
    fn missing_configuration_is_refused() {
        let err = claim("", "p99 3.2ms", true)
            .accept_as_superiority_claim()
            .unwrap_err();
        assert_eq!(err, RefusedBenchmarkClaim::MissingConfiguration);
    }

    // spec: MEDULLA-DURABLE-KERNEL-R042.1
    #[test]
    fn missing_measured_result_is_refused() {
        let err = claim("workload A", "   ", true)
            .accept_as_superiority_claim()
            .unwrap_err();
        assert_eq!(err, RefusedBenchmarkClaim::MissingMeasuredResult);
    }

    // spec: MEDULLA-DURABLE-KERNEL-R042.1
    #[test]
    fn a_non_passing_report_is_never_a_superiority_claim() {
        let err = claim("workload A", "p99 9.9ms vs 5.1ms", false)
            .accept_as_superiority_claim()
            .unwrap_err();
        assert_eq!(err, RefusedBenchmarkClaim::DidNotPass);
        assert!(err.to_string().contains("did not pass"));
    }

    // spec: MEDULLA-DURABLE-KERNEL-R042.100.1
    #[test]
    fn throughput_and_cost_match_hand_computed_values() {
        let config = r#"{"version":1,"currency":"USD","minor_unit_exponent":2,"prices":{
            "cpu_ms":{"unit":"ms","unit_price":500000},
            "model_tokens":{"unit":"token","unit_price":2500}}}"#;
        let (table, digest) = PriceTable::load(config).unwrap();
        assert_eq!(digest, sha256_hex(config.as_bytes()));
        assert_eq!(digest.len(), 64);
        let m = measurement(&digest);
        // 12_000 tokens * 1000 / 4_000 ms = 3_000 tokens per second.
        assert_eq!(m.throughput_tokens_per_second(), 3_000);
        // 10 cpu_ms * 0.5 + 12_000 tokens * 0.0025 = 5 + 30 = 35 minor units.
        assert_eq!(m.cost_minor_units(&table), 35);
        m.validate().unwrap();
        // Round half to even: 0.5 -> 0, 1.5 -> 2, 2.5 -> 2, 2.6 -> 3.
        assert_eq!(round_half_even_div(500_000, 1_000_000), 0);
        assert_eq!(round_half_even_div(1_500_000, 1_000_000), 2);
        assert_eq!(round_half_even_div(2_500_000, 1_000_000), 2);
        assert_eq!(round_half_even_div(2_600_000, 1_000_000), 3);
    }

    pub(super) fn measurement(price_digest: &str) -> BenchmarkMeasurement {
        let d = |c: char| c.to_string().repeat(64);
        BenchmarkMeasurement {
            schema_version: MEASUREMENT_SCHEMA_VERSION,
            benchmark_case: "ycsb-a".into(),
            workload: "mixed".into(),
            profile: "gpu-1".into(),
            configuration_digest: d('a'),
            dataset_digest: d('b'),
            query_digest: d('c'),
            release_commit: "0123abc".into(),
            hardware_digest: d('d'),
            model: "m".into(),
            provider: "p".into(),
            tokenizer: "tok-1".into(),
            price_table_digest: price_digest.into(),
            minor_unit_exponent: 2,
            request_start_ms: 0,
            request_end_ms: 4_100,
            model_ready_ms: 50,
            inference_elapsed_ms: 4_000,
            full_input_tokens: 12_000,
            usage: BTreeMap::from([
                (ResourceKind::CpuMs, 10),
                (ResourceKind::ModelTokens, 12_000),
            ]),
        }
    }
}
