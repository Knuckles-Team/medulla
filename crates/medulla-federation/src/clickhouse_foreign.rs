//! A live ClickHouse table as a foreign source (MEDULLA-UNIFIED-DATA-PLANE-R017.2.2).
//!
//! `ForeignSourceSpec::ClickHouse` is read through a host-registered [`ClickHouseReader`]: this
//! crate sits below `medulla-plan`, which owns the HTTP client (`clickhouse_connection::query`),
//! so the host registers the reader once at startup, the same seam the warehouse reader uses.
//! The destination passes the one federation outbound gate before the reader is called, and a
//! column plan pushes its projection and ordered filters into the rendered statement.

use std::sync::{Arc, RwLock};

use medulla_exec::rowset::RowSet;

use crate::federation::ForeignSource;
use crate::federation_opt::{ColumnPlan, ColumnPredicate, ForeignRows};
use crate::warehouse_foreign::CAPABILITY_UNSUPPORTED;

/// The refusal reason of a capture request against ClickHouse.
pub const CLICKHOUSE_NO_CDC: &str = "clickhouse_no_cdc";

/// The registration refusal of a capture request: ClickHouse has no change capture.
pub fn capture_refusal() -> String {
    format!("{CAPABILITY_UNSUPPORTED}: {CLICKHOUSE_NO_CDC}: ClickHouse offers no change capture")
}

/// Vet the destination a ClickHouse source dials through the one federation outbound gate:
/// public HTTPS, or an exact allowlist entry.
pub fn check_outbound(host: &str, port: u16, tls: bool) -> Result<(), String> {
    let scheme = if tls { "https" } else { "http" };
    crate::federation_ssrf::validate_http_json_target(&format!("{scheme}://{host}:{port}/"))
        .map(|_| ())
}

/// One read of a ClickHouse table: what the column plan asked the source for.
#[derive(Debug, Clone)]
pub struct ClickHouseRead<'a> {
    /// The ClickHouse server host.
    pub host: &'a str,
    /// The ClickHouse HTTP port.
    pub port: u16,
    /// The database holding the table.
    pub database: &'a str,
    /// Whether the read uses HTTPS.
    pub tls: bool,
    /// The secret reference of the credential, never the secret.
    pub auth_ref: Option<&'a str>,
    /// The table to read.
    pub table: &'a str,
    /// The columns to return (includes the id column when named).
    pub columns: Vec<String>,
    /// The column carrying the row id; empty means the row ordinal.
    pub id_column: &'a str,
    /// Filters certified pushable by [`crate::warehouse_foreign::filter_support`]; the local
    /// residual still runs.
    pub filters: Vec<ColumnPredicate>,
}

/// The host's reader of ClickHouse tables.
pub trait ClickHouseReader: Send + Sync {
    /// Read the table. `Err` carries a typed code prefix (`SOURCE_UNAVAILABLE: ...`,
    /// `SCHEMA_DRIFT: ...`).
    fn read(&self, request: &ClickHouseRead<'_>) -> Result<ForeignRows, String>;
}

static READER: RwLock<Option<Arc<dyn ClickHouseReader>>> = RwLock::new(None);

/// Register the host's reader, replacing any earlier one.
pub fn register_clickhouse_reader(reader: Arc<dyn ClickHouseReader>) {
    if let Ok(mut slot) = READER.write() {
        *slot = Some(reader);
    }
}

fn registered_reader() -> Option<Arc<dyn ClickHouseReader>> {
    READER.read().ok().and_then(|slot| slot.clone())
}

/// The [`ForeignSource`] of a `ClickHouse` spec.
pub struct ClickHouseSource<'a> {
    /// The ClickHouse server host.
    pub host: &'a str,
    /// The ClickHouse HTTP port.
    pub port: u16,
    /// The database holding the table.
    pub database: &'a str,
    /// Whether reads use HTTPS.
    pub tls: bool,
    /// The secret reference of the credential, never the secret.
    pub auth_ref: Option<&'a str>,
    /// The table the spec names.
    pub table: &'a str,
    /// The columns the spec declares.
    pub columns: &'a [String],
    /// The column carrying the row id; empty means the row ordinal.
    pub id_column: &'a str,
}

impl ClickHouseSource<'_> {
    fn read(
        &self,
        columns: Vec<String>,
        filters: Vec<ColumnPredicate>,
    ) -> Result<ForeignRows, String> {
        check_outbound(self.host, self.port, self.tls)?;
        let reader = registered_reader().ok_or_else(|| {
            format!("{CAPABILITY_UNSUPPORTED}: no ClickHouse reader is installed in this build")
        })?;
        reader.read(&ClickHouseRead {
            host: self.host,
            port: self.port,
            database: self.database,
            tls: self.tls,
            auth_ref: self.auth_ref,
            table: self.table,
            columns,
            id_column: self.id_column,
            filters,
        })
    }

    fn with_id(&self, mut columns: Vec<String>) -> Vec<String> {
        if !self.id_column.is_empty() && !columns.iter().any(|c| c == self.id_column) {
            columns.push(self.id_column.to_string());
        }
        columns
    }
}

impl ForeignSource for ClickHouseSource<'_> {
    fn fetch(&self) -> Result<RowSet, String> {
        self.fetch_columns().map(ForeignRows::into_rowset)
    }

    fn fetch_columns(&self) -> Result<ForeignRows, String> {
        self.read(self.with_id(self.columns.to_vec()), Vec::new())
    }

    fn fetch_projected(&self, plan: &ColumnPlan) -> Result<ForeignRows, String> {
        self.read(
            self.with_id(plan.remote_projection.clone()),
            plan.remote_filters.clone(),
        )
    }
}
