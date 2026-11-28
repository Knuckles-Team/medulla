//! Per-tenant usage ledger model (MEDULLA-DECISION-ENGINE-R801).
//!
//! A [`UsageRecord`] is one observed model call, recorded under the tenant of
//! the verified caller. Prompt and completion content never cross the wire or
//! reach the store: only their [`UsageRecord::content_digest`] does.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Format identity of the usage and price records.
pub const USAGE_SCHEMA_VERSION: u16 = 1;

/// Refusal code of a record whose `content_digest` is not 64 lowercase hex characters.
pub const USAGE_BAD_DIGEST: &str = "USAGE_BAD_DIGEST";

/// The closed outcome of one model call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum UsageOutcome {
    /// The call completed and produced a completion.
    Ok,
    /// The call failed before producing a usable completion.
    Error,
}

/// One observed model call, owned by the tenant of the verified caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct UsageRecord {
    /// Format identity of this record.
    pub schema_version: u16,
    /// When the call was recorded, in milliseconds since the epoch.
    pub recorded_at_ms: u64,
    /// The child run the call belongs to; with `operation` it partitions a run
    /// for the error rate of the run-adjustment family.
    pub child_id: String,
    /// The operation the call served.
    pub operation: String,
    /// The tenant that owns the record (the verified caller's tenant).
    pub tenant: String,
    /// The principal that made the call.
    pub principal: String,
    /// The run the call belongs to.
    pub run_id: String,
    /// The model key as reported by the caller, unnormalized.
    pub model_key: String,
    /// Input tokens billed on the call.
    pub tokens_in: u64,
    /// Output tokens produced by the call.
    pub tokens_out: u64,
    /// Cached input tokens reused from the prefix cache.
    pub tokens_cached: u64,
    /// Wall-clock latency of the call, in milliseconds.
    pub latency_ms: u64,
    /// Whether the call succeeded.
    pub outcome: UsageOutcome,
    /// Digest of the prompt and completion content; the content itself is
    /// never stored.
    pub content_digest: String,
    /// The project the call was made for; empty when the caller named none
    /// (MEDULLA-DECISION-ENGINE-R801.9).
    #[serde(default)]
    pub project_id: String,
    /// The agent that made the call; empty when the caller named none
    /// (MEDULLA-DECISION-ENGINE-R801.9).
    #[serde(default)]
    pub agent_id: String,
}

impl UsageRecord {
    /// `Ok` when the record may be stored; otherwise the stable refusal code.
    pub fn validate(&self) -> Result<(), &'static str> {
        if is_sha256_hex(&self.content_digest) {
            Ok(())
        } else {
            Err(USAGE_BAD_DIGEST)
        }
    }
}

/// True when `text` is 64 lowercase hexadecimal characters (a SHA-256 digest).
pub fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// One model price, valid from `valid_from_ms` until the next entry of its key.
///
/// Prices are whole micro-units of the catalog currency for one million
/// tokens, so no float appears.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PriceEntry {
    /// Format identity of this entry.
    pub schema_version: u16,
    /// The normalized model key the price applies to.
    pub model_key: String,
    /// First millisecond the price applies to.
    pub valid_from_ms: u64,
    /// Micro-units per million input tokens.
    pub in_per_mtok_micro: u64,
    /// Micro-units per million output tokens.
    pub out_per_mtok_micro: u64,
    /// Micro-units per million cached input tokens.
    pub cached_per_mtok_micro: u64,
    /// ISO 4217 code of the currency this price is recorded in (three uppercase
    /// letters); empty when the catalog named none. A cost is reported in the
    /// currency of the price that produced it and is never converted
    /// (MEDULLA-DECISION-ENGINE-R801.9).
    #[serde(default)]
    pub currency: String,
}

/// The price entries and the model-name aliases a ledger prices against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PriceCatalog {
    /// Normalized raw name to canonical model key.
    pub aliases: BTreeMap<String, String>,
    /// Every price entry.
    pub entries: Vec<PriceEntry>,
}

/// One served usage-ledger operation (`Method::Usage`,
/// MEDULLA-DECISION-ENGINE-R801.8, R801.7). `Append` and `PutPrice` write and
/// need the service-only scope `usage:write`; `Read` needs `usage:read`. The
/// tenant is always the verified caller's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum UsageOp {
    /// Append one batch of observed model calls in one ledger transaction.
    Append {
        /// The records, each owned by the verified tenant.
        records: Vec<UsageRecord>,
    },
    /// Validate a price catalog and store its entries.
    PutPrice {
        /// The catalog to store.
        catalog: PriceCatalog,
    },
    /// Read the verified tenant's records or daily rollup.
    Read {
        /// First millisecond of the window (inclusive).
        from_ms: u64,
        /// Last millisecond of the window (inclusive).
        to_ms: u64,
        /// Keep only this model (normalized against the tenant's catalog).
        #[serde(default)]
        model_key: Option<String>,
        /// Which view to return.
        view: UsageReadView,
        /// Also fold the window into one row per model, project or agent
        /// (`Rollup` view only; MEDULLA-DECISION-ENGINE-R801.9).
        #[serde(default)]
        group_by: Option<UsageGroupBy>,
        /// Opaque cursor of the previous records page.
        #[serde(default)]
        cursor: Option<String>,
    },
}

impl UsageOp {
    /// True for the ops that write the ledger.
    pub fn is_mutation(&self) -> bool {
        !matches!(self, Self::Read { .. })
    }

    /// The scope the op needs.
    pub fn required_scope(&self) -> &'static str {
        if self.is_mutation() {
            "usage:write"
        } else {
            "usage:read"
        }
    }
}

/// The closed view a served usage read returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum UsageReadView {
    /// The individual records, paged by cursor.
    Records,
    /// The per-model day buckets.
    Rollup,
}

/// The closed dimension a grouped rollup folds the window by
/// (MEDULLA-DECISION-ENGINE-R801.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum UsageGroupBy {
    /// One row per normalized model key.
    Model,
    /// One row per `project_id`; the empty id groups the unassigned calls.
    Project,
    /// One row per `agent_id`; the empty id groups the unassigned calls.
    Agent,
}

/// One group of a grouped rollup (MEDULLA-DECISION-ENGINE-R801.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct UsageGroupRollup {
    /// The model key, project id or agent id the group is keyed by.
    pub key: String,
    /// Calls in the group.
    pub calls: u64,
    /// Input tokens.
    pub tokens_in: u64,
    /// Output tokens.
    pub tokens_out: u64,
    /// Cached input tokens.
    pub tokens_cached: u64,
    /// Calls that errored.
    pub errors: u64,
    /// Summed derived cost; `None` when any call has no price or the prices
    /// are in different currencies.
    pub cost_micro: Option<u64>,
    /// ISO 4217 currency of `cost_micro`, as recorded on the prices; `None`
    /// when the cost is `None` or the catalog named no currency.
    pub currency: Option<String>,
    /// Nearest-rank 95th percentile of the calls' latency, in milliseconds.
    pub latency_p95_ms: u64,
}

/// One day of one model in a rollup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DayRollup {
    /// The normalized model key.
    pub model_key: String,
    /// Days since the epoch (`recorded_at_ms / 86_400_000`).
    pub day: u64,
    /// Calls in the bucket.
    pub calls: u64,
    /// Input tokens.
    pub tokens_in: u64,
    /// Output tokens.
    pub tokens_out: u64,
    /// Cached input tokens.
    pub tokens_cached: u64,
    /// Calls that errored.
    pub errors: u64,
    /// Derived cost, priced at the last millisecond of the day; `None` when no
    /// price applies.
    pub cost_micro: Option<u64>,
    /// ISO 4217 currency of `cost_micro`, as recorded on the price used;
    /// `None` when there is no cost or the price named no currency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

/// What a served `Usage` read returns (MEDULLA-DECISION-ENGINE-R801.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "view", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum UsageReadResult {
    /// One page of records.
    Records {
        /// The records of the page, in key order.
        records: Vec<UsageRecord>,
        /// Present only when a full page was returned.
        next_cursor: Option<String>,
    },
    /// The day buckets.
    Rollup {
        /// Buckets in `(model_key, day)` order.
        days: Vec<DayRollup>,
        /// The window folded by the requested `group_by`, in key order;
        /// empty when none was requested.
        #[serde(default)]
        groups: Vec<UsageGroupRollup>,
        /// The newest `recorded_at_ms` the tenant's ledger holds (the last
        /// ingested event time), independent of the window; `None` when the
        /// ledger is empty.
        #[serde(default)]
        index_watermark_ms: Option<u64>,
    },
}

/// What a served `Usage` append returns (MEDULLA-DECISION-ENGINE-R801.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct UsageAppended {
    /// Records written.
    pub appended: u64,
    /// The digest of the canonical records, as recorded in the audit line.
    pub batch_digest: String,
}

/// What a served `Usage` price put returns (MEDULLA-DECISION-ENGINE-R801.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct UsagePriceStored {
    /// Price entries in the stored catalog (identical replays included).
    pub entries: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> UsageRecord {
        UsageRecord {
            schema_version: USAGE_SCHEMA_VERSION,
            recorded_at_ms: 1_700_000_000_000,
            child_id: "child-1".into(),
            operation: "decide".into(),
            tenant: "t1".into(),
            principal: "p1".into(),
            run_id: "run-1".into(),
            model_key: "m".into(),
            tokens_in: 10,
            tokens_out: 5,
            tokens_cached: 2,
            latency_ms: 40,
            outcome: UsageOutcome::Ok,
            content_digest: "a".repeat(64),
            project_id: "p-1".into(),
            agent_id: "a-1".into(),
        }
    }

    fn entry() -> PriceEntry {
        PriceEntry {
            schema_version: USAGE_SCHEMA_VERSION,
            model_key: "m".into(),
            valid_from_ms: 7,
            in_per_mtok_micro: 3_000_000,
            out_per_mtok_micro: 15_000_000,
            cached_per_mtok_micro: 300_000,
            currency: "USD".into(),
        }
    }

    #[test]
    fn record_round_trips_digest_only() {
        // spec: MEDULLA-DECISION-ENGINE-R801.1
        let rec = record();
        let json = serde_json::to_string(&rec).unwrap();
        assert_eq!(serde_json::from_str::<UsageRecord>(&json).unwrap(), rec);
        assert!(!json.contains("prompt") && !json.contains("completion"));
        let price = entry();
        let json = serde_json::to_string(&price).unwrap();
        assert_eq!(serde_json::from_str::<PriceEntry>(&json).unwrap(), price);
        let catalog = PriceCatalog {
            aliases: BTreeMap::from([("gpt".to_string(), "m".to_string())]),
            entries: vec![price],
        };
        let json = serde_json::to_string(&catalog).unwrap();
        assert_eq!(
            serde_json::from_str::<PriceCatalog>(&json).unwrap(),
            catalog
        );

        let mut value = serde_json::to_value(&rec).unwrap();
        value["prompt"] = "secret".into();
        assert!(serde_json::from_value::<UsageRecord>(value).is_err());
        let mut value = serde_json::to_value(entry()).unwrap();
        value["extra"] = 1.into();
        assert!(serde_json::from_value::<PriceEntry>(value).is_err());

        assert_eq!(rec.validate(), Ok(()));
        let mut short = record();
        short.content_digest = "a".repeat(40);
        assert_eq!(short.validate(), Err(USAGE_BAD_DIGEST));
        let mut upper = record();
        upper.content_digest = "A".repeat(64);
        assert_eq!(upper.validate(), Err(USAGE_BAD_DIGEST));
    }
}
