//! The host's reader of warehouse and lake sources (MEDULLA-UNIFIED-DATA-PLANE-R020.2.2).
//!
//! `medulla-federation` sits below this crate, so a `ForeignSourceSpec::Warehouse` read
//! reaches the engines through the [`WarehouseReader`] port registered here at startup. The
//! chain is: `Warehouse` spec → `WarehouseSource` → this reader → [`plan_pushdown`] → the
//! kind's engine: the Iceberg REST-catalog scan of `medulla-sql` (manifest pruning by the
//! pushed filters and the projection) or the read-only DuckDB file reader. `snowflake` and
//! `bigquery` are served by connectors and refuse `CAPABILITY_UNSUPPORTED`.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use medulla_federation::federation_opt::{ColumnPredicate, Comparison, ForeignRow, ForeignRows};
use medulla_federation::warehouse_foreign::{
    register_warehouse_reader, WarehouseRead, WarehouseReader, CAPABILITY_UNSUPPORTED,
    SCHEMA_DRIFT, SOURCE_UNAVAILABLE,
};
use medulla_sql::sql::{
    read_iceberg_table, IcebergCompare, IcebergFilter, IcebergReadError, IcebergReadRequest,
};
use medulla_query_types::warehouse_federation::WarehouseSourceKind;

use crate::warehouse_pushdown::{
    plan_pushdown, WarehousePushdownRefusal, WarehousePushdownRequest,
};

/// Typed refusal: a DuckDB file outside `foreign.duckdb.allowed_roots`.
pub const DUCKDB_FILE_DENIED: &str = "DUCKDB_FILE_DENIED";

static DUCKDB_ROOTS: RwLock<Vec<PathBuf>> = RwLock::new(Vec::new());

/// Install the startup-loaded `foreign.duckdb.allowed_roots`. This crate is below the server
/// config, so the server hands the value down once at startup.
pub fn set_duckdb_allowed_roots(roots: Vec<PathBuf>) {
    if let Ok(mut slot) = DUCKDB_ROOTS.write() {
        *slot = roots;
    }
}

/// The canonical path of `path` when it lies under an allowed root.
#[cfg_attr(not(feature = "source-duckdb"), allow(dead_code))]
fn path_under_allowed_roots(path: &str) -> Result<PathBuf, String> {
    let denied = |why: &str| format!("{DUCKDB_FILE_DENIED}: {why}");
    let canonical = std::fs::canonicalize(path)
        .map_err(|_| denied("duckdb file path is not an existing file"))?;
    let roots = DUCKDB_ROOTS
        .read()
        .map_err(|_| denied("the allowed roots are unavailable"))?;
    if roots
        .iter()
        .any(|root| std::fs::canonicalize(root).is_ok_and(|root| canonical.starts_with(root)))
    {
        Ok(canonical)
    } else {
        Err(denied(
            "duckdb file is outside foreign.duckdb.allowed_roots",
        ))
    }
}

fn compare(comparison: Comparison) -> IcebergCompare {
    match comparison {
        Comparison::Eq => IcebergCompare::Eq,
        Comparison::Ne => IcebergCompare::Ne,
        Comparison::Lt => IcebergCompare::Lt,
        Comparison::Le => IcebergCompare::Le,
        Comparison::Gt => IcebergCompare::Gt,
        Comparison::Ge => IcebergCompare::Ge,
    }
}

fn read_iceberg(request: &WarehouseRead<'_>) -> Result<ForeignRows, String> {
    let filters = request
        .filters
        .iter()
        .map(
            |ColumnPredicate {
                 column,
                 comparison,
                 value,
             }| IcebergFilter {
                column: column.clone(),
                compare: compare(*comparison),
                value: value.clone(),
            },
        )
        .collect();
    let read = read_iceberg_table(&IcebergReadRequest {
        fields: &request.config.fields,
        table: request.table,
        snapshot_id: request.snapshot_id,
        columns: request.columns.clone(),
        filters,
    })
    .map_err(|error| match error {
        IcebergReadError::Unavailable(message) => format!("{SOURCE_UNAVAILABLE}: {message}"),
        IcebergReadError::SchemaDrift(message) => format!("{SCHEMA_DRIFT}: {message}"),
    })?;
    Ok(ForeignRows::from_rows(
        read.rows
            .into_iter()
            .enumerate()
            .map(|(ordinal, columns)| ForeignRow {
                id: row_id(&columns, request.id_column, ordinal),
                score: None,
                columns,
            }),
    ))
}

fn row_id(
    columns: &std::collections::BTreeMap<String, serde_json::Value>,
    id_column: &str,
    ordinal: usize,
) -> String {
    match columns.get(id_column) {
        Some(serde_json::Value::String(s)) if !id_column.is_empty() => s.clone(),
        Some(serde_json::Value::Null) | None => ordinal.to_string(),
        Some(other) => other.to_string(),
    }
}

#[cfg(feature = "source-duckdb")]
fn read_duckdb(request: &WarehouseRead<'_>) -> Result<ForeignRows, String> {
    use crate::duckdb_foreign::{fetch, DuckdbReadError};
    let path = request
        .config
        .fields
        .get("database_path")
        .map_or("", String::as_str);
    let canonical = path_under_allowed_roots(path)?;
    fetch(
        &canonical,
        request.table,
        &request.columns,
        request.id_column,
        &request.filters,
    )
    .map_err(|error| match error {
        DuckdbReadError::Unavailable(message) => format!("{SOURCE_UNAVAILABLE}: {message}"),
        DuckdbReadError::SchemaDrift(message) => format!("{SCHEMA_DRIFT}: {message}"),
    })
}

#[cfg(not(feature = "source-duckdb"))]
fn read_duckdb(_request: &WarehouseRead<'_>) -> Result<ForeignRows, String> {
    Err(format!(
        "{CAPABILITY_UNSUPPORTED}: this build does not include the DuckDB reader (source-duckdb)"
    ))
}

/// The engine-backed [`WarehouseReader`].
#[derive(Debug, Default, Clone, Copy)]
pub struct EngineWarehouseReader;

impl WarehouseReader for EngineWarehouseReader {
    fn read(&self, request: &WarehouseRead<'_>) -> Result<ForeignRows, String> {
        plan_pushdown(&WarehousePushdownRequest {
            config: request.config.clone(),
            prefer_flight_sql: request.prefer_flight_sql,
        })
        .map_err(|refusal| match refusal {
            WarehousePushdownRefusal::InvalidConfig(missing) => {
                format!("INVALID_ARGUMENT: warehouse config invalid: {missing}")
            }
            WarehousePushdownRefusal::FlightSqlNotOffered(kind) => {
                format!("{CAPABILITY_UNSUPPORTED}: {kind:?} does not offer Arrow Flight SQL")
            }
        })?;
        match request.config.kind {
            Some(WarehouseSourceKind::Iceberg) => read_iceberg(request),
            Some(WarehouseSourceKind::DuckDb) => read_duckdb(request),
            Some(kind @ (WarehouseSourceKind::Snowflake | WarehouseSourceKind::BigQuery)) => {
                Err(format!(
                    "{CAPABILITY_UNSUPPORTED}: connector_source: {kind:?} is served by a connector"
                ))
            }
            None => Err("INVALID_ARGUMENT: warehouse config invalid: no kind".to_string()),
        }
    }
}

/// Install the engine reader as the host's warehouse reader.
pub fn register() {
    register_warehouse_reader(Arc::new(EngineWarehouseReader));
}
