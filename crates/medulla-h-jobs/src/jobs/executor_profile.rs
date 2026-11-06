//! Column profiling for the `DatasetProfile` job kind
//! (MEDULLA-UNIFIED-DATA-PLANE-R761.2): pushed aggregates where the source
//! dialect allows them, a reservoir sample otherwise, a HyperLogLog sketch for
//! distinct counts.

use std::collections::HashMap;

use medulla_compute::sketch::{HyperLogLog, HLL_DEFAULT_PRECISION};
use medulla_types::profile::{ColumnProfile, HistogramBucket, ProfileMethod, TopValue};

/// Refusal code: the source supports neither aggregate pushdown nor sampling
/// (MEDULLA-UNIFIED-DATA-PLANE-R761.4).
pub(super) const PROFILE_UNSUPPORTED: &str = "PROFILE_UNSUPPORTED";

/// The SQL dialect of a profiled source; only the listed dialects take a
/// pushed aggregate statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProfileDialect {
    Postgres,
    Mysql,
    /// A native dataset or a source without aggregate pushdown.
    Other,
}

impl ProfileDialect {
    fn pushes_aggregates(self) -> bool {
        matches!(self, Self::Postgres | Self::Mysql)
    }
}

/// The result of the one pushed aggregate statement.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct AggregateRow {
    pub rows: u64,
    pub non_null: u64,
    pub min: Option<String>,
    pub max: Option<String>,
}

/// What the profiler reads from one dataset.
pub(super) trait ProfileSource {
    /// The dataset's catalog name.
    fn dataset(&self) -> &str;
    /// The source dialect.
    fn dialect(&self) -> ProfileDialect;
    /// Run one aggregate statement through the federated pushdown path.
    fn push_aggregate(&self, statement: &str) -> Result<AggregateRow, String>;
    /// Whether [`Self::scan_column`] is available.
    fn supports_sampling(&self) -> bool;
    /// Stream every value of `column` (`None` = SQL NULL).
    fn scan_column(&self, column: &str, visit: &mut dyn FnMut(Option<&str>)) -> Result<(), String>;
}

/// Profiling knobs (`profile.sample_rows`, `profile.top_k`,
/// `profile.histogram_buckets`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProfileConfig {
    pub sample_rows: u64,
    pub top_k: usize,
    pub histogram_buckets: usize,
    pub profiled_at: i64,
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            sample_rows: 100_000,
            top_k: 10,
            histogram_buckets: 20,
            profiled_at: 0,
        }
    }
}

/// Why a column could not be profiled; renders as `CODE: detail`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProfileRefusal {
    pub code: &'static str,
    pub detail: String,
}

impl std::fmt::Display for ProfileRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

fn unsupported(detail: impl Into<String>) -> ProfileRefusal {
    ProfileRefusal {
        code: PROFILE_UNSUPPORTED,
        detail: detail.into(),
    }
}

/// Quote a column identifier for the pushed statement.
fn quote_ident(dialect: ProfileDialect, column: &str) -> String {
    match dialect {
        ProfileDialect::Mysql => format!("`{}`", column.replace('`', "``")),
        _ => format!("\"{}\"", column.replace('"', "\"\"")),
    }
}

/// The one pushed aggregate statement for `column` of `dataset`.
pub(super) fn aggregate_statement(dialect: ProfileDialect, dataset: &str, column: &str) -> String {
    let col = quote_ident(dialect, column);
    format!(
        "SELECT COUNT(*), COUNT({col}), MIN({col}), MAX({col}) FROM {}",
        quote_ident(dialect, dataset)
    )
}

/// Deterministic xorshift generator for the reservoir draw.
struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// Values drawn from one scan: the reservoir plus the exact row and null counts.
struct Drawn {
    rows: u64,
    nulls: u64,
    reservoir: Vec<String>,
}

fn draw_reservoir(
    source: &dyn ProfileSource,
    column: &str,
    capacity: u64,
) -> Result<Drawn, ProfileRefusal> {
    let capacity = capacity.max(1) as usize;
    let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
    let mut drawn = Drawn {
        rows: 0,
        nulls: 0,
        reservoir: Vec::with_capacity(capacity.min(1 << 16)),
    };
    let mut seen_non_null: u64 = 0;
    source
        .scan_column(column, &mut |value| {
            drawn.rows += 1;
            let Some(value) = value else {
                drawn.nulls += 1;
                return;
            };
            seen_non_null += 1;
            if drawn.reservoir.len() < capacity {
                drawn.reservoir.push(value.to_string());
            } else {
                let slot = (rng.next() % seen_non_null) as usize;
                if slot < capacity {
                    drawn.reservoir[slot] = value.to_string();
                }
            }
        })
        .map_err(unsupported)?;
    Ok(drawn)
}

fn top_values(values: &[String], k: usize) -> Vec<TopValue> {
    let mut counts: HashMap<&str, u64> = HashMap::new();
    for value in values {
        *counts.entry(value.as_str()).or_default() += 1;
    }
    let mut ranked: Vec<(&str, u64)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    ranked.truncate(k);
    ranked
        .into_iter()
        .map(|(value, count)| TopValue {
            value: value.to_string(),
            count,
        })
        .collect()
}

/// Equal-width histogram when every sampled value is numeric; none otherwise.
fn histogram(values: &[String], buckets: usize) -> Vec<HistogramBucket> {
    if buckets == 0 || values.is_empty() {
        return Vec::new();
    }
    let mut numbers = Vec::with_capacity(values.len());
    for value in values {
        match value.trim().parse::<f64>() {
            Ok(n) if n.is_finite() => numbers.push(n),
            _ => return Vec::new(),
        }
    }
    let lo = numbers.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = numbers.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let width = if hi > lo {
        (hi - lo) / buckets as f64
    } else {
        1.0
    };
    let mut counts = vec![0u64; buckets];
    for n in numbers {
        let idx = (((n - lo) / width) as usize).min(buckets - 1);
        counts[idx] += 1;
    }
    counts
        .into_iter()
        .enumerate()
        .map(|(i, count)| HistogramBucket {
            lower: (lo + width * i as f64).to_string(),
            upper: (lo + width * (i + 1) as f64).to_string(),
            count,
        })
        .collect()
}

fn min_max(values: &[String]) -> (Option<String>, Option<String>) {
    (values.iter().min().cloned(), values.iter().max().cloned())
}

/// Profile one column (MEDULLA-UNIFIED-DATA-PLANE-R761.2).
pub(super) fn profile_column(
    source: &dyn ProfileSource,
    column: &str,
    cfg: &ProfileConfig,
) -> Result<ColumnProfile, ProfileRefusal> {
    let pushdown = source.dialect().pushes_aggregates();
    if !pushdown && !source.supports_sampling() {
        return Err(unsupported(format!(
            "dataset {} supports neither aggregate pushdown nor sampling",
            source.dataset()
        )));
    }
    let drawn = if source.supports_sampling() {
        Some(draw_reservoir(source, column, cfg.sample_rows)?)
    } else {
        None
    };
    let sample: &[String] = drawn
        .as_ref()
        .map(|d| d.reservoir.as_slice())
        .unwrap_or(&[]);
    let mut hll = HyperLogLog::new(HLL_DEFAULT_PRECISION);
    for value in sample {
        hll.insert(value.as_str());
    }
    let (rows, nulls, min, max, method) = if pushdown {
        let statement = aggregate_statement(source.dialect(), source.dataset(), column);
        let agg = source.push_aggregate(&statement).map_err(unsupported)?;
        let nulls = agg.rows.saturating_sub(agg.non_null);
        (agg.rows, nulls, agg.min, agg.max, ProfileMethod::Pushdown)
    } else {
        let drawn = drawn.as_ref().expect("sampling checked above");
        let (min, max) = min_max(sample);
        (
            drawn.rows,
            drawn.nulls,
            min,
            max,
            ProfileMethod::Sample {
                size: sample.len() as u64,
            },
        )
    };
    let distinct_estimate = (hll.estimate().round() as u64).min(rows - nulls);
    Ok(ColumnProfile {
        dataset: source.dataset().to_string(),
        column: column.to_string(),
        rows,
        nulls,
        distinct_estimate,
        min,
        max,
        top_k: top_values(sample, cfg.top_k),
        histogram: histogram(sample, cfg.histogram_buckets),
        profiled_at: cfg.profiled_at,
        method,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dialect: ProfileDialect,
        sampling: bool,
        values: Vec<Option<String>>,
    }

    impl ProfileSource for Fixture {
        fn dataset(&self) -> &str {
            "fixture"
        }
        fn dialect(&self) -> ProfileDialect {
            self.dialect
        }
        fn push_aggregate(&self, _statement: &str) -> Result<AggregateRow, String> {
            let non_null: Vec<&String> = self.values.iter().flatten().collect();
            Ok(AggregateRow {
                rows: self.values.len() as u64,
                non_null: non_null.len() as u64,
                min: non_null.iter().min().map(|s| s.to_string()),
                max: non_null.iter().max().map(|s| s.to_string()),
            })
        }
        fn supports_sampling(&self) -> bool {
            self.sampling
        }
        fn scan_column(
            &self,
            _column: &str,
            visit: &mut dyn FnMut(Option<&str>),
        ) -> Result<(), String> {
            for value in &self.values {
                visit(value.as_deref());
            }
            Ok(())
        }
    }

    fn values() -> Vec<Option<String>> {
        (0..5_000)
            .map(|i| (i % 7 != 0).then(|| (i % 300).to_string()))
            .collect()
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R761.2
    #[test]
    fn sample_profile_counts_are_consistent() {
        let source = Fixture {
            dialect: ProfileDialect::Other,
            sampling: true,
            values: values(),
        };
        let cfg = ProfileConfig {
            sample_rows: 1_000,
            ..ProfileConfig::default()
        };
        let profile = profile_column(&source, "c", &cfg).unwrap();
        assert!(profile.nulls <= profile.rows);
        assert_eq!(profile.rows, 5_000);
        assert_eq!(profile.method, ProfileMethod::Sample { size: 1_000 });
        assert!(profile.top_k.len() <= cfg.top_k);
        assert_eq!(profile.histogram.len(), cfg.histogram_buckets);
        profile.validate().unwrap();

        let pushed = Fixture {
            dialect: ProfileDialect::Postgres,
            ..source
        };
        let profile = profile_column(&pushed, "c", &cfg).unwrap();
        assert_eq!(profile.method, ProfileMethod::Pushdown);
        assert!(profile.nulls <= profile.rows);
        profile.validate().unwrap();

        let none = Fixture {
            dialect: ProfileDialect::Other,
            sampling: false,
            values: Vec::new(),
        };
        let refusal = profile_column(&none, "c", &cfg).unwrap_err();
        assert_eq!(refusal.code, PROFILE_UNSUPPORTED);
        assert!(refusal.to_string().starts_with("PROFILE_UNSUPPORTED: "));
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R761.2
    #[test]
    fn each_pushdown_dialect_quotes_its_identifiers() {
        assert!(ProfileDialect::Mysql.pushes_aggregates());
        assert!(ProfileDialect::Postgres.pushes_aggregates());
        assert!(!ProfileDialect::Other.pushes_aggregates());
        assert_eq!(
            aggregate_statement(ProfileDialect::Mysql, "orders", "a`b"),
            "SELECT COUNT(*), COUNT(`a``b`), MIN(`a``b`), MAX(`a``b`) FROM `orders`"
        );
        assert_eq!(
            aggregate_statement(ProfileDialect::Postgres, "orders", "a\"b"),
            "SELECT COUNT(*), COUNT(\"a\"\"b\"), MIN(\"a\"\"b\"), MAX(\"a\"\"b\") FROM \"orders\""
        );
    }
}
