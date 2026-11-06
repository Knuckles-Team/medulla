//! What crossed the network, per remote fragment (design §7) — the EXPLAIN/PROFILE record.

use serde::Serialize;

use super::snapshot::Consistency;
pub use medulla_types::metadata_acceleration::FragmentExecutionMode;
use medulla_types::metadata_acceleration::{SnapshotIsolation, SnapshotSemantics};

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
    /// The statement pushed to the source, when the fragment came from a graph view.
    pub pushed_statement: Option<String>,
    /// SHA-256 of the fragment's mapping, 64 lowercase hex characters (MEDULLA-UNIFIED-DATA-PLANE-R038.6.5).
    pub mapping_digest: Option<String>,
}

impl FragmentTrace {
    pub(crate) fn new(
        source: String,
        strategy: FetchStrategy,
        mode: FragmentExecutionMode,
    ) -> Self {
        Self {
            source,
            strategy,
            mode,
            watermark: None,
            copy_rows: None,
            snapshot: None,
            read_at_ms: 0,
            consistency: Consistency::Snapshot,
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

/// ` mapping_digest=… pushed_statement=…` for a view fragment. A digest that is not
/// 64 lowercase hex characters is not printed.
fn view_words(t: &FragmentTrace) -> String {
    let digest = t
        .mapping_digest
        .as_deref()
        .filter(|d| d.len() == 64 && d.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        .map(|d| format!(" mapping_digest={d}"));
    let statement = t
        .pushed_statement
        .as_deref()
        .map(|s| format!(" pushed_statement={s:?}"));
    digest.into_iter().chain(statement).collect()
}

/// One line per fragment for a log/EXPLAIN view, without registered source names.
pub fn render_trace(trace: &[FragmentTrace]) -> String {
    trace
        .iter()
        .map(|t| {
            format!(
                "{} {:?} keys={} limit={:?} requests={} fetched={} kept={} ms={} estimate={:?} mode={}{}{}{}",
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
                view_words(t)
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

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.6.5
    #[test]
    fn view_fragment_trace_prints_statement_and_digest() {
        let mut fragment = FragmentTrace::new(
            "sql:registered#deadbeef".into(),
            FetchStrategy::FullFetch,
            FragmentExecutionMode::Live,
        );
        fragment.pushed_statement = Some("SELECT * FROM \"person\"".into());
        fragment.mapping_digest = Some("ab".repeat(32));
        let output = render_trace(&[fragment.clone()]);
        assert!(
            output.contains(&format!(" mapping_digest={}", "ab".repeat(32))),
            "{output}"
        );
        assert!(
            output.contains(" pushed_statement=\"SELECT * FROM \\\"person\\\"\""),
            "{output}"
        );
        fragment.mapping_digest = Some("AB".repeat(32));
        assert!(!render_trace(&[fragment]).contains("mapping_digest="));
    }
}
