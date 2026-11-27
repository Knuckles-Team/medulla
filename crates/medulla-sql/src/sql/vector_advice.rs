//! `information_schema.vector_advice` (MEDULLA-FEDERATED-QUERY-R077.6.3): ranked,
//! read-only advice on which text columns would benefit from an embedding
//! binding, computed from the text-search usage the tenant's reads recorded.
//! Synthesized per statement like `medulla_index_status`, over the reading
//! store, which holds only the tables the caller may `SELECT`. Advice never
//! creates a binding and queues no embedding.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use datafusion::datasource::MemTable;
use datafusion::logical_expr::LogicalPlan;
use datafusion::prelude::SessionContext;
use medulla_table_store::tables::embedding_binding::{
    is_embeddable, AdviceEligibility, ColumnVectorAdvice,
};
use medulla_table_store::tables::{
    admit_pinned, catalog_schema_revision, rank_columns, AdviceColumn, AdviceInput, TableStore,
};
use medulla_tables::schema::{Cell, TableSchema};

use super::filter_shape::text_search_columns;

/// The name of the relation, `information_schema.vector_advice`.
pub(crate) const VECTOR_ADVICE_RELATION: &str = "vector_advice";

/// Whether `sql` may read the advice relation. Advice moves with traffic, not
/// with a catalog write, so such a statement never uses a cached context.
/// Over-approximates on purpose: a false positive only skips the cache.
pub(crate) fn reads_vector_advice(sql: &str) -> bool {
    sql.to_ascii_lowercase().contains(VECTOR_ADVICE_RELATION)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn table_schemas(store: &TableStore) -> Result<Vec<TableSchema>, String> {
    let mut tables = Vec::new();
    for name in store.list_tables()? {
        if let Some(schema) = store.get_schema(&name)? {
            tables.push(schema);
        }
    }
    Ok(tables)
}

/// Record the text-search uses of a planned statement into the counters of the
/// store it reads. A recording failure never fails the read.
pub(crate) fn record_text_search_usage(store: &TableStore, plan: &LogicalPlan) {
    let pairs = text_search_columns(plan);
    if pairs.is_empty() {
        return;
    }
    let recorded = (|| -> Result<(), String> {
        let tables = table_schemas(store)?;
        let user: HashSet<&str> = tables.iter().map(|schema| schema.name.as_str()).collect();
        let pairs: Vec<(String, String)> = pairs
            .into_iter()
            .filter(|(table, _)| user.contains(table.as_str()))
            .collect();
        if pairs.is_empty() {
            return Ok(());
        }
        let revision = catalog_schema_revision(&tables)?;
        store.column_usage().observe(&pairs, revision, now_ms());
        Ok(())
    })();
    if let Err(reason) = recorded {
        tracing::debug!(%reason, "column usage was not recorded");
    }
}

/// Average byte length and distinct ratio of one column over a sample.
fn column_stats(cells: impl Iterator<Item = Option<String>>) -> (f64, f64) {
    let mut count = 0_u64;
    let mut bytes = 0_u64;
    let mut distinct: HashSet<String> = HashSet::new();
    for value in cells.flatten() {
        count += 1;
        bytes += value.len() as u64;
        distinct.insert(value);
    }
    if count == 0 {
        return (0.0, 0.0);
    }
    (
        bytes as f64 / count as f64,
        distinct.len() as f64 / count as f64,
    )
}

fn cell_text(cell: &Cell) -> Option<String> {
    match cell {
        Cell::Text(text) => Some(text.clone()),
        Cell::Json(value) => Some(value.to_string()),
        _ => None,
    }
}

/// The `(table, column)` a statement pins with `table_name = '..' AND
/// column_name = '..'` on the advice relation; `None` for a listing.
pub(crate) fn pinned_column(sql: &str) -> Option<(String, String)> {
    let lowered = sql.to_ascii_lowercase();
    if lowered.contains(" or ") {
        return None;
    }
    Some((
        literal_after(sql, &lowered, "table_name")?,
        literal_after(sql, &lowered, "column_name")?,
    ))
}

/// The single-quoted literal in `key = '<literal>'`, matched on word boundaries.
fn literal_after(sql: &str, lowered: &str, key: &str) -> Option<String> {
    let mut from = 0;
    while let Some(found) = lowered[from..].find(key) {
        let start = from + found;
        let end = start + key.len();
        from = end;
        let boundary_before = lowered[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        let rest = sql[end..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('\'') else {
            continue;
        };
        if !boundary_before {
            continue;
        }
        let mut literal = String::new();
        let mut chars = rest.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                    literal.push('\'');
                    continue;
                }
                return Some(literal);
            }
            literal.push(c);
        }
        return None;
    }
    None
}

/// The ranked advice over every column of every table `store` holds. A pinned
/// read (`pin`) names one column and is admitted for that column alone
/// (MEDULLA-FEDERATED-QUERY-R077.6.4).
pub(crate) fn read_advice(
    store: &TableStore,
    pin: Option<&(String, String)>,
) -> Result<Vec<ColumnVectorAdvice>, String> {
    let now = now_ms();
    let counters = store.column_usage();
    let settings = counters.settings();
    let tables = table_schemas(store)?;
    let revision = catalog_schema_revision(&tables)?;
    let usage: HashMap<(String, String), _> = counters
        .read(now)
        .into_iter()
        .map(|entry| ((entry.table.clone(), entry.column.clone()), entry))
        .collect();
    let mut columns = Vec::new();
    for schema in &tables {
        let sample = store.row_snapshot(&schema.name, None, None)?;
        for (index, column) in schema.columns().iter().enumerate() {
            let (avg_length, distinct_ratio) = if is_embeddable(column.ty) {
                column_stats(
                    sample
                        .rows
                        .iter()
                        .take(settings.stats_sample_rows)
                        .map(|row| row.cells.get(index).and_then(cell_text)),
                )
            } else {
                (0.0, 0.0)
            };
            let observed = usage.get(&(schema.name.clone(), column.name.clone()));
            let advice_column = AdviceColumn {
                table: schema.name.clone(),
                column: column.name.clone(),
                ty: column.ty,
                observed_schema_revision: observed
                    .map_or(revision, |entry| entry.observed_schema_revision),
                text_search_uses: observed.map_or(0, |entry| entry.text_search_uses),
                avg_length,
                distinct_ratio,
            };
            if pin.is_some_and(|(table, name)| *table == schema.name && *name == column.name) {
                admit_pinned(&advice_column, revision).map_err(|error| {
                    format!("{}: column `{}.{}`", error.code(), schema.name, column.name)
                })?;
            }
            columns.push(advice_column);
        }
    }
    let input = AdviceInput {
        schema_revision: revision,
        window_start: now.saturating_sub(settings.window_ms),
        window_end: now,
        columns,
    };
    rank_columns(&input, &settings.policy).map_err(|error| error.code().to_string())
}

/// `(column, type, nullable)`, in relation order.
const COLUMNS: [(&str, DataType, bool); 14] = [
    ("table_name", DataType::Utf8, false),
    ("column_name", DataType::Utf8, false),
    ("schema_revision", DataType::Int64, false),
    ("window_start", DataType::Int64, false),
    ("window_end", DataType::Int64, false),
    ("sample_count", DataType::Int64, false),
    ("eligibility", DataType::Utf8, false),
    ("reason", DataType::Utf8, false),
    ("score", DataType::Float64, false),
    ("total_text_search_uses", DataType::Int64, false),
    ("policy_revision", DataType::Utf8, false),
    ("demand_contribution", DataType::Float64, false),
    ("length_contribution", DataType::Float64, false),
    ("distinct_contribution", DataType::Float64, false),
];

fn schema() -> SchemaRef {
    Arc::new(Schema::new(
        COLUMNS
            .iter()
            .map(|(name, kind, nullable)| Field::new(*name, kind.clone(), *nullable))
            .collect::<Vec<_>>(),
    ))
}

fn saturating(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn eligibility_name(eligibility: AdviceEligibility) -> &'static str {
    match eligibility {
        AdviceEligibility::Eligible => "eligible",
        AdviceEligibility::UnsupportedType => "unsupported_type",
        AdviceEligibility::InsufficientObservations => "insufficient_observations",
    }
}

/// Register `rows` as `information_schema.vector_advice` in `ctx`, whose
/// `information_schema` must already be registered.
pub(crate) fn register(ctx: &SessionContext, rows: &[ColumnVectorAdvice]) -> Result<(), String> {
    let information_schema = ctx
        .catalog("datafusion")
        .and_then(|catalog| catalog.schema("information_schema"))
        .ok_or_else(|| "information_schema is not registered".to_string())?;
    let text = |pick: fn(&ColumnVectorAdvice) -> String| -> ArrayRef {
        Arc::new(StringArray::from(rows.iter().map(pick).collect::<Vec<_>>()))
    };
    let number = |pick: fn(&ColumnVectorAdvice) -> i64| -> ArrayRef {
        Arc::new(Int64Array::from(rows.iter().map(pick).collect::<Vec<_>>()))
    };
    let float = |pick: fn(&ColumnVectorAdvice) -> f64| -> ArrayRef {
        Arc::new(Float64Array::from(
            rows.iter().map(pick).collect::<Vec<_>>(),
        ))
    };
    let columns: Vec<ArrayRef> = vec![
        text(|row| row.table.clone()),
        text(|row| row.column.clone()),
        number(|row| saturating(row.schema_revision)),
        number(|row| row.window_start),
        number(|row| row.window_end),
        number(|row| saturating(row.sample_count)),
        text(|row| eligibility_name(row.eligibility).to_string()),
        text(|row| row.reason.clone()),
        float(|row| row.score),
        number(|row| saturating(row.receipt.total_text_search_uses)),
        text(|row| row.receipt.policy_revision.clone()),
        float(|row| row.receipt.demand),
        float(|row| row.receipt.length),
        float(|row| row.receipt.distinct),
    ];
    let batch = RecordBatch::try_new(schema(), columns)
        .map_err(|error| format!("information_schema.{VECTOR_ADVICE_RELATION} batch: {error}"))?;
    let table = MemTable::try_new(batch.schema(), vec![vec![batch]])
        .map_err(|error| format!("{VECTOR_ADVICE_RELATION} memtable: {error}"))?;
    information_schema
        .register_table(VECTOR_ADVICE_RELATION.to_string(), Arc::new(table))
        .map_err(|error| format!("register {VECTOR_ADVICE_RELATION}: {error}"))?;
    Ok(())
}
