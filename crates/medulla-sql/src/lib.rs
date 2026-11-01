//! medulla-sql: SQL dialect: parse, classify, plan and execute over tables.
//!
//! Level L4 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: engine-parity (SQL), federated-query-and-reasoning (SQL).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(feature = "sql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sql;
#[cfg(feature = "sql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod provider;

#[cfg(feature = "sql")]
use crate::sql::ColumnDef;
#[cfg(feature = "sql")]
use medulla_tables::{schema::Column, schema::ColumnType};

#[cfg(feature = "sql")]
/// Resolve parsed SQL column declarations into the durable table-schema columns
/// consumed by every SQL transport. Keeping this conversion beside the two
/// types it joins prevents the embedded, native, and wire paths from drifting.
pub fn columns_from_defs(cols: &[ColumnDef]) -> Result<Vec<Column>, String> {
    cols.iter()
        .map(|column| {
            let ty = ColumnType::parse(&column.type_name)?;
            Ok(Column {
                name: column.name.clone(),
                ty,
                nullable: column.nullable,
                primary_key: column.primary_key,
                unique: column.unique,
                serial: column.serial,
                default: column.default.clone(),
                check: column.check.clone(),
            })
        })
        .collect()
}
