//! Asset usage and catalog search hits (MEDULLA-UNIFIED-DATA-PLANE-R764). Usage
//! counts come from the query audit. Search fuses text and vector scores
//! with a usage prior that never exceeds `USAGE_PRIOR_CAP` of the score.

use serde::{Deserialize, Serialize};

/// Reads and queries of one asset by one principal in one period.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetUsage {
    pub asset: String,
    pub principal: String,
    pub period: String,
    pub reads: u64,
    pub queries: u64,
}

/// `knob catalog_usage.period`: the usage bucket width in seconds.
pub const CATALOG_USAGE_PERIOD_KNOB: &str = "catalog_usage.period";
/// Default of `knob catalog_usage.period` (seconds).
pub const CATALOG_USAGE_PERIOD_DEFAULT_S: u64 = 86_400;
/// Inclusive bounds of `knob catalog_usage.period` (seconds).
pub const CATALOG_USAGE_PERIOD_RANGE_S: (u64, u64) = (3_600, 2_592_000);
/// Environment override of `knob catalog_usage.period`.
pub const ENV_CATALOG_USAGE_PERIOD_S: &str = "MEDULLA_CATALOG_USAGE_PERIOD_S";
/// The `medulla-tsdb` series counting reads of one asset by one principal.
pub const ASSET_USAGE_READS_SERIES: &str = "asset_usage.reads";
/// The `medulla-tsdb` series counting queries that touched one asset by one principal.
pub const ASSET_USAGE_QUERIES_SERIES: &str = "asset_usage.queries";

/// The effective `knob catalog_usage.period` in seconds: the environment value when it lies
/// inside the knob's bounds, the default otherwise.
pub fn catalog_usage_period_s() -> u64 {
    std::env::var(ENV_CATALOG_USAGE_PERIOD_S)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|n| (CATALOG_USAGE_PERIOD_RANGE_S.0..=CATALOG_USAGE_PERIOD_RANGE_S.1).contains(n))
        .unwrap_or(CATALOG_USAGE_PERIOD_DEFAULT_S)
}

/// The tsdb series name for one `(base series, principal, asset)`: the tags of an asset usage
/// point are tenant (the series key), principal and asset.
pub fn usage_series_name(base: &str, principal: &str, asset: &str) -> String {
    format!("{base}|principal={principal}|asset={asset}")
}

/// The `medulla-tsdb` series counting joins of one asset with another by one principal.
pub const ASSET_USAGE_JOIN_SERIES: &str = "asset_usage.join";
/// The `medulla-tsdb` series counting filters on one column of an asset by one principal.
pub const ASSET_USAGE_FILTER_SERIES: &str = "asset_usage.filter";

/// An integer knob of the catalog usage family: name, default, inclusive bounds, environment
/// override. `value` reads the environment and falls back to the default outside the bounds.
#[derive(Clone, Copy, Debug)]
pub struct UsageKnob {
    pub name: &'static str,
    pub default: u64,
    pub range: (u64, u64),
    pub env: &'static str,
}

impl UsageKnob {
    /// The effective value of the knob.
    pub fn value(&self) -> u64 {
        std::env::var(self.env)
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|n| (self.range.0..=self.range.1).contains(n))
            .unwrap_or(self.default)
    }
}

/// `knob catalog_usage.log_batch`: statements read from a source query log per run.
pub const CATALOG_USAGE_LOG_BATCH: UsageKnob = UsageKnob {
    name: "catalog_usage.log_batch",
    default: 1000,
    range: (10, 100_000),
    env: "MEDULLA_CATALOG_USAGE_LOG_BATCH",
};
/// `knob catalog_usage.log_interval`: seconds between source query log runs.
pub const CATALOG_USAGE_LOG_INTERVAL: UsageKnob = UsageKnob {
    name: "catalog_usage.log_interval",
    default: 3600,
    range: (60, 86_400),
    env: "MEDULLA_CATALOG_USAGE_LOG_INTERVAL_S",
};
/// `knob catalog_usage.top_n`: entries in each `CatalogUsage` answer when the request names none.
pub const CATALOG_USAGE_TOP_N: UsageKnob = UsageKnob {
    name: "catalog_usage.top_n",
    default: 5,
    range: (1, 50),
    env: "MEDULLA_CATALOG_USAGE_TOP_N",
};
/// `knob catalog_usage.suggestion_min_count`: uses a join or filter needs to be suggested.
pub const CATALOG_USAGE_SUGGESTION_MIN_COUNT: UsageKnob = UsageKnob {
    name: "catalog_usage.suggestion_min_count",
    default: 3,
    range: (1, 1000),
    env: "MEDULLA_CATALOG_USAGE_SUGGESTION_MIN_COUNT",
};

/// Scope that lets a caller see which principals use an asset.
pub const CATALOG_USAGE_PRINCIPALS_SCOPE: &str = "catalog:usage:principals";

/// The tsdb series name of one join: the tags are principal, asset and the joined asset.
pub fn usage_join_series_name(principal: &str, asset: &str, with: &str) -> String {
    format!("{ASSET_USAGE_JOIN_SERIES}|principal={principal}|asset={asset}|with={with}")
}

/// The tsdb series name of one filter: the tags are principal, asset and the filtered column.
pub fn usage_filter_series_name(principal: &str, asset: &str, column: &str) -> String {
    format!("{ASSET_USAGE_FILTER_SERIES}|principal={principal}|asset={asset}|column={column}")
}

/// One principal that reads an asset, most reads first. `principal` is null for a caller
/// without scope `catalog:usage:principals`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrequentUser {
    pub principal: Option<String>,
    pub reads: u64,
}

/// One suggested join or filter of an asset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSuggestion {
    /// `join` or `filter`.
    pub kind: String,
    /// The joined asset for a join, the column for a filter.
    pub target: String,
    pub count: u64,
}

/// The answer of `Method::CatalogUsage`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogUsageAnswer {
    pub asset: String,
    pub frequent_users: Vec<FrequentUser>,
    pub suggestions: Vec<UsageSuggestion>,
}

/// The score components of one hit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HitComponents {
    pub text: f64,
    pub vector: f64,
    pub usage: f64,
}

/// One catalog search hit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSearchHit {
    pub asset: String,
    pub kind: String,
    pub score: f64,
    pub usage_rank: u32,
    pub components: HitComponents,
}

/// The usage prior's maximum share of a fused score.
pub const USAGE_PRIOR_CAP: f64 = 0.20;

/// Fuse a relevance score (text plus vector, each in `[0, 1]`, averaged)
/// with a usage prior. `usage_norm` is the asset's usage normalized to
/// `[0, 1]` over the candidate set. The usage component is at most
/// `USAGE_PRIOR_CAP` of the fused score.
pub fn fuse_with_usage_prior(text: f64, vector: f64, usage_norm: f64) -> HitComponents {
    let clamp = |value: f64| {
        if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    let relevance = (clamp(text) + clamp(vector)) / 2.0;
    // usage <= cap * (relevance + usage)  <=>  usage <= cap / (1 - cap) * relevance
    let ceiling = USAGE_PRIOR_CAP / (1.0 - USAGE_PRIOR_CAP) * relevance;
    let usage = (clamp(usage_norm) * USAGE_PRIOR_CAP).min(ceiling);
    HitComponents {
        text: clamp(text) / 2.0,
        vector: clamp(vector) / 2.0,
        usage,
    }
}

impl HitComponents {
    pub fn total(&self) -> f64 {
        self.text + self.vector + self.usage
    }
}
