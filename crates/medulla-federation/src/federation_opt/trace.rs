//! What crossed the network, per remote fragment (design §7) — the EXPLAIN/PROFILE record.

use serde::Serialize;

use super::snapshot::Consistency;
pub use medulla_types::metadata_acceleration::FragmentExecutionMode;
use medulla_types::metadata_acceleration::{SnapshotIsolation, SnapshotSemantics};
use medulla_types::search_source::ScoreKind;

/// How a fragment was fetched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FetchStrategy {
    /// The whole source, then local evaluation (no capability, or cheaper by estimate).
    FullFetch,
    /// A `Limit k` pushed into the source's request.
    LimitPushdown,
    /// A pushed limit returned fewer distinct ids than `k`; refilled from a full fetch.
    LimitRefill,
    /// Read page by page.
    Paged,
    /// Local ids shipped as batched key lookups.
    BindJoin,
    /// A pushed request failed; the fragment fell back to the naive full fetch.
    FallbackFullFetch,
}

/// Where the cardinality estimate behind a strategy choice came from.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum EstimateProvenance {
    /// No estimate was needed (a source op, or no capability to choose between).
    NotNeeded,
    /// No execution had observed the source yet.
    Default,
    /// Learned from `samples` past full fetches averaging `rows` rows.
    Learned { samples: u64, rows: f64 },
}

/// One remote fragment of one query. Never carries a URL, DSN, credential or key value —
/// the source is named by kind, registered name and an 8-hex fingerprint.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FragmentTrace {
    /// `<kind>[:<registered name>]#<fingerprint>`.
    pub source: String,
    pub strategy: FetchStrategy,
    /// Distinct local ids shipped as keys (bind join).
    pub keys_pushed: usize,
    /// The limit pushed into the source, if any.
    pub limit_pushed: Option<usize>,
    /// Round trips.
    pub requests: u32,
    /// Rows received from the source.
    pub rows_fetched: usize,
    /// Rows the fragment handed to the rest of the plan.
    pub rows_kept: usize,
    /// Wall time spent in the fragment.
    pub elapsed_ms: u64,
    pub estimate: EstimateProvenance,
    /// `live` or `copy`, set by the route that ran.
    pub mode: FragmentExecutionMode,
    /// The copy's watermark, when `mode` is `Copy`.
    pub watermark: Option<String>,
    /// Rows the copy served, when `mode` is `Copy`.
    pub copy_rows: Option<u64>,
    /// The snapshot every fragment of this source read, when one was opened.
    pub snapshot: Option<SnapshotSemantics>,
    /// Unix milliseconds at which the fragment started reading.
    pub read_at_ms: u64,
    /// How the query's fragments of this source relate in time.
    pub consistency: Consistency,
    /// The index request this fragment pushed, when it was a pushed search
    /// (MEDULLA-UNIFIED-DATA-PLANE-R038.5.5).
    pub search: Option<SearchReceipt>,
    /// The source returned more rows than its declared `max_results` and the engine cut the
    /// rest (MEDULLA-UNIFIED-DATA-PLANE-R038.7.4).
    pub truncated: bool,
    /// The one statement a graph-view read pushed to the source, credential-free
    /// (MEDULLA-UNIFIED-DATA-PLANE-R038.6.5).
    pub pushed_statement: Option<String>,
    /// SHA-256 of the mapping the fragment ran under, 64 lowercase hex characters.
    pub mapping_digest: Option<String>,
}

/// The receipt of one pushed search. The fragment's `source` label names the source; the
/// endpoint never appears.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SearchReceipt {
    /// SHA-256 over the canonical pushed request, 64 lowercase hex characters.
    pub query_digest: String,
    /// The `k` the request carried.
    pub k: u32,
    /// What the returned scores mean.
    pub score_kind: ScoreKind,
    /// Results the source returned across every page.
    pub returned: usize,
}

impl FragmentTrace {
    /// A fresh trace for one fragment; callers outside the crate (EXPLAIN of a
    /// graph-view read) fill the pushed statement and mapping digest after.
    pub fn new(source: String, strategy: FetchStrategy, mode: FragmentExecutionMode) -> Self {
        Self {
            source,
            strategy,
            mode,
            watermark: None,
            copy_rows: None,
            snapshot: None,
            read_at_ms: 0,
            consistency: Consistency::Snapshot,
            search: None,
            truncated: false,
            pushed_statement: None,
            mapping_digest: None,
            keys_pushed: 0,
            limit_pushed: None,
            requests: 0,
            rows_fetched: 0,
            rows_kept: 0,
            elapsed_ms: 0,
            estimate: EstimateProvenance::NotNeeded,
        }
    }
}

/// Drop a registry name from the optimizer's trusted `<kind>[:name]#<8 hex>` label.
/// Fail closed for malformed labels rather than echoing arbitrary caller text.
pub(crate) fn redacted_label(label: &str) -> String {
    let Some((prefix, digest)) = label.rsplit_once('#') else {
        return "source#unknown".into();
    };
    if digest.len() != 8 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return "source#unknown".into();
    }
    let kind = prefix.split(':').next().unwrap_or_default();
    if !matches!(
        kind,
        "named"
            | "registered"
            | "remote-engine"
            | "http-json"
            | "sql"
            | "trino"
            | "neo4j"
            | "age"
            | "falkordb"
            | "spark-batch"
    ) {
        return "source#unknown".into();
    }
    format!("{kind}#{digest}")
}

/// `live` or `copy`, as printed per fragment.
pub(crate) const fn mode_word(mode: FragmentExecutionMode) -> &'static str {
    match mode {
        FragmentExecutionMode::Live => "live",
        FragmentExecutionMode::Copy => "copy",
    }
}

const fn isolation_word(isolation: SnapshotIsolation) -> &'static str {
    match isolation {
        SnapshotIsolation::Snapshot => "snapshot",
        SnapshotIsolation::RepeatableRead => "repeatable_read",
    }
}

/// One line per fragment for a log/EXPLAIN view, without registered source names.
pub fn render_trace(trace: &[FragmentTrace]) -> String {
    trace
        .iter()
        .map(|t| {
            format!(
                "{} {:?} keys={} limit={:?} requests={} fetched={} kept={} ms={} estimate={:?} mode={}{}{}{}{}",
                redacted_label(&t.source),
                t.strategy,
                t.keys_pushed,
                t.limit_pushed,
                t.requests,
                t.rows_fetched,
                t.rows_kept,
                t.elapsed_ms,
                t.estimate,
                mode_word(t.mode),
                t.snapshot
                    .as_ref()
                    .map(|snap| format!(
                        " snapshot={} isolation={}",
                        snap.snapshot_token,
                        isolation_word(snap.isolation)
                    ))
                    .unwrap_or_default(),
                match t.consistency {
                    Consistency::PerFragment => " consistency=per_fragment",
                    Consistency::Snapshot => "",
                },
                t.pushed_statement
                    .as_ref()
                    .map(|statement| format!(" pushed={statement:?}"))
                    .unwrap_or_default(),
                t.mapping_digest
                    .as_ref()
                    .map(|digest| format!(" mapping_digest={digest}"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{render_trace, FetchStrategy, FragmentExecutionMode, FragmentTrace};

    #[test]
    fn telemetry_trace_redacts_registered_names_and_keeps_counts() {
        let mut fragment = FragmentTrace::new(
            "registered:https://user:redact-me@example.invalid/db#deadbeef".into(),
            FetchStrategy::BindJoin,
            FragmentExecutionMode::Live,
        );
        fragment.keys_pushed = 3;
        fragment.requests = 2;
        fragment.rows_fetched = 5;
        fragment.rows_kept = 4;
        let output = render_trace(&[fragment]);
        assert!(
            output.starts_with("registered#deadbeef BindJoin"),
            "{output}"
        );
        assert!(output.contains("keys=3 limit=None requests=2 fetched=5 kept=4"));
        assert!(!output.contains("redact-me") && !output.contains("example.invalid"));
    }

    #[test]
    fn render_trace_names_pushed_statement_and_mapping_digest() {
        let mut fragment = FragmentTrace::new(
            "sql:people#0a0b0c0d".into(),
            FetchStrategy::FullFetch,
            FragmentExecutionMode::Live,
        );
        fragment.pushed_statement = Some("SELECT \"name\" FROM \"people\"".into());
        fragment.mapping_digest = Some("ab12".repeat(16));
        let output = render_trace(&[fragment]);
        assert!(
            output.contains("pushed=\"SELECT \\\"name\\\" FROM \\\"people\\\"\""),
            "{output}"
        );
        assert!(output.contains(&format!("mapping_digest={}", "ab12".repeat(16))));
    }

    #[test]
    fn telemetry_trace_fails_closed_for_malformed_source_labels() {
        let fragment = FragmentTrace::new(
            "untrusted:redact-me".into(),
            FetchStrategy::FullFetch,
            FragmentExecutionMode::Live,
        );
        let output = render_trace(&[fragment]);
        assert!(output.starts_with("source#unknown FullFetch"), "{output}");
        assert!(!output.contains("redact-me"));
    }
}
