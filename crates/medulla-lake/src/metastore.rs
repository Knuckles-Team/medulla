//! Hive-metastore catalog model (MEDULLA-UNIFIED-DATA-PLANE-R038.4.1): the catalog
//! spec, file formats, partition and file statistics, the table description with its
//! content hash, the client port and the typed refusals of a metastore scan.

use std::collections::BTreeMap;

use medulla_types::contract::ServerErrorCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub mod scan;

/// Secret-reference schemes an `auth_ref` may use; anything else is an inline secret.
const AUTH_REF_SCHEMES: &[&str] = &["env://", "openbao://"];

/// One registered metastore catalog. `auth_ref` is a secret reference, never a secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetastoreCatalogSpec {
    pub endpoint: String,
    pub database_filter: Option<String>,
    pub auth_ref: String,
}

impl MetastoreCatalogSpec {
    /// Refuses an empty endpoint and an `auth_ref` that is not a secret reference.
    pub fn validate(&self) -> Result<(), MetastoreError> {
        let is_reference = AUTH_REF_SCHEMES.iter().any(|scheme| {
            self.auth_ref
                .strip_prefix(scheme)
                .is_some_and(|r| !r.is_empty())
        });
        match (self.endpoint.trim().is_empty(), is_reference) {
            (true, _) => Err(MetastoreError::SpecInvalid("endpoint is empty".into())),
            (_, false) => Err(MetastoreError::SpecInvalid(
                "auth_ref must be a secret reference, not an inline secret".into(),
            )),
            _ => Ok(()),
        }
    }
}

/// Data-file formats a metastore table can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LakeFileFormat {
    Parquet,
    Orc,
    Avro,
}

/// Statistics of one data file; `column_min_max` maps a column to its (min, max).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileStats {
    pub path: String,
    pub bytes: u64,
    pub rows: u64,
    pub column_min_max: BTreeMap<String, (String, String)>,
}

/// One partition: its key values (in `partition_keys` order), location and files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartitionInfo {
    pub values: Vec<String>,
    pub location: String,
    pub files: Vec<FileStats>,
}

/// A column name and its declared type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetastoreColumn {
    pub name: String,
    pub data_type: String,
}

/// A metastore table with its partitions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetastoreTable {
    pub database: String,
    pub name: String,
    pub columns: Vec<MetastoreColumn>,
    pub partition_keys: Vec<String>,
    pub format: LakeFileFormat,
    pub location: String,
    pub partitions: Vec<PartitionInfo>,
}

impl MetastoreTable {
    /// SHA-256 over the canonical (sorted) JSON bytes: 64 lowercase hex characters.
    /// It fills `MetadataContract.content_hash`.
    pub fn content_hash(&self) -> String {
        let mut canonical = self.clone();
        canonical.columns.sort_by(|a, b| a.name.cmp(&b.name));
        canonical.partition_keys.sort();
        canonical.partitions.sort_by(|a, b| a.values.cmp(&b.values));
        for partition in &mut canonical.partitions {
            partition.files.sort_by(|a, b| a.path.cmp(&b.path));
        }
        // Serialising plain data with string-keyed maps cannot fail.
        let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// One page of table names; `next` is the cursor of the following page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TablePage {
    pub tables: Vec<String>,
    pub next: Option<String>,
}

/// The typed refusals of a metastore read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetastoreError {
    SpecInvalid(String),
    SchemaIncompatible(String),
    UnsupportedLayout(String),
    /// The catalog did not answer. The label names the source, never the endpoint.
    SourceUnreachable {
        source_label: String,
    },
    PartitionLimit {
        limit: u64,
        found: u64,
    },
}

impl MetastoreError {
    /// The registered refusal code this error is reported under.
    pub fn code(&self) -> ServerErrorCode {
        match self {
            Self::SpecInvalid(_) => ServerErrorCode::MetastoreSpecInvalid,
            Self::SchemaIncompatible(_) => ServerErrorCode::SchemaIncompatible,
            Self::UnsupportedLayout(_) => ServerErrorCode::UnsupportedTableLayout,
            Self::SourceUnreachable { .. } => ServerErrorCode::SourceUnreachable,
            Self::PartitionLimit { .. } => ServerErrorCode::PartitionLimitExceeded,
        }
    }
}

impl core::fmt::Display for MetastoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let detail = match self {
            Self::SpecInvalid(d) | Self::SchemaIncompatible(d) | Self::UnsupportedLayout(d) => {
                d.clone()
            }
            Self::SourceUnreachable { source_label } => format!("source {source_label}"),
            Self::PartitionLimit { limit, found } => format!("{found} partitions over {limit}"),
        };
        write!(f, "{}: {detail}", self.code())
    }
}

impl std::error::Error for MetastoreError {}

/// The metastore port. The production implementation speaks the metastore wire protocol;
/// tests use a recorded-response fake.
pub trait MetastoreClient {
    fn list_tables(&self, db: &str, page: Option<&str>) -> Result<TablePage, MetastoreError>;
    fn table(&self, db: &str, name: &str) -> Result<MetastoreTable, MetastoreError>;
    fn partitions_by_filter(
        &self,
        db: &str,
        name: &str,
        filter: &str,
    ) -> Result<Vec<PartitionInfo>, MetastoreError>;
}
