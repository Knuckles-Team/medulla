//! A DuckDB database file as a read-only foreign source (MEDULLA-UNIFIED-DATA-PLANE-R020.2.2).
//!
//! The file is opened read-only through the `duckdb` crate (bundled engine), so a source read
//! never writes it. The caller (`warehouse_reader`) has already checked the path against
//! `foreign.duckdb.allowed_roots`. Identifiers are quoted and every filter value is bound as a
//! parameter, never spliced into the statement.

use std::collections::BTreeMap;
use std::path::Path;

use duckdb::types::Value as DuckValue;
use duckdb::{AccessMode, Config, Connection};
use medulla_federation::federation_opt::{ColumnPredicate, Comparison, ForeignRow, ForeignRows};

/// A typed failure of one DuckDB read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DuckdbReadError {
    /// The file or table could not be opened or read.
    Unavailable(String),
    /// A requested column is absent from the table.
    SchemaDrift(String),
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn unavailable(error: impl std::fmt::Display) -> DuckdbReadError {
    DuckdbReadError::Unavailable(error.to_string())
}

/// Split `schema.table` (or a bare `table`) into its schema and table names.
fn split_table(table: &str) -> (Option<&str>, &str) {
    match table.rsplit_once('.') {
        Some((schema, name)) => (Some(schema), name),
        None => (None, table),
    }
}

/// The types read as themselves; any other column type is read as its text rendering.
fn is_scalar(data_type: &str) -> bool {
    matches!(
        data_type,
        "BOOLEAN"
            | "TINYINT"
            | "SMALLINT"
            | "INTEGER"
            | "BIGINT"
            | "UTINYINT"
            | "USMALLINT"
            | "UINTEGER"
            | "UBIGINT"
            | "FLOAT"
            | "DOUBLE"
            | "VARCHAR"
    )
}

fn is_numeric(data_type: &str) -> bool {
    is_scalar(data_type) && data_type != "VARCHAR" && data_type != "BOOLEAN"
}

fn operator(comparison: Comparison) -> &'static str {
    match comparison {
        Comparison::Eq => "=",
        Comparison::Ne => "<>",
        Comparison::Lt => "<",
        Comparison::Le => "<=",
        Comparison::Gt => ">",
        Comparison::Ge => ">=",
    }
}

fn json_value(value: DuckValue) -> Result<serde_json::Value, DuckdbReadError> {
    Ok(match value {
        DuckValue::Null => serde_json::Value::Null,
        DuckValue::Boolean(v) => serde_json::Value::from(v),
        DuckValue::TinyInt(v) => serde_json::Value::from(v),
        DuckValue::SmallInt(v) => serde_json::Value::from(v),
        DuckValue::Int(v) => serde_json::Value::from(v),
        DuckValue::BigInt(v) => serde_json::Value::from(v),
        DuckValue::UTinyInt(v) => serde_json::Value::from(v),
        DuckValue::USmallInt(v) => serde_json::Value::from(v),
        DuckValue::UInt(v) => serde_json::Value::from(v),
        DuckValue::UBigInt(v) => serde_json::Value::from(v),
        DuckValue::Float(v) => serde_json::Number::from_f64(f64::from(v))
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        DuckValue::Double(v) => serde_json::Number::from_f64(v)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        DuckValue::Text(v) => serde_json::Value::String(v),
        other => {
            return Err(DuckdbReadError::Unavailable(format!(
                "duckdb returned a value of an unsupported type: {other:?}"
            )))
        }
    })
}

fn row_id(value: &serde_json::Value, ordinal: usize) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => ordinal.to_string(),
        other => other.to_string(),
    }
}

/// Read `columns` of `table` in the DuckDB file at `path`, keyed by the `id_column` value
/// (the row ordinal when `id_column` is empty). `filters` are pushed into the statement for
/// columns whose type holds the value; the caller's residual filter still decides rows.
pub fn fetch(
    path: &Path,
    table: &str,
    columns: &[String],
    id_column: &str,
    filters: &[ColumnPredicate],
) -> Result<ForeignRows, DuckdbReadError> {
    let config = Config::default()
        .access_mode(AccessMode::ReadOnly)
        .map_err(unavailable)?;
    let conn = Connection::open_with_flags(path, config).map_err(unavailable)?;
    let (schema, name) = split_table(table);

    // The table's declared columns and types.
    let mut declared: BTreeMap<String, String> = BTreeMap::new();
    {
        let (sql, args): (&str, Vec<&str>) = match schema {
            Some(schema) => (
                "SELECT column_name, data_type FROM information_schema.columns \
                 WHERE table_name = ? AND table_schema = ?",
                vec![name, schema],
            ),
            None => (
                "SELECT column_name, data_type FROM information_schema.columns \
                 WHERE table_name = ?",
                vec![name],
            ),
        };
        let mut stmt = conn.prepare(sql).map_err(unavailable)?;
        let rows = stmt
            .query_map(duckdb::params_from_iter(args), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(unavailable)?;
        for row in rows {
            let (column, data_type) = row.map_err(unavailable)?;
            declared.insert(column, data_type);
        }
    }
    if declared.is_empty() {
        return Err(DuckdbReadError::Unavailable(format!(
            "duckdb table '{table}' does not exist"
        )));
    }
    for column in columns {
        if !declared.contains_key(column) {
            return Err(DuckdbReadError::SchemaDrift(format!(
                "duckdb table '{table}' has no column '{column}'"
            )));
        }
    }

    let select = columns
        .iter()
        .map(|column| {
            if declared.get(column).is_some_and(|t| is_scalar(t)) {
                quote(column)
            } else {
                format!("CAST({} AS VARCHAR) AS {}", quote(column), quote(column))
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let from = match schema {
        Some(schema) => format!("{}.{}", quote(schema), quote(name)),
        None => quote(name),
    };
    let mut sql = format!("SELECT {select} FROM {from}");
    let mut params: Vec<DuckValue> = Vec::new();
    let mut clauses = Vec::new();
    for filter in filters {
        let Some(data_type) = declared.get(&filter.column) else {
            continue;
        };
        let value = match &filter.value {
            serde_json::Value::String(s) if data_type == "VARCHAR" => DuckValue::Text(s.clone()),
            serde_json::Value::Number(n) if is_numeric(data_type) => match n.as_i64() {
                Some(i) => DuckValue::BigInt(i),
                None => DuckValue::Double(n.as_f64().unwrap_or(f64::NAN)),
            },
            _ => continue,
        };
        clauses.push(format!(
            "{} {} ?",
            quote(&filter.column),
            operator(filter.comparison)
        ));
        params.push(value);
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }

    let mut stmt = conn.prepare(&sql).map_err(unavailable)?;
    let mut result = stmt
        .query(duckdb::params_from_iter(params.iter()))
        .map_err(unavailable)?;
    let mut rows = Vec::new();
    while let Some(row) = result.next().map_err(unavailable)? {
        let mut values = BTreeMap::new();
        for (index, column) in columns.iter().enumerate() {
            let value: DuckValue = row.get(index).map_err(unavailable)?;
            values.insert(column.clone(), json_value(value)?);
        }
        let id = if id_column.is_empty() {
            rows.len().to_string()
        } else {
            row_id(
                values.get(id_column).unwrap_or(&serde_json::Value::Null),
                rows.len(),
            )
        };
        rows.push(ForeignRow {
            id,
            score: None,
            columns: values,
        });
    }
    Ok(ForeignRows::from_rows(rows))
}
