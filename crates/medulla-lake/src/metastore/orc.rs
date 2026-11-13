//! ORC stripe pruning (MEDULLA-UNIFIED-DATA-PLANE-R038.4.2.2): a pushed predicate is checked
//! against each stripe's column statistics from the file footer, and only the stripes that can
//! hold a matching row are decoded. The counters it returns are exact: they count what the
//! scan opened, so the scan receipt (`.4.5`) is read straight from them.

use arrow::array::RecordBatch;
use bytes::Bytes;
use orc_rust::arrow_reader::ArrowReaderBuilder;
use orc_rust::projection::ProjectionMask;
use orc_rust::reader::metadata::FileMetadata;
use orc_rust::statistics::{ColumnStatistics, TypeStatistics};
use orc_rust::stripe::StripeMetadata;

use super::scan::{DataFileSource, OpenedFile, RowFileSource};
use super::{FileStats, MetastoreColumn, MetastoreError};

/// The comparison of one pushed predicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripeOp {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
}

/// The literal of one pushed predicate.
#[derive(Clone, Debug, PartialEq)]
pub enum StripeBound {
    Int(i64),
    Double(f64),
    Text(String),
}

/// One predicate `column <op> value` pushed to the ORC scan.
#[derive(Clone, Debug, PartialEq)]
pub struct StripePredicate {
    pub column: String,
    pub op: StripeOp,
    pub value: StripeBound,
}

/// The rows of the stripes a scan kept, and what opening the file cost.
#[derive(Debug)]
pub struct OrcScan {
    pub batches: Vec<RecordBatch>,
    pub opened: OpenedFile,
}

fn unreadable(detail: impl core::fmt::Display) -> MetastoreError {
    MetastoreError::UnsupportedLayout(format!("orc file unreadable: {detail}"))
}

/// Whether `value op bound` can hold for some row of a stripe whose values lie in
/// `[low, high]`.
fn range_may_match<T: PartialOrd>(op: StripeOp, low: &T, high: &T, value: &T) -> bool {
    match op {
        StripeOp::Eq => low <= value && value <= high,
        StripeOp::Lt => low < value,
        StripeOp::Le => low <= value,
        StripeOp::Gt => high > value,
        StripeOp::Ge => high >= value,
    }
}

/// Whether one stripe's statistics for the predicate column leave room for a matching row.
/// Statistics that are absent or of another kind never prune: a stripe is skipped only when
/// its statistics prove no row can match.
fn statistics_may_match(stats: &ColumnStatistics, predicate: &StripePredicate) -> bool {
    if stats.number_of_values() == 0 {
        // Every value of the column is null in this stripe; a comparison never holds on null.
        return false;
    }
    match (stats.type_statistics(), &predicate.value) {
        (Some(TypeStatistics::Integer { min, max, .. }), StripeBound::Int(v)) => {
            range_may_match(predicate.op, min, max, v)
        }
        (Some(TypeStatistics::Integer { min, max, .. }), StripeBound::Double(v)) => {
            range_may_match(predicate.op, &(*min as f64), &(*max as f64), v)
        }
        (Some(TypeStatistics::Double { min, max, .. }), StripeBound::Double(v)) => {
            range_may_match(predicate.op, min, max, v)
        }
        (Some(TypeStatistics::Double { min, max, .. }), StripeBound::Int(v)) => {
            range_may_match(predicate.op, min, max, &(*v as f64))
        }
        (
            Some(TypeStatistics::String {
                lower_bound,
                upper_bound,
                ..
            }),
            StripeBound::Text(v),
        ) => range_may_match(predicate.op, lower_bound, upper_bound, v),
        _ => true,
    }
}

/// Chooses the stripes of one ORC file that can hold a row matching every predicate.
pub struct OrcStripePruner<'a> {
    pub predicates: &'a [StripePredicate],
}

impl OrcStripePruner<'_> {
    /// The indexes of the stripes whose statistics may satisfy the predicates. A stripe without
    /// statistics for a column is always kept, and a predicate on a column the file lacks
    /// keeps every stripe.
    pub fn select(&self, metadata: &FileMetadata) -> Vec<usize> {
        let columns: Vec<(&str, usize)> = metadata
            .root_data_type()
            .children()
            .iter()
            .map(|c| (c.name(), c.data_type().column_index()))
            .collect();
        let may_match = |stripe: &StripeMetadata| {
            self.predicates.iter().all(|predicate| {
                let index = columns
                    .iter()
                    .find(|(name, _)| *name == predicate.column)
                    .map(|(_, index)| *index);
                match index.and_then(|i| stripe.column_statistics().get(i)) {
                    Some(stats) => statistics_may_match(stats, predicate),
                    None => true,
                }
            })
        };
        metadata
            .stripe_metadatas()
            .iter()
            .enumerate()
            .filter(|(_, stripe)| may_match(stripe))
            .map(|(index, _)| index)
            .collect()
    }
}

/// Decode the stripes of one ORC file whose statistics can match every predicate, reading only
/// the `projection` columns (all columns when it is empty).
pub fn scan_orc(
    file: Bytes,
    predicates: &[StripePredicate],
    projection: &[String],
) -> Result<OrcScan, MetastoreError> {
    let probe = ArrowReaderBuilder::try_new(file.clone()).map_err(unreadable)?;
    let metadata = probe.file_metadata();
    let selected = OrcStripePruner { predicates }.select(metadata);
    let mask = (!projection.is_empty())
        .then(|| ProjectionMask::named_roots(metadata.root_data_type(), projection));
    let mut opened = OpenedFile {
        stripes_total: metadata.stripe_metadatas().len() as u64,
        ..OpenedFile::default()
    };
    let mut batches = Vec::new();
    for index in selected {
        let stripe = &metadata.stripe_metadatas()[index];
        opened.stripes_read += 1;
        opened.bytes_read += stripe.index_length() + stripe.data_length() + stripe.footer_length();
        let offset = stripe.offset() as usize;
        let mut builder = ArrowReaderBuilder::try_new(file.clone())
            .map_err(unreadable)?
            .with_file_byte_range(offset..offset + 1);
        if let Some(mask) = &mask {
            builder = builder.with_projection(mask.clone());
        }
        for batch in builder.build() {
            batches.push(batch.map_err(unreadable)?);
        }
    }
    Ok(OrcScan { batches, opened })
}

/// The column names and types an ORC file declares in its footer.
pub fn orc_schema(file: Bytes) -> Result<Vec<MetastoreColumn>, MetastoreError> {
    let reader = ArrowReaderBuilder::try_new(file).map_err(unreadable)?;
    Ok(reader
        .schema()
        .fields()
        .iter()
        .map(|f| MetastoreColumn {
            name: f.name().clone(),
            data_type: f.data_type().to_string().to_lowercase(),
        })
        .collect())
}

/// The byte port of data files: the served path reads through the object store, tests and
/// local warehouses read the file system.
pub trait FileBytes {
    fn read(&self, path: &str) -> Result<Bytes, MetastoreError>;
}

/// Reads data files from the local file system.
pub struct LocalFileBytes {
    pub source_label: String,
}

impl FileBytes for LocalFileBytes {
    fn read(&self, path: &str) -> Result<Bytes, MetastoreError> {
        std::fs::read(path)
            .map(Bytes::from)
            .map_err(|_| MetastoreError::SourceUnreachable {
                source_label: self.source_label.clone(),
            })
    }
}

/// ORC data files with pushed stripe predicates: the `DataFileSource` of an ORC table.
pub struct OrcFiles<'a> {
    pub bytes: &'a dyn FileBytes,
    pub predicates: Vec<StripePredicate>,
    /// Columns to decode; empty means all.
    pub projection: Vec<String>,
}

impl OrcFiles<'_> {
    /// Read one file, keeping the rows of the stripes that survive pruning.
    pub fn read(&self, file: &FileStats) -> Result<OrcScan, MetastoreError> {
        scan_orc(
            self.bytes.read(&file.path)?,
            &self.predicates,
            &self.projection,
        )
    }
}

impl DataFileSource for OrcFiles<'_> {
    fn writer_schema(&self, file: &FileStats) -> Result<Vec<MetastoreColumn>, MetastoreError> {
        orc_schema(self.bytes.read(&file.path)?)
    }

    fn open(&self, file: &FileStats) -> Result<OpenedFile, MetastoreError> {
        self.read(file).map(|scan| scan.opened)
    }
}

impl RowFileSource for OrcFiles<'_> {
    fn read(&self, file: &FileStats) -> Result<(Vec<RecordBatch>, OpenedFile), MetastoreError> {
        OrcFiles::read(self, file).map(|scan| (scan.batches, scan.opened))
    }
}
