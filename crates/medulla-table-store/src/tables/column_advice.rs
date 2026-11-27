//! Workload-first column vectorization advice
//! (MEDULLA-FEDERATED-QUERY-R077.6.2).
//!
//! [`rank_columns`] is a pure function over resolved inputs: it holds no store
//! handle, reads no counters and never creates a binding.

use std::collections::HashMap;
use std::sync::Mutex;

use medulla_tables::schema::{ColumnType, TableSchema};
use sha2::{Digest, Sha256};

use crate::tables::embedding_binding::{
    is_embeddable, AdviceEligibility, AdviceReceipt, ColumnVectorAdvice,
};

/// Reason carried by a row whose column type cannot be embedded.
pub const REASON_COLUMN_TYPE_UNSUPPORTED: &str = "COLUMN_TYPE_UNSUPPORTED";
/// Reason carried by a row whose counters belong to another schema revision.
pub const REASON_SCHEMA_REVISION_STALE: &str = "SCHEMA_REVISION_STALE";
/// Reason carried by a row without enough observed text-search uses.
pub const REASON_INSUFFICIENT_OBSERVATIONS: &str = "INSUFFICIENT_OBSERVATIONS";
/// Reason carried by a ranked row.
pub const REASON_WORKLOAD_RANKED: &str = "TEXT_SEARCH_WORKLOAD";

/// Closed refusal codes of column advice (MEDULLA-FEDERATED-QUERY-R077.6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnAdviceError {
    /// The pinned column's type fails `is_embeddable`.
    ColumnTypeUnsupported,
    /// The pinned column's counters belong to another schema revision.
    SchemaRevisionStale,
}

impl ColumnAdviceError {
    /// The wire refusal code.
    pub const fn code(self) -> &'static str {
        match self {
            Self::ColumnTypeUnsupported => REASON_COLUMN_TYPE_UNSUPPORTED,
            Self::SchemaRevisionStale => REASON_SCHEMA_REVISION_STALE,
        }
    }
}

impl std::fmt::Display for ColumnAdviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for ColumnAdviceError {}

/// One column's observations.
#[derive(Debug, Clone, PartialEq)]
pub struct AdviceColumn {
    pub table: String,
    pub column: String,
    pub ty: ColumnType,
    pub observed_schema_revision: u64,
    pub text_search_uses: u64,
    pub avg_length: f64,
    pub distinct_ratio: f64,
}

/// Everything `rank_columns` reads.
#[derive(Debug, Clone, PartialEq)]
pub struct AdviceInput {
    pub schema_revision: u64,
    pub window_start: i64,
    pub window_end: i64,
    pub columns: Vec<AdviceColumn>,
}

/// Resolved knob values; the table-store crate cannot read the registry.
#[derive(Debug, Clone, PartialEq)]
pub struct AdvicePolicy {
    pub min_samples: u64,
    pub w_demand: f64,
    pub w_length: f64,
    pub w_distinct: f64,
    pub length_norm: f64,
    pub max_rows: usize,
}

impl AdvicePolicy {
    /// The policy revision a receipt names: every knob the score reads, so a changed knob is a
    /// changed revision (MEDULLA-FEDERATED-QUERY-R077.6.5).
    pub fn revision(&self) -> String {
        format!(
            "sql_advice/v1 min_samples={} w_demand={} w_length={} w_distinct={} length_norm={} max_rows={}",
            self.min_samples,
            self.w_demand,
            self.w_length,
            self.w_distinct,
            self.length_norm,
            self.max_rows
        )
    }
}

/// Rank columns by observed text-search workload.
///
/// Type admission runs first with [`is_embeddable`]. Score is
/// `w_demand * share + w_length * min(avg_length / length_norm, 1) +
/// w_distinct * distinct_ratio`, where `share` is the column's uses over the
/// sum of all uses in the input. A positive score needs at least one use.
/// Fewer than `min_samples` total uses makes every admitted row
/// `insufficient_observations` with score 0. Ties break by `(table, column)`.
pub fn rank_columns(
    input: &AdviceInput,
    policy: &AdvicePolicy,
) -> Result<Vec<ColumnVectorAdvice>, ColumnAdviceError> {
    let total: u64 = input.columns.iter().map(|c| c.text_search_uses).sum();
    let enough = total >= policy.min_samples;
    let policy_revision = policy.revision();
    let mut rows: Vec<ColumnVectorAdvice> = input
        .columns
        .iter()
        .map(|column| {
            let (eligibility, reason, parts) = if !is_embeddable(column.ty) {
                (
                    AdviceEligibility::UnsupportedType,
                    REASON_COLUMN_TYPE_UNSUPPORTED,
                    (0.0, 0.0, 0.0),
                )
            } else if column.observed_schema_revision != input.schema_revision {
                (
                    AdviceEligibility::InsufficientObservations,
                    REASON_SCHEMA_REVISION_STALE,
                    (0.0, 0.0, 0.0),
                )
            } else if !enough || column.text_search_uses == 0 {
                (
                    AdviceEligibility::InsufficientObservations,
                    REASON_INSUFFICIENT_OBSERVATIONS,
                    (0.0, 0.0, 0.0),
                )
            } else {
                let share = column.text_search_uses as f64 / total as f64;
                let length = (column.avg_length / policy.length_norm).min(1.0);
                (
                    AdviceEligibility::Eligible,
                    REASON_WORKLOAD_RANKED,
                    (
                        policy.w_demand * share,
                        policy.w_length * length,
                        policy.w_distinct * column.distinct_ratio,
                    ),
                )
            };
            let (demand, length, distinct) = parts;
            ColumnVectorAdvice {
                table: column.table.clone(),
                column: column.column.clone(),
                schema_revision: input.schema_revision,
                window_start: input.window_start,
                window_end: input.window_end,
                sample_count: column.text_search_uses,
                eligibility,
                reason: reason.to_string(),
                score: demand + length + distinct,
                receipt: AdviceReceipt {
                    total_text_search_uses: total,
                    policy_revision: policy_revision.clone(),
                    demand,
                    length,
                    distinct,
                },
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.table.cmp(&b.table))
            .then_with(|| a.column.cmp(&b.column))
    });
    rows.truncate(policy.max_rows);
    Ok(rows)
}

/// Admit a pinned read of one column (MEDULLA-FEDERATED-QUERY-R077.6.4). A column
/// that fails `is_embeddable` refuses `COLUMN_TYPE_UNSUPPORTED`; counters observed
/// under another schema revision than `schema_revision` refuse
/// `SCHEMA_REVISION_STALE`. A listing never calls this: it shows such columns as
/// rows. Advice checks no embedding model; binding admission owns that.
pub fn admit_pinned(column: &AdviceColumn, schema_revision: u64) -> Result<(), ColumnAdviceError> {
    if !is_embeddable(column.ty) {
        return Err(ColumnAdviceError::ColumnTypeUnsupported);
    }
    if column.observed_schema_revision != schema_revision {
        return Err(ColumnAdviceError::SchemaRevisionStale);
    }
    Ok(())
}

/// The resolved settings one tenant's advice reads. The table-store crate
/// cannot read the knob registry, so the serving crate installs these through
/// [`ColumnUsageCounters::configure`]; `Default` carries the knob defaults for
/// stores that are never served (MEDULLA-FEDERATED-QUERY-R077.6.3).
#[derive(Debug, Clone, PartialEq)]
pub struct AdviceSettings {
    pub policy: AdvicePolicy,
    /// `sql_advice.window_hours`, in milliseconds.
    pub window_ms: i64,
    /// `sql_advice.max_columns`.
    pub max_columns: usize,
    /// `sql_advice.stats_sample_rows`.
    pub stats_sample_rows: usize,
}

impl Default for AdviceSettings {
    fn default() -> Self {
        Self {
            policy: AdvicePolicy {
                min_samples: 50,
                w_demand: 0.6,
                w_length: 0.25,
                w_distinct: 0.15,
                length_norm: 512.0,
                max_rows: 1000,
            },
            window_ms: 24 * 3_600_000,
            max_columns: 100_000,
            stats_sample_rows: 10_000,
        }
    }
}

/// One `(table, column)` entry of the usage counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageObservation {
    pub table: String,
    pub column: String,
    pub observed_schema_revision: u64,
    pub text_search_uses: u64,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
}

#[derive(Debug, Default)]
struct UsageState {
    settings: AdviceSettings,
    entries: HashMap<(String, String), UsageObservation>,
}

/// Process-local text-search usage statistics of one tenant's tables
/// (MEDULLA-FEDERATED-QUERY-R077.6.3). They are not facts: a restart opens a
/// new window. One instance is created with the tenant's cached `TableStore`
/// and shared by every clone of it.
#[derive(Debug, Default)]
pub struct ColumnUsageCounters {
    state: Mutex<UsageState>,
}

impl ColumnUsageCounters {
    fn locked(&self) -> std::sync::MutexGuard<'_, UsageState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Install the resolved knob values.
    pub fn configure(&self, settings: AdviceSettings) {
        self.locked().settings = settings;
    }

    /// The installed settings.
    pub fn settings(&self) -> AdviceSettings {
        self.locked().settings.clone()
    }

    /// Count one text-search use of each `(table, column)` pair. An entry that
    /// was observed under another schema revision restarts. The map is bounded
    /// by `max_columns` and evicts the least recently seen entry.
    pub fn observe(&self, pairs: &[(String, String)], schema_revision: u64, now_ms: i64) {
        let mut state = self.locked();
        for (table, column) in pairs {
            let key = (table.clone(), column.clone());
            let max_columns = state.settings.max_columns;
            if !state.entries.contains_key(&key) && state.entries.len() >= max_columns {
                let oldest = state
                    .entries
                    .iter()
                    .min_by(|a, b| {
                        a.1.last_seen_ms
                            .cmp(&b.1.last_seen_ms)
                            .then_with(|| a.0.cmp(b.0))
                    })
                    .map(|(key, _)| key.clone());
                if let Some(oldest) = oldest {
                    state.entries.remove(&oldest);
                }
            }
            let entry = state
                .entries
                .entry(key)
                .or_insert_with(|| UsageObservation {
                    table: table.clone(),
                    column: column.clone(),
                    observed_schema_revision: schema_revision,
                    text_search_uses: 0,
                    first_seen_ms: now_ms,
                    last_seen_ms: now_ms,
                });
            if entry.observed_schema_revision != schema_revision {
                entry.observed_schema_revision = schema_revision;
                entry.text_search_uses = 0;
                entry.first_seen_ms = now_ms;
            }
            entry.text_search_uses = entry.text_search_uses.saturating_add(1);
            entry.last_seen_ms = now_ms;
        }
    }

    /// The entries still inside the window ending at `now_ms`; older entries
    /// are dropped on read.
    pub fn read(&self, now_ms: i64) -> Vec<UsageObservation> {
        let mut state = self.locked();
        let window_start = now_ms.saturating_sub(state.settings.window_ms);
        state
            .entries
            .retain(|_, entry| entry.last_seen_ms >= window_start);
        let mut out: Vec<UsageObservation> = state.entries.values().cloned().collect();
        out.sort_by(|a, b| (&a.table, &a.column).cmp(&(&b.table, &b.column)));
        out
    }
}

/// The schema revision of a set of tables: a digest of their schemas, so any
/// DDL change moves it, in the projection of a read and in the tenant store
/// alike.
pub fn catalog_schema_revision(tables: &[TableSchema]) -> Result<u64, String> {
    let mut digests = tables
        .iter()
        .map(TableSchema::schema_digest)
        .collect::<Result<Vec<_>, _>>()?;
    digests.sort();
    let mut hasher = Sha256::new();
    hasher.update(b"epistemic-graph/column-advice-revision\0");
    for digest in &digests {
        hasher.update(digest.as_bytes());
    }
    let hash = hasher.finalize();
    let mut head = [0_u8; 8];
    head.copy_from_slice(&hash[..8]);
    Ok(u64::from_be_bytes(head))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, uses: u64) -> AdviceColumn {
        AdviceColumn {
            table: "docs".to_string(),
            column: name.to_string(),
            ty: ColumnType::Text,
            observed_schema_revision: 3,
            text_search_uses: uses,
            avg_length: 100.0,
            distinct_ratio: 0.5,
        }
    }

    fn policy() -> AdvicePolicy {
        AdvicePolicy {
            min_samples: 10,
            w_demand: 0.6,
            w_length: 0.25,
            w_distinct: 0.15,
            length_norm: 512.0,
            max_rows: 100,
        }
    }

    fn input(columns: Vec<AdviceColumn>) -> AdviceInput {
        AdviceInput {
            schema_revision: 3,
            window_start: 0,
            window_end: 1000,
            columns,
        }
    }

    /// MEDULLA-FEDERATED-QUERY-R077.6.4
    #[test]
    fn pinned_reads_refuse_per_question() {
        let mut int = column("n", 1);
        int.ty = ColumnType::BigInt;
        assert_eq!(
            admit_pinned(&int, 3),
            Err(ColumnAdviceError::ColumnTypeUnsupported)
        );
        let mut stale = column("a", 1);
        stale.observed_schema_revision = 2;
        assert_eq!(
            admit_pinned(&stale, 3),
            Err(ColumnAdviceError::SchemaRevisionStale)
        );
        assert_eq!(admit_pinned(&column("a", 1), 3), Ok(()));
        // a listing never refuses on account of another column
        let listed = rank_columns(&input(vec![int, stale, column("a", 60)]), &policy()).unwrap();
        assert_eq!(listed.len(), 3);
    }

    /// MEDULLA-FEDERATED-QUERY-R077.6.2
    #[test]
    fn traffic_changes_rank_not_statistics() {
        let first = rank_columns(&input(vec![column("a", 30), column("b", 5)]), &policy()).unwrap();
        assert_eq!(first[0].column, "a");
        assert_eq!(first[0].eligibility, AdviceEligibility::Eligible);
        // identical statistics, opposite traffic: the order flips
        let second =
            rank_columns(&input(vec![column("a", 5), column("b", 30)]), &policy()).unwrap();
        assert_eq!(second[0].column, "b");

        // fewer than min_samples total uses: everything is insufficient, score 0
        let few = rank_columns(&input(vec![column("a", 3), column("b", 2)]), &policy()).unwrap();
        assert!(few.iter().all(
            |r| r.score == 0.0 && r.eligibility == AdviceEligibility::InsufficientObservations
        ));

        // an integer column is unsupported and sinks
        let mut int = column("n", 100);
        int.ty = ColumnType::BigInt;
        let mixed = rank_columns(&input(vec![int, column("a", 20)]), &policy()).unwrap();
        assert_eq!(mixed[0].column, "a");
        assert_eq!(mixed[1].eligibility, AdviceEligibility::UnsupportedType);
        assert_eq!(mixed[1].reason, "COLUMN_TYPE_UNSUPPORTED");

        // ties break by (table, column)
        let tie = rank_columns(&input(vec![column("z", 10), column("y", 10)]), &policy()).unwrap();
        assert_eq!(tie[0].column, "y");
    }

    /// MEDULLA-FEDERATED-QUERY-R077.6.3
    #[test]
    fn counters_are_bounded_restart_on_ddl_and_drop_old_uses() {
        let counters = ColumnUsageCounters::default();
        let mut settings = AdviceSettings::default();
        settings.max_columns = 2;
        settings.window_ms = 1000;
        counters.configure(settings);
        let pair = |c: &str| vec![("docs".to_string(), c.to_string())];
        counters.observe(&pair("a"), 1, 10);
        counters.observe(&pair("a"), 1, 20);
        counters.observe(&pair("b"), 1, 30);
        counters.observe(&pair("c"), 1, 40);
        let read = counters.read(50);
        assert_eq!(
            read.iter().map(|e| e.column.as_str()).collect::<Vec<_>>(),
            ["b", "c"]
        );
        counters.observe(&pair("b"), 2, 60);
        let b = counters
            .read(60)
            .into_iter()
            .find(|e| e.column == "b")
            .unwrap();
        assert_eq!((b.observed_schema_revision, b.text_search_uses), (2, 1));
        assert!(counters.read(5000).is_empty());
    }
}
