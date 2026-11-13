//! Metastore scan (MEDULLA-UNIFIED-DATA-PLANE-R038.4.2.1, .4.4, .4.5): partition pruning by a
//! pushed filter, the typed refusals that read zero data files, and the exact scan receipt.

use arrow::array::RecordBatch;
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

impl ScanLimits {
    /// The limits of one scan, from the resolved `metastore.list_page_size` and
    /// `metastore.max_partitions` knob values.
    pub fn new(list_page_size: u64, max_partitions: u64) -> Self {
        Self {
            list_page_size,
            max_partitions,
        }
    }
}

/// Every table name of `db`, listed in pages of `limits.list_page_size`. A client that
/// answers a page larger than asked, or a cursor that never advances, is refused rather
/// than trusted, so a listing is bounded by the knob and never loops.
pub fn list_all_tables(
    client: &dyn MetastoreClient,
    db: &str,
    limits: ScanLimits,
) -> Result<Vec<String>, MetastoreError> {
    let mut tables = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = client.list_tables(db, cursor.as_deref(), limits.list_page_size)?;
        if page.tables.len() as u64 > limits.list_page_size {
            return Err(MetastoreError::SpecInvalid(format!(
                "a listing page held {} tables over the page size {}",
                page.tables.len(),
                limits.list_page_size
            )));
        }
        tables.extend(page.tables);
        match page.next {
            None => return Ok(tables),
            Some(next) if cursor.as_deref() == Some(next.as_str()) => {
                return Err(MetastoreError::SpecInvalid(
                    "the listing cursor did not advance".to_string(),
                ))
            }
            Some(next) => cursor = Some(next),
        }
    }
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

/// A data-file port that also returns the rows it kept.
pub trait RowFileSource: DataFileSource {
    /// Read one file: the batches of the stripes that survived pruning and what it cost.
    fn read(&self, file: &FileStats) -> Result<(Vec<RecordBatch>, OpenedFile), MetastoreError>;
}

/// The rows of one scanned partition: its key values and the batches of its files.
#[derive(Debug)]
pub struct PartitionRows {
    pub values: Vec<String>,
    pub batches: Vec<RecordBatch>,
}

/// A scan that returned rows: the table, the exact receipt and the rows per partition.
#[derive(Debug)]
pub struct RowScan {
    pub table: MetastoreTable,
    pub receipt: ScanReceipt,
    pub partitions: Vec<PartitionRows>,
}

impl MetastoreTableProvider<'_> {
    /// Validate, prune, then read: every refusal happens before the first data file opens.
    pub fn scan(&self, filter: &str) -> Result<ScanReceipt, MetastoreError> {
        let (_, receipt, _) = self.run(filter, |_, file| {
            self.files.open(file).map(|opened| (opened, ()))
        })?;
        Ok(receipt)
    }

    /// As [`Self::scan`], also returning the rows of every file the scan opened. `rows` reads
    /// the files; the provider's own `files` port still answers the schema checks.
    pub fn scan_rows(
        &self,
        filter: &str,
        rows: &dyn RowFileSource,
    ) -> Result<RowScan, MetastoreError> {
        let (table, receipt, partitions) = self.run(filter, |_, file| {
            rows.read(file).map(|(batches, opened)| (opened, batches))
        })?;
        Ok(RowScan {
            table,
            receipt,
            partitions: partitions
                .into_iter()
                .map(|(values, parts)| PartitionRows {
                    values,
                    batches: parts.into_iter().flatten().collect(),
                })
                .collect(),
        })
    }

    #[allow(clippy::type_complexity)]
    fn run<T>(
        &self,
        filter: &str,
        mut open: impl FnMut(&PartitionInfo, &FileStats) -> Result<(OpenedFile, T), MetastoreError>,
    ) -> Result<(MetastoreTable, ScanReceipt, Vec<(Vec<String>, Vec<T>)>), MetastoreError> {
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
        let mut read = Vec::with_capacity(matched.len());
        for partition in &matched {
            let mut parts = Vec::with_capacity(partition.files.len());
            for file in &partition.files {
                let (opened, value) = open(partition, file).map_err(|e| self.relabel(e))?;
                receipt.files_read += 1;
                receipt.stripes_read += opened.stripes_read;
                receipt.stripes_skipped += opened.stripes_total.saturating_sub(opened.stripes_read);
                receipt.bytes_read += opened.bytes_read;
                parts.push(value);
            }
            read.push((partition.values.clone(), parts));
        }
        Ok((table, receipt, read))
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

/// Avro schema resolution: a reader column the writer also has must carry the same type or a
/// lossless promotion. A reader column the writer lacks is an optional field added later and
/// reads as null (MEDULLA-UNIFIED-DATA-PLANE-R038.4.2.3).
pub fn resolve_schema(
    reader: &[MetastoreColumn],
    writer: &[MetastoreColumn],
) -> Result<(), MetastoreError> {
    for column in reader {
        let found = writer.iter().find(|w| w.name == column.name);
        if found.is_some_and(|w| !promotes(&w.data_type, &column.data_type)) {
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
