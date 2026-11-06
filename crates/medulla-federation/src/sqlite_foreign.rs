//! A live SQLite file as a foreign source (MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.3).
//!
//! `ForeignSourceSpec::SqliteFile { path, table, columns }` reads one table of a SQLite
//! file in place, through the lock-free WAL snapshot reader of `medulla-sqlite-format`
//! (R015.2.2.1): the file is opened read-only and never written, so a concurrent writer
//! never sees this reader. The row id is the SQLite rowid; the exposed columns come from
//! the spec (every declared column when it lists none). The registration gate
//! (`foreign.sqlite.allowed_roots`) lives where the spec is persisted; this module only
//! reads.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use medulla_exec::rowset::RowSet;
use medulla_sqlite_format::{Reader, Value};

use crate::federation::ForeignSource;
use crate::federation_opt::{ForeignRow, ForeignRows};

/// The `foreign.sqlite.busy_timeout_ms` default, used until the server installs its knob.
pub const DEFAULT_BUSY_TIMEOUT_MS: u64 = 1_000;

static BUSY_TIMEOUT_MS: AtomicU64 = AtomicU64::new(DEFAULT_BUSY_TIMEOUT_MS);

/// Install the startup-loaded `foreign.sqlite.busy_timeout_ms` knob. This crate is below
/// the server config, so the server hands the value down once at startup.
pub fn set_busy_timeout_ms(ms: u64) {
    BUSY_TIMEOUT_MS.store(ms, Ordering::Relaxed);
}

fn busy_timeout() -> Duration {
    Duration::from_millis(BUSY_TIMEOUT_MS.load(Ordering::Relaxed))
}

fn json_value(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Integer(i) => serde_json::Value::from(*i),
        Value::Real(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Text(s) => serde_json::Value::String(s.clone()),
        Value::Blob(b) => serde_json::Value::String(hex::encode(b)),
    }
}

/// Read `table` of the SQLite file at `path` as foreign rows keyed by rowid. `columns`
/// names the exposed columns (empty exposes every declared column); a name the table does
/// not declare is an error, never a silent absence.
pub fn fetch(path: &str, table: &str, columns: &[String]) -> Result<ForeignRows, String> {
    let main = PathBuf::from(path);
    let mut wal = main.clone().into_os_string();
    wal.push("-wal");
    let reader = Reader::open_with_wal(&main, &PathBuf::from(wal), busy_timeout())
        .map_err(|e| format!("federation: sqlite source '{table}': {e}"))?;
    let declared: Vec<String> = reader
        .table_columns(table)
        .map_err(|e| format!("federation: sqlite source '{table}': {e}"))?
        .into_iter()
        .map(|c| c.name)
        .collect();
    let exposed: Vec<&String> = if columns.is_empty() {
        declared.iter().collect()
    } else {
        columns.iter().collect()
    };
    let mut slots = Vec::with_capacity(exposed.len());
    for name in exposed {
        let slot = declared.iter().position(|d| d == name).ok_or_else(|| {
            format!("federation: sqlite source '{table}' declares no column '{name}'")
        })?;
        slots.push((name.clone(), slot));
    }
    let rows = reader
        .scan_table_with_rowids(table)
        .map_err(|e| format!("federation: sqlite source '{table}': {e}"))?;
    Ok(ForeignRows::from_rows(rows.into_iter().map(
        |(rowid, row)| {
            ForeignRow {
                id: rowid.to_string(),
                score: None,
                columns: slots
                    .iter()
                    .map(|(name, slot)| {
                        (
                            name.clone(),
                            row.get(*slot).map_or(serde_json::Value::Null, json_value),
                        )
                    })
                    .collect::<BTreeMap<_, _>>(),
            }
        },
    )))
}

/// The [`ForeignSource`] of a `SqliteFile` spec.
pub struct SqliteFileSource<'a> {
    /// Path of the database file.
    pub path: &'a str,
    /// The table to read.
    pub table: &'a str,
    /// The exposed columns.
    pub columns: &'a [String],
}

impl ForeignSource for SqliteFileSource<'_> {
    fn fetch(&self) -> Result<RowSet, String> {
        self.fetch_columns().map(ForeignRows::into_rowset)
    }

    fn fetch_columns(&self) -> Result<ForeignRows, String> {
        fetch(self.path, self.table, self.columns)
    }
}
