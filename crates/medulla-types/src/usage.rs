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
pub enum UsageOutcome {
    /// The call completed and produced a completion.
    Ok,
    /// The call failed before producing a usable completion.
    Error,
}

/// One observed model call, owned by the tenant of the verified caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
}

/// The price entries and the model-name aliases a ledger prices against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceCatalog {
    /// Normalized raw name to canonical model key.
    pub aliases: BTreeMap<String, String>,
    /// Every price entry.
    pub entries: Vec<PriceEntry>,
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
