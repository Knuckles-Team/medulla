//! Warehouse and lake sources as foreign sources (MEDULLA-UNIFIED-DATA-PLANE-R020.2.2).
//!
//! `ForeignSourceSpec::Warehouse` is read through a host-registered [`WarehouseReader`]: this
//! crate sits below the SQL and plan crates that own the Iceberg scan and the DuckDB reader, so
//! the host registers the reader once at startup (the same seam the MCP connector port uses).
//! The reader decides the transport with `plan_pushdown` and answers a
//! [`WarehouseRead`] with the projected columns and the pushed filters of the column plan.

use std::sync::{Arc, RwLock};

use medulla_exec::rowset::RowSet;
use medulla_query_types::warehouse_federation::{WarehouseSourceConfig, WarehouseSourceKind};

use crate::federation::ForeignSource;
use crate::federation_opt::{
    ColumnPlan, ColumnPredicate, Comparison, ForeignRows, PushdownSupport,
};

/// Typed refusal: the kind or transport is not served by the engine.
pub const CAPABILITY_UNSUPPORTED: &str = "CAPABILITY_UNSUPPORTED";
/// Typed refusal: the source cannot be reached or read.
pub const SOURCE_UNAVAILABLE: &str = "SOURCE_UNAVAILABLE";
/// Typed refusal: the source schema no longer matches the registered columns.
pub const SCHEMA_DRIFT: &str = "SCHEMA_DRIFT";

/// Vet the destinations an Iceberg config dials (the REST catalog and its token endpoint)
/// through the one federation outbound gate: public HTTPS, or an exact allowlist entry.
/// Other kinds dial nothing from here (a DuckDB file is checked against its path allowlist).
pub fn check_outbound(config: &WarehouseSourceConfig) -> Result<(), String> {
    if config.kind != Some(WarehouseSourceKind::Iceberg) {
        return Ok(());
    }
    for field in ["catalog_uri", "oauth2_uri"] {
        if let Some(uri) = config.fields.get(field).filter(|uri| !uri.is_empty()) {
            crate::federation_ssrf::validate_http_json_target(uri)?;
        }
    }
    Ok(())
}

/// One read of a warehouse table: what the column plan asked the source for.
#[derive(Debug, Clone)]
pub struct WarehouseRead<'a> {
    /// The warehouse source's connection settings.
    pub config: &'a WarehouseSourceConfig,
    /// The table to read.
    pub table: &'a str,
    /// The columns to return (the remote projection; includes the id column when named).
    pub columns: Vec<String>,
    /// The column carrying the row id; empty means the row ordinal.
    pub id_column: &'a str,
    /// Prefer the Arrow Flight SQL transport when the source offers it.
    pub prefer_flight_sql: bool,
    /// The table snapshot to read; `None` reads the current snapshot.
    pub snapshot_id: Option<i64>,
    /// Filters certified pushable by [`filter_support`]; the local residual still runs.
    pub filters: Vec<ColumnPredicate>,
}

/// The host's reader of warehouse and lake tables.
pub trait WarehouseReader: Send + Sync {
    /// Read the table. `Err` carries a typed code prefix (`CAPABILITY_UNSUPPORTED: ...`,
    /// `SOURCE_UNAVAILABLE: ...`, `SCHEMA_DRIFT: ...`).
    fn read(&self, request: &WarehouseRead<'_>) -> Result<ForeignRows, String>;
}

static READER: RwLock<Option<Arc<dyn WarehouseReader>>> = RwLock::new(None);

/// Register the host's reader, replacing any earlier one.
pub fn register_warehouse_reader(reader: Arc<dyn WarehouseReader>) {
    if let Ok(mut slot) = READER.write() {
        *slot = Some(reader);
    }
}

fn registered_reader() -> Option<Arc<dyn WarehouseReader>> {
    READER.read().ok().and_then(|slot| slot.clone())
}

/// Which column predicates a warehouse scan can evaluate: a comparison of a column with a
/// string or a number. The local residual still decides exact membership.
pub fn filter_support(predicate: &ColumnPredicate) -> PushdownSupport {
    let comparable = predicate.value.is_string() || predicate.value.is_number();
    let ordered = matches!(
        predicate.comparison,
        Comparison::Eq
            | Comparison::Ne
            | Comparison::Lt
            | Comparison::Le
            | Comparison::Gt
            | Comparison::Ge
    );
    if comparable && ordered {
        PushdownSupport::Inexact
    } else {
        PushdownSupport::Unsupported
    }
}

/// The [`ForeignSource`] of a `Warehouse` spec.
pub struct WarehouseSource<'a> {
    /// The warehouse source's connection settings.
    pub config: &'a WarehouseSourceConfig,
    /// The table to read.
    pub table: &'a str,
    /// The columns to return.
    pub columns: &'a [String],
    /// The column carrying the row id; empty means the row ordinal.
    pub id_column: &'a str,
    /// Prefer the Arrow Flight SQL transport when the source offers it.
    pub prefer_flight_sql: bool,
    /// The table snapshot to read; `None` reads the current snapshot.
    pub snapshot_id: Option<i64>,
}

impl WarehouseSource<'_> {
    fn read(
        &self,
        columns: Vec<String>,
        filters: Vec<ColumnPredicate>,
    ) -> Result<ForeignRows, String> {
        check_outbound(self.config)?;
        let reader = registered_reader().ok_or_else(|| {
            format!("{CAPABILITY_UNSUPPORTED}: no warehouse reader is installed in this build")
        })?;
        reader.read(&WarehouseRead {
            config: self.config,
            table: self.table,
            columns,
            id_column: self.id_column,
            prefer_flight_sql: self.prefer_flight_sql,
            snapshot_id: self.snapshot_id,
            filters,
        })
    }
}

impl ForeignSource for WarehouseSource<'_> {
    fn fetch(&self) -> Result<RowSet, String> {
        self.fetch_columns().map(ForeignRows::into_rowset)
    }

    fn fetch_columns(&self) -> Result<ForeignRows, String> {
        let mut columns: Vec<String> = self.columns.to_vec();
        if !self.id_column.is_empty() && !columns.iter().any(|c| c == self.id_column) {
            columns.push(self.id_column.to_string());
        }
        self.read(columns, Vec::new())
    }

    fn fetch_projected(&self, plan: &ColumnPlan) -> Result<ForeignRows, String> {
        let mut columns = plan.remote_projection.clone();
        if !self.id_column.is_empty() && !columns.iter().any(|c| c == self.id_column) {
            columns.push(self.id_column.to_string());
        }
        self.read(columns, plan.remote_filters.clone())
    }
}
