//! SQL column profiles carried by a source record
//! (MEDULLA-DECISION-ENGINE-R079.4, R079.6).
//!
//! A SQL connector describes each column of a record it delivers with a
//! [`SqlColumnProfile`]. Medulla alone decides which column values enter the
//! embedding set (`ingestion_admission::admit_sql_column`) and reports the
//! decision in the ingestion receipt: every excluded column with its reason
//! ([`ColumnExclusion`]) and every column value that survived
//! ([`EmbeddingSetEntry`]).

use serde::{Deserialize, Serialize};

use medulla_types::contract::ResourceId;

/// One in Q32 fixed point: a distinct ratio of `Q32_ONE` means every sampled
/// value was different.
pub const Q32_ONE: u64 = 1 << 32;
/// The longest column name a profile may carry.
pub const MAX_COLUMN_NAME_BYTES: usize = 256;

/// The kind of a SQL column, as the connector classifies its declared type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SqlColumnType {
    /// Character data: free text, or an enumeration when few values repeat.
    Text,
    Boolean,
    Timestamp,
    /// Integer, decimal and floating-point types.
    Numeric,
}

/// What the connector measured about one column of one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SqlColumnProfile {
    pub sql_type: SqlColumnType,
    pub is_foreign_key: bool,
    /// Distinct values over sampled values, in Q32 fixed point (at most
    /// [`Q32_ONE`]).
    pub distinct_ratio_q32: u64,
    /// How many values the ratio was measured over.
    pub sample_count: u32,
}

/// Why a column's values stay out of the embedding set. Closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ColumnExclusionReason {
    ForeignKey,
    Boolean,
    Timestamp,
    Numeric,
    /// Text whose distinct ratio is below `embedding.column_min_distinct_ratio`.
    Enumeration,
    /// Free text measured over fewer than `embedding.column_min_samples` values.
    InsufficientSamples,
}

/// The decision for one column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision", content = "reason")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ColumnAdmission {
    Admitted,
    Excluded(ColumnExclusionReason),
}

/// One excluded column of a stream, named in the ingestion receipt.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ColumnExclusion {
    pub stream: ResourceId,
    pub column: String,
    pub reason: ColumnExclusionReason,
}

/// One column value of one record that entered the embedding set.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EmbeddingSetEntry {
    pub stream: ResourceId,
    pub record_id: String,
    pub column: String,
}

impl SqlColumnProfile {
    /// Refuse a profile no connector can measure.
    pub fn validate(&self, column: &str) -> Result<(), String> {
        let code = medulla_types::contract::ServerErrorCode::SourceColumnProfileInvalid.as_str();
        if column.trim().is_empty() || column.len() > MAX_COLUMN_NAME_BYTES {
            return Err(format!(
                "{code}: a column name must be 1 to {MAX_COLUMN_NAME_BYTES} bytes"
            ));
        }
        if self.distinct_ratio_q32 > Q32_ONE {
            return Err(format!(
                "{code}: column `{column}` distinct ratio exceeds one"
            ));
        }
        Ok(())
    }
}
