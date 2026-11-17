//! Hive-metastore table function (MEDULLA-UNIFIED-DATA-PLANE-R038.4.3) —
//! `hive('database.table')` or `hive('source', 'database.table')`, registered beside `iceberg`
//! so one SQL query can join a metastore table with the local graph.
//!
//! The endpoint never comes from SQL text. A call names a source (or none, when the caller
//! owns exactly one metastore source) and the caller's [`HiveSources`] resolves it: the served
//! path builds that resolver from the verified caller's own registry, so a source another
//! tenant registered resolves to nothing and cannot be named.
//!
//! A scan prunes partitions with the pushed partition-key equalities, prunes ORC stripes with
//! the pushed column comparisons, reads only the projected columns and returns the exact
//! [`ScanReceipt`] through the [`HiveScope`], which the served read puts into the query
//! warnings as `hive_scan_receipt`.

use std::sync::{Arc, Mutex};

use arrow::array::{new_null_array, ArrayRef, RecordBatch, RecordBatchOptions, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use datafusion::catalog::{Session, TableFunctionArgs, TableFunctionImpl, TableProvider};
use datafusion::datasource::MemTable;
use datafusion::error::{DataFusionError, Result as DfResult};
use datafusion::logical_expr::{BinaryExpr, Expr, Operator, TableProviderFilterPushDown};
use datafusion::scalar::ScalarValue;
use medulla_lake::metastore::avro::AvroFiles;
use medulla_lake::metastore::orc::{FileBytes, OrcFiles, StripeBound, StripeOp, StripePredicate};
use medulla_lake::metastore::scan::{
    partition_filter, DataFileSource, MetastoreTableProvider, PartitionPredicate, RowFileSource,
    ScanLimits, ScanReceipt, RECEIPT_WARNING_KEY,
};
use medulla_lake::metastore::{LakeFileFormat, MetastoreClient, MetastoreError, MetastoreTable};

use super::filter_shape::{classify_pushdown, ScanPlan};

/// One resolved metastore source: the catalog client, the data-file bytes port, the scan limits
/// (from the resolved `metastore.*` knobs) and the label every refusal names it by.
#[derive(Clone)]
pub struct HiveSource {
    pub label: String,
    pub client: Arc<dyn MetastoreClient + Send + Sync>,
    pub bytes: Arc<dyn FileBytes + Send + Sync>,
    pub limits: ScanLimits,
}

/// The caller-scoped source resolver. `None` names no source: it resolves when the caller owns
/// exactly one metastore source and refuses otherwise.
pub trait HiveSources: Send + Sync {
    fn resolve(&self, source: Option<&str>) -> Result<HiveSource, MetastoreError>;
}

/// What one query's `hive()` calls share: the resolver and the receipts the scans recorded.
pub struct HiveScope {
    sources: Arc<dyn HiveSources>,
    receipts: Mutex<Vec<ScanReceipt>>,
}

impl HiveScope {
    pub fn new(sources: Arc<dyn HiveSources>) -> Arc<Self> {
        Arc::new(Self {
            sources,
            receipts: Mutex::new(Vec::new()),
        })
    }

    fn record(&self, receipt: ScanReceipt) {
        if let Ok(mut receipts) = self.receipts.lock() {
            receipts.push(receipt);
        }
    }

    /// The `hive_scan_receipt=<json>` warning of each scan the query ran, in scan order.
    pub fn warnings(&self) -> Vec<String> {
        let receipts = self.receipts.lock().map(|r| r.clone()).unwrap_or_default();
        receipts
            .iter()
            .filter_map(|r| serde_json::to_string(r).ok())
            .map(|json| format!("{RECEIPT_WARNING_KEY}={json}"))
            .collect()
    }
}

/// The `hive(...)` table function.
#[derive(Default)]
pub struct MetastoreFunc {
    scope: Option<Arc<HiveScope>>,
}

impl std::fmt::Debug for MetastoreFunc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetastoreFunc")
            .field("bound", &self.scope.is_some())
            .finish()
    }
}

impl MetastoreFunc {
    pub fn new(scope: Option<Arc<HiveScope>>) -> Self {
        Self { scope }
    }
}

fn refusal(error: MetastoreError) -> DataFusionError {
    DataFusionError::Execution(error.to_string())
}

fn literal_str(e: &Expr, ctx: &str) -> DfResult<String> {
    match e {
        Expr::Literal(ScalarValue::Utf8(Some(s)), _) => Ok(s.clone()),
        _ => Err(refusal(MetastoreError::SpecInvalid(format!(
            "hive(...): {ctx} must be a string literal"
        )))),
    }
}

impl TableFunctionImpl for MetastoreFunc {
    fn call_with_args(&self, args: TableFunctionArgs<'_, '_>) -> DfResult<Arc<dyn TableProvider>> {
        let exprs = args.exprs();
        let (source, ident) = match exprs {
            [table] => (None, literal_str(table, "table identifier")?),
            [source, table] => (
                Some(literal_str(source, "source name")?),
                literal_str(table, "table identifier")?,
            ),
            _ => {
                return Err(refusal(MetastoreError::SpecInvalid(
                    "hive(...) expects ('database.table') or ('source', 'database.table')".into(),
                )))
            }
        };
        let (database, table) = ident.split_once('.').ok_or_else(|| {
            refusal(MetastoreError::SpecInvalid(format!(
                "hive(...): '{ident}' is not a `database.table` identifier"
            )))
        })?;
        let Some(scope) = &self.scope else {
            return Err(refusal(MetastoreError::SpecInvalid(
                "hive(...): no metastore source is bound to this query".into(),
            )));
        };
        let resolved = scope.sources.resolve(source.as_deref()).map_err(refusal)?;
        let description = resolved.client.table(database, table).map_err(refusal)?;
        let schema = Arc::new(table_schema(&description).map_err(refusal)?);
        Ok(Arc::new(HiveTableProvider {
            source: resolved,
            scope: Arc::clone(scope),
            database: database.to_string(),
            table: table.to_string(),
            description,
            schema,
        }))
    }
}

/// The Arrow type a Hive column type reads as; a type the scan cannot decode is refused.
fn hive_type(declared: &str) -> Result<DataType, MetastoreError> {
    Ok(match declared.trim().to_ascii_lowercase().as_str() {
        "boolean" => DataType::Boolean,
        "tinyint" => DataType::Int8,
        "smallint" => DataType::Int16,
        "int" | "integer" => DataType::Int32,
        "bigint" => DataType::Int64,
        "float" => DataType::Float32,
        "double" => DataType::Float64,
        "string" => DataType::Utf8,
        t if t.starts_with("varchar") || t.starts_with("char") => DataType::Utf8,
        "binary" => DataType::Binary,
        "date" => DataType::Date32,
        "timestamp" => DataType::Timestamp(TimeUnit::Nanosecond, None),
        other => {
            return Err(MetastoreError::SchemaIncompatible(format!(
                "column type {other} is not readable by hive()"
            )))
        }
    })
}

/// Data columns, then each partition key as a text column.
fn table_schema(table: &MetastoreTable) -> Result<Schema, MetastoreError> {
    let mut fields = Vec::new();
    for column in &table.columns {
        fields.push(Field::new(
            &column.name,
            hive_type(&column.data_type)?,
            true,
        ));
    }
    for key in &table.partition_keys {
        fields.push(Field::new(key, DataType::Utf8, true));
    }
    Ok(Schema::new(fields))
}

struct HiveTableProvider {
    source: HiveSource,
    scope: Arc<HiveScope>,
    database: String,
    table: String,
    description: MetastoreTable,
    schema: SchemaRef,
}

impl std::fmt::Debug for HiveTableProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HiveTableProvider")
            .field("source", &self.source.label)
            .field("table", &format!("{}.{}", self.database, self.table))
            .finish()
    }
}

/// One `column <op> literal` conjunct, with the column on the left.
fn comparison(expr: &Expr) -> Option<(String, Operator, &ScalarValue)> {
    let Expr::BinaryExpr(BinaryExpr { left, op, right }) = expr else {
        return None;
    };
    match (left.as_ref(), right.as_ref()) {
        (Expr::Column(c), Expr::Literal(v, _)) => Some((c.name.clone(), *op, v)),
        (Expr::Literal(v, _), Expr::Column(c)) => {
            let flipped = match op {
                Operator::Lt => Operator::Gt,
                Operator::LtEq => Operator::GtEq,
                Operator::Gt => Operator::Lt,
                Operator::GtEq => Operator::LtEq,
                other => *other,
            };
            Some((c.name.clone(), flipped, v))
        }
        _ => None,
    }
}

fn stripe_op(op: Operator) -> Option<StripeOp> {
    Some(match op {
        Operator::Eq => StripeOp::Eq,
        Operator::Lt => StripeOp::Lt,
        Operator::LtEq => StripeOp::Le,
        Operator::Gt => StripeOp::Gt,
        Operator::GtEq => StripeOp::Ge,
        _ => return None,
    })
}

fn stripe_bound(value: &ScalarValue) -> Option<StripeBound> {
    Some(match value {
        ScalarValue::Int8(Some(v)) => StripeBound::Int(i64::from(*v)),
        ScalarValue::Int16(Some(v)) => StripeBound::Int(i64::from(*v)),
        ScalarValue::Int32(Some(v)) => StripeBound::Int(i64::from(*v)),
        ScalarValue::Int64(Some(v)) => StripeBound::Int(*v),
        ScalarValue::Float32(Some(v)) => StripeBound::Double(f64::from(*v)),
        ScalarValue::Float64(Some(v)) => StripeBound::Double(*v),
        ScalarValue::Utf8(Some(v)) | ScalarValue::LargeUtf8(Some(v)) => {
            StripeBound::Text(v.clone())
        }
        _ => return None,
    })
}

impl HiveTableProvider {
    fn is_key(&self, name: &str) -> bool {
        self.description.partition_keys.iter().any(|k| k == name)
    }

    fn partition_predicate(&self, expr: &Expr) -> Option<PartitionPredicate> {
        let (key, op, value) = comparison(expr)?;
        match (op, value) {
            (Operator::Eq, ScalarValue::Utf8(Some(v))) if self.is_key(&key) => {
                Some(PartitionPredicate {
                    key,
                    value: v.clone(),
                })
            }
            _ => None,
        }
    }

    fn stripe_predicate(&self, expr: &Expr) -> Option<StripePredicate> {
        let (column, op, value) = comparison(expr)?;
        let is_data = self.description.columns.iter().any(|c| c.name == column);
        is_data.then_some(())?;
        Some(StripePredicate {
            column,
            op: stripe_op(op)?,
            value: stripe_bound(value)?,
        })
    }

    /// The projected batch of one file batch: each projected column cast to its declared type
    /// (a column the file lacks reads as null), partition keys as constant text columns.
    fn shape(
        &self,
        batch: &RecordBatch,
        values: &[String],
        projected: &SchemaRef,
    ) -> DfResult<RecordBatch> {
        let columns: Vec<ArrayRef> = projected
            .fields()
            .iter()
            .map(|field| {
                let key = self
                    .description
                    .partition_keys
                    .iter()
                    .position(|k| k == field.name());
                match key {
                    Some(at) => {
                        let value = values.get(at).cloned();
                        Ok(Arc::new(StringArray::from(vec![value; batch.num_rows()])) as ArrayRef)
                    }
                    None => match batch.column_by_name(field.name()) {
                        Some(column) => {
                            arrow::compute::cast(column, field.data_type()).map_err(Into::into)
                        }
                        None => Ok(new_null_array(field.data_type(), batch.num_rows())),
                    },
                }
            })
            .collect::<DfResult<_>>()?;
        let options = RecordBatchOptions::new().with_row_count(Some(batch.num_rows()));
        RecordBatch::try_new_with_options(Arc::clone(projected), columns, &options)
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl TableProvider for HiveTableProvider {
    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    fn table_type(&self) -> datafusion::logical_expr::TableType {
        datafusion::logical_expr::TableType::Base
    }

    /// Partition-key equalities and ORC column comparisons narrow the scan; DataFusion keeps
    /// the exact residual filter above it.
    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> DfResult<Vec<TableProviderFilterPushDown>> {
        Ok(classify_pushdown(filters, |candidate| {
            self.partition_predicate(candidate).is_some()
                || self.stripe_predicate(candidate).is_some()
        }))
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        pushed: &[Expr],
        limit: Option<usize>,
    ) -> ScanPlan {
        if !matches!(
            self.description.format,
            LakeFileFormat::Orc | LakeFileFormat::Avro
        ) {
            return Err(refusal(MetastoreError::UnsupportedLayout(format!(
                "{}.{} is stored as {:?}; hive() reads ORC and Avro tables",
                self.database, self.table, self.description.format
            ))));
        }
        let projected: SchemaRef = match projection {
            Some(indices) => Arc::new(self.schema.project(indices)?),
            None => Arc::clone(&self.schema),
        };
        let partition_predicates: Vec<PartitionPredicate> = pushed
            .iter()
            .filter_map(|e| self.partition_predicate(e))
            .collect();
        let columns: Vec<String> = projected
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .filter(|name| !self.is_key(name))
            .collect();
        // The scan borrows the source's ports, which are not `Sync`; finish it before the
        // await below so no borrow lives across it.
        let batches = {
            let orc = OrcFiles {
                bytes: self.source.bytes.as_ref(),
                predicates: pushed
                    .iter()
                    .filter_map(|e| self.stripe_predicate(e))
                    .collect(),
                projection: columns.clone(),
            };
            // MEDULLA-UNIFIED-DATA-PLANE-R038.4.2.3: an Avro table reads in the metastore's reader
            // schema; it has no stripe statistics, so only partition pruning skips files.
            let avro = AvroFiles {
                bytes: self.source.bytes.as_ref(),
                reader_schema: self.description.columns.clone(),
                projection: columns,
            };
            let (files, data): (&dyn RowFileSource, &dyn DataFileSource) =
                match self.description.format {
                    LakeFileFormat::Avro => (&avro, &avro),
                    _ => (&orc, &orc),
                };
            let provider = MetastoreTableProvider {
                client: self.source.client.as_ref(),
                files: data,
                source_label: self.source.label.clone(),
                database: self.database.clone(),
                table: self.table.clone(),
                limits: self.source.limits,
            };
            let scanned = provider
                .scan_rows(&partition_filter(&partition_predicates), files)
                .map_err(refusal)?;
            self.scope.record(scanned.receipt);
            let mut batches = Vec::new();
            for partition in &scanned.partitions {
                for batch in &partition.batches {
                    batches.push(self.shape(batch, &partition.values, &projected)?);
                }
            }
            batches
        };
        let mem = MemTable::try_new(projected, vec![batches])?;
        let read_limit = if pushed.is_empty() { limit } else { None };
        mem.scan(state, None, &[], read_limit).await
    }
}
