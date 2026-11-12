//! Asset usage from the query log of an attached source (MEDULLA-UNIFIED-DATA-PLANE-R764.7).
//!
//! One run reads the statements a source logged since its watermark, counts reads per table
//! and the joins and filters in common use with the shared parser, and appends them as
//! `asset_usage.reads`, `asset_usage.join` and `asset_usage.filter` points under the principal
//! `source:<source>:<source_user>`. The per-source cursor (watermark and the running totals of a
//! cumulative log) is one JSON record under the durable state directory, written after the
//! points, so a replay of the same log state adds nothing.

use std::sync::Arc;

use medulla_server_api::state::ServerState;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Whose source to read and where its usage lands. All four values come from the verified
/// submit, never from the client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryLogRun {
    /// The verified owner scope that registered the source.
    pub owner_scope: String,
    /// The verified tenant scope of the series key.
    pub tenant: String,
    /// The graph the usage points land in.
    pub graph: String,
    /// The registered source name.
    pub source: String,
}

/// What one run counted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryLogSummary {
    /// Statements newly counted (growth since the last run).
    pub statements: u64,
    /// Distinct `(user, table)` read series written.
    pub reads: u64,
    /// Distinct join series written.
    pub joins: u64,
    /// Distinct filter series written.
    pub filters: u64,
    /// Statements that did not parse.
    pub unparsed: u64,
    /// The watermark after the run.
    pub watermark_ms: u64,
}

/// The per-source cursor.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryLogCursor {
    /// The newest statement time already counted.
    pub watermark_ms: u64,
    /// Running totals already counted, by cumulative key.
    pub seen: std::collections::BTreeMap<String, u64>,
}

impl QueryLogCursor {
    /// The new times to count for a cumulative or event entry, advancing the cursor.
    pub fn advance(&mut self, key: Option<&str>, calls: u64, executed_at_ms: u64) -> u64 {
        self.watermark_ms = self.watermark_ms.max(executed_at_ms);
        match key {
            Some(key) => {
                let before = self.seen.insert(key.to_string(), calls).unwrap_or(0);
                calls.saturating_sub(before)
            }
            None => calls,
        }
    }
}

/// Run one ingest of `run`'s source query log. Refusals: `QUERY_LOG_UNSUPPORTED` (the source
/// keeps no readable log) and `QUERY_LOG_UNAVAILABLE` (the log, the time-series store or the
/// state directory cannot be reached).
#[cfg(all(feature = "federation-sql", feature = "tsdb"))]
pub async fn run_query_log_ingest(
    state: &Arc<RwLock<ServerState>>,
    run: &QueryLogRun,
) -> Result<QueryLogSummary, String> {
    use crate::api::query_txn::ForeignStateExt;
    use medulla_federation::source_query_log::{dialect_for_dsn, query_log_for_dsn, tally_entries};
    use medulla_tsdb::point::Point;
    use medulla_tsdb::store::SeriesKey;
    use medulla_types::asset_usage::{
        catalog_usage_period_s, usage_filter_series_name, usage_join_series_name,
        usage_series_name, ASSET_USAGE_READS_SERIES, CATALOG_USAGE_LOG_BATCH,
    };

    let (catalog, store, persist) = {
        let guard = state.read().await;
        (
            guard.foreign_sources(),
            guard.tsdb_store.clone(),
            guard.persist_dir.clone(),
        )
    };
    let dsn = catalog
        .sql_source_dsn(&run.owner_scope, &run.source)
        .ok_or_else(|| {
            "QUERY_LOG_UNSUPPORTED: the source is not a registered SQL source".to_string()
        })?;
    let log = query_log_for_dsn(&dsn);
    let persist = persist.ok_or_else(|| {
        "QUERY_LOG_UNAVAILABLE: no durable state directory holds the watermark".to_string()
    })?;
    let store = store.ok_or_else(|| "QUERY_LOG_UNAVAILABLE: no time-series store".to_string())?;
    let path = cursor_path(&persist, run);
    let mut cursor = load_cursor(&path)?;
    let batch = CATALOG_USAGE_LOG_BATCH.value() as usize;
    let entries = log
        .read_since(cursor.watermark_ms, batch)
        .await
        .map_err(|refused| format!("{}: the source query log cannot be read", refused.code()))?;

    let mut summary = QueryLogSummary::default();
    let mut counted = Vec::with_capacity(entries.len());
    for entry in &entries {
        let times = cursor.advance(
            entry.cumulative_key.as_deref(),
            entry.calls,
            entry.executed_at_ms,
        );
        summary.statements += times;
        counted.push((entry, times));
    }
    let tally = tally_entries(counted, dialect_for_dsn(&dsn));
    summary.unparsed = tally.unparsed;
    summary.watermark_ms = cursor.watermark_ms;
    if tally.unparsed > 0 {
        medulla_kernel::metrics::process().query_log_unparsed(tally.unparsed);
    }

    let bucket_ns = catalog_usage_period_s().saturating_mul(1_000_000_000);
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0);
    let fields = vec!["count".to_string()];
    let principal = |user: &str| format!("source:{}:{user}", run.source);
    let write = |series: String, count: u64| -> Result<(), String> {
        let key = SeriesKey::new(run.tenant.as_str(), run.graph.as_str(), series);
        store
            .append_scoped(
                &key,
                1,
                bucket_ns,
                &fields,
                &[Point::single(now_ns, count as f64)],
            )
            .map_err(|_| "QUERY_LOG_UNAVAILABLE: usage points could not be stored".to_string())
    };
    for ((user, table), count) in &tally.reads {
        write(
            usage_series_name(ASSET_USAGE_READS_SERIES, &principal(user), table),
            *count,
        )?;
    }
    for ((user, table, other), count) in &tally.joins {
        write(
            usage_join_series_name(&principal(user), table, other),
            *count,
        )?;
    }
    for ((user, table, column), count) in &tally.filters {
        write(
            usage_filter_series_name(&principal(user), table, column),
            *count,
        )?;
    }
    summary.reads = tally.reads.len() as u64;
    summary.joins = tally.joins.len() as u64;
    summary.filters = tally.filters.len() as u64;
    save_cursor(&path, &cursor)?;
    Ok(summary)
}

/// Without the SQL federation or time-series build nothing can be read or stored.
#[cfg(not(all(feature = "federation-sql", feature = "tsdb")))]
pub async fn run_query_log_ingest(
    _state: &Arc<RwLock<ServerState>>,
    _run: &QueryLogRun,
) -> Result<QueryLogSummary, String> {
    Err("QUERY_LOG_UNAVAILABLE: this build has no source query log reader".to_string())
}

/// Where the cursor of `run` lives under the durable state directory.
pub fn cursor_path(persist_dir: &str, run: &QueryLogRun) -> std::path::PathBuf {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(
        format!(
            "{}\0{}\0{}\0{}",
            run.owner_scope, run.tenant, run.graph, run.source
        )
        .as_bytes(),
    );
    std::path::Path::new(persist_dir)
        .join("query_log_cursor")
        .join(format!("{}.json", hex::encode(digest)))
}

/// Read a cursor; a missing file is the start of the log.
pub fn load_cursor(path: &std::path::Path) -> Result<QueryLogCursor, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| "QUERY_LOG_UNAVAILABLE: the watermark record is unreadable".to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(QueryLogCursor::default()),
        Err(_) => Err("QUERY_LOG_UNAVAILABLE: the watermark record cannot be read".to_string()),
    }
}

/// Write a cursor atomically (temporary file, then rename).
pub fn save_cursor(path: &std::path::Path, cursor: &QueryLogCursor) -> Result<(), String> {
    fn unavailable<E>(_: E) -> String {
        "QUERY_LOG_UNAVAILABLE: the watermark cannot be saved".to_string()
    }
    let parent = path.parent().ok_or_else(|| unavailable(()))?;
    std::fs::create_dir_all(parent).map_err(unavailable)?;
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec(cursor).map_err(unavailable)?;
    std::fs::write(&tmp, body).map_err(unavailable)?;
    std::fs::rename(&tmp, path).map_err(unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R764.7
    #[test]
    fn a_replay_of_the_same_log_state_adds_nothing() {
        let mut cursor = QueryLogCursor::default();
        assert_eq!(cursor.advance(Some("u:d:q"), 5, 100), 5);
        assert_eq!(cursor.advance(Some("u:d:q"), 5, 200), 0);
        assert_eq!(cursor.advance(Some("u:d:q"), 8, 300), 3);
        assert_eq!(cursor.advance(None, 1, 250), 1);
        assert_eq!(cursor.watermark_ms, 300);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R764.7
    #[test]
    fn the_cursor_round_trips_through_its_file() {
        let dir = std::env::temp_dir().join(format!("qlog-{}", std::process::id()));
        let run = QueryLogRun {
            owner_scope: "o".into(),
            tenant: "t".into(),
            graph: "g".into(),
            source: "s".into(),
        };
        let path = cursor_path(dir.to_str().unwrap(), &run);
        assert_eq!(load_cursor(&path).unwrap(), QueryLogCursor::default());
        let mut cursor = QueryLogCursor::default();
        cursor.advance(Some("k"), 2, 9);
        save_cursor(&path, &cursor).unwrap();
        assert_eq!(load_cursor(&path).unwrap(), cursor);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
