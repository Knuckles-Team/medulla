//! Workload-first column vectorization advice
//! (MEDULLA-FEDERATED-QUERY-R077.6.2).
//!
//! [`rank_columns`] is a pure function over resolved inputs: it holds no store
//! handle, reads no counters and never creates a binding.

use medulla_tables::schema::ColumnType;

use crate::tables::embedding_binding::{is_embeddable, AdviceEligibility, ColumnVectorAdvice};

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
    let mut rows: Vec<ColumnVectorAdvice> = input
        .columns
        .iter()
        .map(|column| {
            let (eligibility, reason, score) = if !is_embeddable(column.ty) {
                (
                    AdviceEligibility::UnsupportedType,
                    REASON_COLUMN_TYPE_UNSUPPORTED,
                    0.0,
                )
            } else if column.observed_schema_revision != input.schema_revision {
                (
                    AdviceEligibility::InsufficientObservations,
                    REASON_SCHEMA_REVISION_STALE,
                    0.0,
                )
            } else if !enough || column.text_search_uses == 0 {
                (
                    AdviceEligibility::InsufficientObservations,
                    REASON_INSUFFICIENT_OBSERVATIONS,
                    0.0,
                )
            } else {
                let share = column.text_search_uses as f64 / total as f64;
                let length = (column.avg_length / policy.length_norm).min(1.0);
                (
                    AdviceEligibility::Eligible,
                    REASON_WORKLOAD_RANKED,
                    policy.w_demand * share
                        + policy.w_length * length
                        + policy.w_distinct * column.distinct_ratio,
                )
            };
            ColumnVectorAdvice {
                table: column.table.clone(),
                column: column.column.clone(),
                schema_revision: input.schema_revision,
                window_start: input.window_start,
                window_end: input.window_end,
                sample_count: column.text_search_uses,
                eligibility,
                reason: reason.to_string(),
                score,
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
        assert!(few.iter().all(|r| r.score == 0.0
            && r.eligibility == AdviceEligibility::InsufficientObservations));

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
}
