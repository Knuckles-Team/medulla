//! Metastore scan (MEDULLA-UNIFIED-DATA-PLANE-R038.4.2.1, .4.4, .4.5): partition pruning by a
//! pushed filter, the typed refusals that read zero data files, and the exact scan receipt.

use serde::{Deserialize, Serialize};

use super::{
    FileStats, LakeFileFormat, MetastoreClient, MetastoreColumn, MetastoreError, MetastoreTable,
    PartitionInfo,
};

/// The query-warnings key the receipt is returned under.
pub const RECEIPT_WARNING_KEY: &str = "hive_scan_receipt";

/// Exact counters of what one scan opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanReceipt {
    pub partitions_pruned: u64,
    pub partitions_read: u64,
    pub files_read: u64,
    pub stripes_read: u64,
    pub stripes_skipped: u64,
    pub bytes_read: u64,
}

/// What opening one data file did: stripes it held, stripes read, bytes read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenedFile {
    pub stripes_total: u64,
    pub stripes_read: u64,
    pub bytes_read: u64,
}

/// The data-file port. `writer_schema` reads only a file header and is not a data-file read.
pub trait DataFileSource {
    fn writer_schema(&self, file: &FileStats) -> Result<Vec<MetastoreColumn>, MetastoreError>;
    fn open(&self, file: &FileStats) -> Result<OpenedFile, MetastoreError>;
}

/// Row limits of one scan: `metastore.list_page_size` and `metastore.max_partitions`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanLimits {
    pub list_page_size: u64,
    pub max_partitions: u64,
}

/// One equality predicate pushed onto a partition key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartitionPredicate {
    pub key: String,
    pub value: String,
}

/// The metastore filter string for pushed predicates; the caller keeps the exact residual.
pub fn partition_filter(predicates: &[PartitionPredicate]) -> String {
    let clause = |p: &PartitionPredicate| format!("{} = '{}'", p.key, p.value.replace('\'', "\\'"));
    predicates
        .iter()
        .map(clause)
        .collect::<Vec<_>>()
        .join(" and ")
}

/// Scans one metastore table. `source_label` names the source in every refusal.
pub struct MetastoreTableProvider<'a> {
    pub client: &'a dyn MetastoreClient,
    pub files: &'a dyn DataFileSource,
    pub source_label: String,
    pub database: String,
    pub table: String,
    pub limits: ScanLimits,
}

impl MetastoreTableProvider<'_> {
    /// Validate, prune, then read: every refusal happens before the first data file opens.
    pub fn scan(&self, filter: &str) -> Result<ScanReceipt, MetastoreError> {
        let table = self.client_call(|c| c.table(&self.database, &self.table))?;
        check_layout(&table)?;
        let matched =
            self.client_call(|c| c.partitions_by_filter(&self.database, &self.table, filter))?;
        let found = matched.len() as u64;
        if found > self.limits.max_partitions {
            return Err(MetastoreError::PartitionLimit {
                limit: self.limits.max_partitions,
                found,
            });
        }
        self.check_schemas(&table, &matched)?;
        let mut receipt = ScanReceipt {
            partitions_pruned: (table.partitions.len() as u64).saturating_sub(found),
            partitions_read: found,
            ..ScanReceipt::default()
        };
        for file in matched.iter().flat_map(|p| &p.files) {
            let opened = self.files.open(file).map_err(|e| self.relabel(e))?;
            receipt.files_read += 1;
            receipt.stripes_read += opened.stripes_read;
            receipt.stripes_skipped += opened.stripes_total.saturating_sub(opened.stripes_read);
            receipt.bytes_read += opened.bytes_read;
        }
        Ok(receipt)
    }

    fn client_call<T>(
        &self,
        call: impl FnOnce(&dyn MetastoreClient) -> Result<T, MetastoreError>,
    ) -> Result<T, MetastoreError> {
        call(self.client).map_err(|e| self.relabel(e))
    }

    /// An unreachable catalog or file names this source, never the endpoint.
    fn relabel(&self, error: MetastoreError) -> MetastoreError {
        match error {
            MetastoreError::SourceUnreachable { .. } => MetastoreError::SourceUnreachable {
                source_label: self.source_label.clone(),
            },
            other => other,
        }
    }

    fn check_schemas(
        &self,
        table: &MetastoreTable,
        partitions: &[PartitionInfo],
    ) -> Result<(), MetastoreError> {
        if table.format != LakeFileFormat::Avro {
            return Ok(());
        }
        for file in partitions.iter().flat_map(|p| &p.files) {
            let writer = self
                .files
                .writer_schema(file)
                .map_err(|e| self.relabel(e))?;
            resolve_schema(&table.columns, &writer)?;
        }
        Ok(())
    }
}

/// A transactional delta layout needs a readable `base_*` directory among its files.
fn check_layout(table: &MetastoreTable) -> Result<(), MetastoreError> {
    let paths: Vec<&str> = table
        .partitions
        .iter()
        .flat_map(|p| &p.files)
        .map(|f| f.path.as_str())
        .collect();
    let has = |marker: &str| paths.iter().any(|p| p.contains(marker));
    if (has("/delta_") || has("/delete_delta_")) && !has("/base_") {
        return Err(MetastoreError::UnsupportedLayout(format!(
            "{}.{} has delta directories and no readable base",
            table.database, table.name
        )));
    }
    Ok(())
}

/// Avro schema resolution: each reader column must exist in the writer with the same type or
/// a lossless promotion.
fn resolve_schema(
    reader: &[MetastoreColumn],
    writer: &[MetastoreColumn],
) -> Result<(), MetastoreError> {
    for column in reader {
        let found = writer.iter().find(|w| w.name == column.name);
        if !found.is_some_and(|w| promotes(&w.data_type, &column.data_type)) {
            return Err(MetastoreError::SchemaIncompatible(format!(
                "column {} cannot be resolved against the writer schema",
                column.name
            )));
        }
    }
    Ok(())
}

const PROMOTIONS: &[(&str, &str)] = &[
    ("int", "bigint"),
    ("int", "float"),
    ("int", "double"),
    ("bigint", "float"),
    ("bigint", "double"),
    ("float", "double"),
    ("string", "binary"),
    ("binary", "string"),
];

fn promotes(writer: &str, reader: &str) -> bool {
    writer == reader || PROMOTIONS.contains(&(writer, reader))
}
