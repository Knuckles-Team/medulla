//! The per-tenant usage ledger store (MEDULLA-DECISION-ENGINE-R801.2/.4/.5).
//!
//! Records, day-bucket increments, audit lines and price entries live in the
//! tenant-scoped Agent Library control owner beside the decision artifacts,
//! under four key families: `usage:<tenant>/<recorded_at_ms>/<seq>`,
//! `usage-bucket:<tenant>/<model_key>/<day>`,
//! `usage-audit:<tenant>/<appended_at_ms>/<seq>` and
//! `price:<tenant>/<model_key>/<valid_from_ms>`. An append writes the records,
//! the bucket increments and one audit line in a single write transaction;
//! every refusal returns before the transaction opens. Cost is never stored:
//! it is derived at read time from the price entries.

use std::collections::BTreeMap;

use medulla_decision_types::decision::record::outcome::RefusalCode;
use medulla_decision_types::decision::usage as codes;
use medulla_query_types::solve_scalar::digest_text;
use medulla_server_api::knob_config::knobs;
use medulla_storage::DECISION_ARTIFACTS;
use medulla_types::usage::{PriceCatalog, PriceEntry, UsageOutcome, UsageRecord};
use redb::ReadableTable;
use serde::{Deserialize, Serialize};

use super::agent_library::AgentLibraryStore;

/// Milliseconds in one day bucket.
pub const DAY_MS: u64 = 86_400_000;

/// Rows read per page when a family is scanned.
const SCAN_PAGE: usize = 5_000;

/// Domain of the batch digest.
const BATCH_DIGEST_DOMAIN: &str = "usage-batch";

/// Why the ledger refused a request. Every refusal precedes any write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageRefusal {
    /// A record `tenant` differs from the verified tenant.
    UnknownTenant,
    /// A wire count is negative.
    NegativeCount,
    /// `tokens_cached` exceeds `tokens_in`.
    CachedExceedsInput,
    /// The batch exceeds `usage.max_batch`.
    BatchTooLarge,
    /// A `content_digest` is not 64 lowercase hex characters.
    BadDigest,
    /// A new price starts at or before the latest entry of its key with other prices.
    PriceWindowOverlap,
    /// The store failed; nothing was written.
    Store(String),
}

impl UsageRefusal {
    /// The stable, registered refusal code.
    pub fn code(&self) -> RefusalCode {
        match self {
            Self::UnknownTenant => codes::USAGE_UNKNOWN_TENANT,
            Self::NegativeCount => codes::USAGE_NEGATIVE_COUNT,
            Self::CachedExceedsInput => codes::USAGE_CACHED_EXCEEDS_INPUT,
            Self::BatchTooLarge => codes::USAGE_BATCH_TOO_LARGE,
            Self::BadDigest => codes::USAGE_BAD_DIGEST,
            Self::PriceWindowOverlap => codes::PRICE_WINDOW_OVERLAP,
            Self::Store(_) => codes::USAGE_STORE_FAILED,
        }
    }
}

impl std::fmt::Display for UsageRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(detail) => write!(f, "{}: {detail}", self.code().as_str()),
            _ => f.write_str(self.code().as_str()),
        }
    }
}

impl From<String> for UsageRefusal {
    fn from(detail: String) -> Self {
        Self::Store(detail)
    }
}

/// Convert a signed wire count; a negative count is `USAGE_NEGATIVE_COUNT`.
pub fn require_non_negative(value: i64) -> Result<u64, UsageRefusal> {
    u64::try_from(value).map_err(|_| UsageRefusal::NegativeCount)
}

/// The evidence one append leaves: a digest of the canonical records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageAuditLine {
    /// The tenant the batch was appended under.
    pub tenant: String,
    /// The verified principal that appended it.
    pub principal: String,
    /// SHA-256 digest of the canonical records, in order. No content.
    pub batch_digest: String,
    /// How many records the batch held.
    pub record_count: u64,
    /// When the batch was appended.
    pub appended_at_ms: u64,
}

/// What an append returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendReceipt {
    /// Records written.
    pub appended: usize,
    /// The digest recorded in the audit line.
    pub batch_digest: String,
}

/// Sums of one `(model_key, day)` bucket, incremented by every append.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct DayBucket {
    calls: u64,
    tokens_in: u64,
    tokens_out: u64,
    tokens_cached: u64,
    errors: u64,
}

/// One day of one model in a rollup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// A rollup and how many ledger rows it read to produce it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rollup {
    /// Day buckets in `(model_key, day)` order.
    pub days: Vec<DayRollup>,
    /// Rows visited; buckets only, never records.
    pub rows_read: usize,
}

/// One run seen by the run-adjustment family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    /// The run.
    pub run_id: String,
    /// True when any record of the run has outcome `error`.
    pub errored: bool,
}

/// One run folded into the observation the run-adjustment family reads
/// (MEDULLA-DECISION-ENGINE-R158.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunWindow {
    /// The run.
    pub run_id: String,
    /// True when any record of the run has outcome `error`.
    pub errored: bool,
    /// When the run's newest record was recorded.
    pub newest_at_ms: u64,
    /// The summed latency of the run's calls, in milliseconds.
    pub latency_ms: u64,
    /// The run's cost in micro-units; `None` when a record has no price.
    pub cost_micro: Option<u64>,
}

fn record_prefix(tenant: &str) -> String {
    format!("usage:{tenant}/")
}

fn record_key(tenant: &str, recorded_at_ms: u64, seq: usize) -> String {
    format!("usage:{tenant}/{recorded_at_ms:020}/{seq:010}")
}

fn bucket_prefix(tenant: &str) -> String {
    format!("usage-bucket:{tenant}/")
}

fn bucket_key(tenant: &str, model_key: &str, day: u64) -> String {
    format!("usage-bucket:{tenant}/{model_key}/{day:010}")
}

fn audit_key(tenant: &str, appended_at_ms: u64, seq: usize) -> String {
    format!("usage-audit:{tenant}/{appended_at_ms:020}/{seq:010}")
}

fn price_prefix(tenant: &str, model_key: &str) -> String {
    format!("price:{tenant}/{model_key}/")
}

fn price_key(tenant: &str, model_key: &str, valid_from_ms: u64) -> String {
    format!("price:{tenant}/{model_key}/{valid_from_ms:020}")
}

/// Trim, ASCII-lowercase and map through `catalog.aliases`; an unmapped name
/// stays normalized.
pub fn normalize_model_key(catalog: &PriceCatalog, raw: &str) -> String {
    let normalized = raw.trim().to_ascii_lowercase();
    match catalog.aliases.get(&normalized) {
        Some(canonical) => canonical.clone(),
        None => normalized,
    }
}

/// The price in force for `model_key` at `at_ms`: the entry with the greatest
/// `valid_from_ms` at or below it.
fn price_at<'a>(catalog: &'a PriceCatalog, model_key: &str, at_ms: u64) -> Option<&'a PriceEntry> {
    catalog
        .entries
        .iter()
        .filter(|entry| entry.model_key == model_key && entry.valid_from_ms <= at_ms)
        .max_by_key(|entry| entry.valid_from_ms)
}

fn cost_of(price: &PriceEntry, tokens_in: u64, tokens_cached: u64, tokens_out: u64) -> Option<u64> {
    let billed_in = u128::from(tokens_in.saturating_sub(tokens_cached));
    let total = billed_in * u128::from(price.in_per_mtok_micro)
        + u128::from(tokens_cached) * u128::from(price.cached_per_mtok_micro)
        + u128::from(tokens_out) * u128::from(price.out_per_mtok_micro);
    u64::try_from(total / 1_000_000).ok()
}

/// Derived cost of one record in micro-units, or `None` when no price applies.
pub fn cost_micro(record: &UsageRecord, catalog: &PriceCatalog) -> Option<u64> {
    let key = normalize_model_key(catalog, &record.model_key);
    let price = price_at(catalog, &key, record.recorded_at_ms)?;
    cost_of(
        price,
        record.tokens_in,
        record.tokens_cached,
        record.tokens_out,
    )
}

fn validate_batch(tenant: &str, records: &[UsageRecord]) -> Result<(), UsageRefusal> {
    let max_batch = usize::try_from(knobs().usage_max_batch).unwrap_or(usize::MAX);
    if records.len() > max_batch {
        return Err(UsageRefusal::BatchTooLarge);
    }
    for record in records {
        if record.tenant != tenant {
            return Err(UsageRefusal::UnknownTenant);
        }
        if record.tokens_cached > record.tokens_in {
            return Err(UsageRefusal::CachedExceedsInput);
        }
        if record.validate().is_err() {
            return Err(UsageRefusal::BadDigest);
        }
    }
    Ok(())
}

fn store<E: ToString>(error: E) -> String {
    error.to_string()
}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, UsageRefusal> {
    serde_json::from_slice(bytes).map_err(|error| UsageRefusal::Store(error.to_string()))
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, UsageRefusal> {
    serde_json::to_vec(value).map_err(|error| UsageRefusal::Store(error.to_string()))
}

impl AgentLibraryStore {
    /// Append `records` for `tenant` in one write transaction: the records,
    /// their day-bucket increments and one audit line. Any refusal aborts the
    /// whole batch before a write opens.
    pub fn append_usage(
        &self,
        tenant: &str,
        principal: &str,
        catalog: &PriceCatalog,
        records: &[UsageRecord],
        appended_at_ms: u64,
    ) -> Result<AppendReceipt, UsageRefusal> {
        validate_batch(tenant, records)?;
        let normalized: Vec<UsageRecord> = records
            .iter()
            .map(|record| UsageRecord {
                model_key: normalize_model_key(catalog, &record.model_key),
                ..record.clone()
            })
            .collect();
        let batch_digest = digest_text(BATCH_DIGEST_DOMAIN, &normalized);
        let line = UsageAuditLine {
            tenant: tenant.to_string(),
            principal: principal.to_string(),
            batch_digest: batch_digest.clone(),
            record_count: normalized.len() as u64,
            appended_at_ms,
        };
        // Serialize before the transaction so an encoding failure writes nothing.
        let mut rows: Vec<(u64, String, u64, Vec<u8>)> = Vec::with_capacity(normalized.len());
        for record in &normalized {
            rows.push((
                record.recorded_at_ms,
                record.model_key.clone(),
                record.recorded_at_ms / DAY_MS,
                encode(record)?,
            ));
        }
        let line_bytes = encode(&line)?;
        let mut failure: Option<UsageRefusal> = None;
        let result = self.maintain_control_rows(tenant, "usage_ledger_append", |owner| {
            let mut table = owner.open_table(DECISION_ARTIFACTS)?;
            let mut seq_in_batch: BTreeMap<u64, usize> = BTreeMap::new();
            for (index, (recorded_at_ms, model_key, day, bytes)) in rows.iter().enumerate() {
                let prefix = format!("usage:{tenant}/{recorded_at_ms:020}/");
                let mut existing = 0usize;
                for row in table.range((tenant, prefix.as_str())..).map_err(store)? {
                    let (key, _) = row.map_err(store)?;
                    if key.value().0 != tenant || !key.value().1.starts_with(&prefix) {
                        break;
                    }
                    existing += 1;
                }
                let seq = seq_in_batch.entry(*recorded_at_ms).or_insert(existing);
                let key = record_key(tenant, *recorded_at_ms, *seq);
                *seq += 1;
                table
                    .insert((tenant, key.as_str()), bytes.as_slice())
                    .map_err(store)?;

                let bucket = bucket_key(tenant, model_key, *day);
                let mut sums = match table.get((tenant, bucket.as_str())).map_err(store)? {
                    Some(row) => match decode::<DayBucket>(row.value()) {
                        Ok(sums) => sums,
                        Err(refusal) => {
                            failure = Some(refusal);
                            return Err("usage bucket row is unreadable".to_string());
                        }
                    },
                    None => DayBucket::default(),
                };
                let record = &normalized[index];
                sums.calls += 1;
                sums.tokens_in += record.tokens_in;
                sums.tokens_out += record.tokens_out;
                sums.tokens_cached += record.tokens_cached;
                if record.outcome == UsageOutcome::Error {
                    sums.errors += 1;
                }
                let encoded = match encode(&sums) {
                    Ok(bytes) => bytes,
                    Err(refusal) => {
                        failure = Some(refusal);
                        return Err("usage bucket row cannot be encoded".to_string());
                    }
                };
                table
                    .insert((tenant, bucket.as_str()), encoded.as_slice())
                    .map_err(store)?;
            }
            let prefix = format!("usage-audit:{tenant}/{appended_at_ms:020}/");
            let mut existing = 0usize;
            for row in table.range((tenant, prefix.as_str())..).map_err(store)? {
                let (key, _) = row.map_err(store)?;
                if key.value().0 != tenant || !key.value().1.starts_with(&prefix) {
                    break;
                }
                existing += 1;
            }
            let key = audit_key(tenant, appended_at_ms, existing);
            table
                .insert((tenant, key.as_str()), line_bytes.as_slice())
                .map_err(store)?;
            Ok(())
        });
        match result {
            Ok(()) => Ok(AppendReceipt {
                appended: normalized.len(),
                batch_digest,
            }),
            Err(detail) => Err(failure.unwrap_or(UsageRefusal::Store(detail))),
        }
    }

    /// Write one price entry. An identical entry is an idempotent no-op; a
    /// different entry at or below the latest `valid_from_ms` of its key is
    /// `PRICE_WINDOW_OVERLAP`.
    pub fn put_price(&self, tenant: &str, entry: &PriceEntry) -> Result<(), UsageRefusal> {
        let bytes = encode(entry)?;
        let prefix = price_prefix(tenant, &entry.model_key);
        let key = price_key(tenant, &entry.model_key, entry.valid_from_ms);
        let mut failure: Option<UsageRefusal> = None;
        let result = self.maintain_control_rows(tenant, "usage_ledger_price", |owner| {
            let mut table = owner.open_table(DECISION_ARTIFACTS)?;
            if let Some(row) = table.get((tenant, key.as_str())).map_err(store)? {
                if row.value() == bytes.as_slice() {
                    return Ok(());
                }
            }
            let mut latest: Option<u64> = None;
            for row in table.range((tenant, prefix.as_str())..).map_err(store)? {
                let (row_key, _) = row.map_err(store)?;
                if row_key.value().0 != tenant || !row_key.value().1.starts_with(&prefix) {
                    break;
                }
                latest = row_key.value().1[prefix.len()..].parse::<u64>().ok();
            }
            if latest.is_some_and(|latest| entry.valid_from_ms <= latest) {
                failure = Some(UsageRefusal::PriceWindowOverlap);
                return Err("price window overlaps the latest entry".to_string());
            }
            table
                .insert((tenant, key.as_str()), bytes.as_slice())
                .map_err(store)?;
            Ok(())
        });
        match result {
            Ok(()) => Ok(()),
            Err(detail) => Err(failure.unwrap_or(UsageRefusal::Store(detail))),
        }
    }

    /// Every price entry of `tenant` for every model, in key order.
    pub fn price_entries(&self, tenant: &str) -> Result<Vec<PriceEntry>, UsageRefusal> {
        let mut out = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let page = self.decision_artifacts_page(
                tenant,
                &format!("price:{tenant}/"),
                after.as_deref(),
                SCAN_PAGE,
            )?;
            let Some((last, _)) = page.last() else { break };
            after = Some(last.clone());
            let full = page.len() == SCAN_PAGE;
            for (_, bytes) in &page {
                out.push(decode::<PriceEntry>(bytes)?);
            }
            if !full {
                break;
            }
        }
        Ok(out)
    }

    /// Day buckets of `tenant` between `from_day` and `to_day` inclusive,
    /// priced against `catalog`. Reads buckets only, never records. A tenant
    /// with no rows gets an empty result.
    pub fn usage_rollup(
        &self,
        tenant: &str,
        catalog: &PriceCatalog,
        from_day: u64,
        to_day: u64,
    ) -> Result<Rollup, UsageRefusal> {
        let mut days = Vec::new();
        let mut rows_read = 0usize;
        let prefix = bucket_prefix(tenant);
        let mut after: Option<String> = None;
        loop {
            let page =
                self.decision_artifacts_page(tenant, &prefix, after.as_deref(), SCAN_PAGE)?;
            let Some((last, _)) = page.last() else { break };
            after = Some(last.clone());
            let full = page.len() == SCAN_PAGE;
            for (key, bytes) in &page {
                rows_read += 1;
                let Some((model_key, day)) = key[prefix.len()..].rsplit_once('/') else {
                    continue;
                };
                let Ok(day) = day.parse::<u64>() else {
                    continue;
                };
                if day < from_day || day > to_day {
                    continue;
                }
                let sums = decode::<DayBucket>(bytes)?;
                let cost_micro =
                    price_at(catalog, model_key, day * DAY_MS + DAY_MS - 1).and_then(|price| {
                        cost_of(price, sums.tokens_in, sums.tokens_cached, sums.tokens_out)
                    });
                days.push(DayRollup {
                    model_key: model_key.to_string(),
                    day,
                    calls: sums.calls,
                    tokens_in: sums.tokens_in,
                    tokens_out: sums.tokens_out,
                    tokens_cached: sums.tokens_cached,
                    errors: sums.errors,
                    cost_micro,
                });
            }
            if !full {
                break;
            }
        }
        Ok(Rollup { days, rows_read })
    }

    /// The newest `n` distinct runs of `(child_id, operation)`, newest first.
    /// A run is `errored` when any of its records has outcome `error`.
    pub fn recent_runs(
        &self,
        tenant: &str,
        child_id: &str,
        operation: &str,
        n: usize,
    ) -> Result<Vec<RunOutcome>, UsageRefusal> {
        let catalog = PriceCatalog {
            aliases: BTreeMap::new(),
            entries: Vec::new(),
        };
        Ok(self
            .recent_run_windows(tenant, child_id, operation, n, 0, &catalog)?
            .into_iter()
            .map(|run| RunOutcome {
                run_id: run.run_id,
                errored: run.errored,
            })
            .collect())
    }

    /// The newest `n` distinct runs of `(child_id, operation)` whose newest
    /// record is at or after `since_ms`, newest first, each folded into one
    /// observation (MEDULLA-DECISION-ENGINE-R158.7). `cost_micro` is priced
    /// against `catalog` and is `None` when any record of the run has no price.
    pub fn recent_run_windows(
        &self,
        tenant: &str,
        child_id: &str,
        operation: &str,
        n: usize,
        since_ms: u64,
        catalog: &PriceCatalog,
    ) -> Result<Vec<RunWindow>, UsageRefusal> {
        let mut runs: BTreeMap<String, RunWindow> = BTreeMap::new();
        let prefix = record_prefix(tenant);
        let mut after: Option<String> = None;
        loop {
            let page =
                self.decision_artifacts_page(tenant, &prefix, after.as_deref(), SCAN_PAGE)?;
            let Some((last, _)) = page.last() else { break };
            after = Some(last.clone());
            let full = page.len() == SCAN_PAGE;
            for (_, bytes) in &page {
                let record = decode::<UsageRecord>(bytes)?;
                if record.child_id != child_id || record.operation != operation {
                    continue;
                }
                let cost = cost_micro(&record, catalog);
                let entry = runs.entry(record.run_id.clone()).or_insert(RunWindow {
                    run_id: record.run_id.clone(),
                    errored: false,
                    newest_at_ms: 0,
                    latency_ms: 0,
                    cost_micro: Some(0),
                });
                entry.newest_at_ms = entry.newest_at_ms.max(record.recorded_at_ms);
                entry.errored |= record.outcome == UsageOutcome::Error;
                entry.latency_ms = entry.latency_ms.saturating_add(record.latency_ms);
                entry.cost_micro = match (entry.cost_micro, cost) {
                    (Some(sum), Some(add)) => Some(sum.saturating_add(add)),
                    _ => None,
                };
            }
            if !full {
                break;
            }
        }
        let mut ordered: Vec<RunWindow> = runs
            .into_values()
            .filter(|run| run.newest_at_ms >= since_ms)
            .collect();
        ordered.sort_by(|a, b| {
            b.newest_at_ms
                .cmp(&a.newest_at_ms)
                .then_with(|| a.run_id.cmp(&b.run_id))
        });
        ordered.truncate(n);
        Ok(ordered)
    }
}

#[cfg(test)]
mod tests;
