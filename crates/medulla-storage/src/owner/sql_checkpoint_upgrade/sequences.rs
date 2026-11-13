//! Named-sequence seeding of the offline SQL layout upgrade
//! (MEDULLA-ENGINE-PARITY-R013.2).
//!
//! Before named sequences a `SERIAL` column took `rowid + 1` from the table's
//! rowid counter in `__sql_seq__`, and a named sequence lived in the same table
//! under reserved NUL-prefixed keys. The successor layout keeps the rowid
//! counter as the physical row key only; every serial column owns the sequence
//! `<table>_<column>_seq` in `__sql_sequences__`. This step runs inside the
//! upgrade's one write transaction: it moves the existing named sequences,
//! then creates a sequence for every serial column of the catalog under every
//! tenant scope the file has served, with its high mark seeded from the old
//! counter so the next insert continues above the old high mark.

use super::super::registry::{
    sequence_row_key, serial_sequence_name, SERIAL_SEQUENCE_CACHE, SERIAL_SEQUENCE_INCREMENT,
    SERIAL_SEQUENCE_START, SQL_BOOTSTRAP_RESOURCE, SQL_CATALOG, SQL_SEQ, SQL_SEQUENCES,
};
use crate::physical::binding::decode_binding;
use crate::tables::SCOPE_BINDINGS;
use medulla_mutation_types::mutation_scope::MutationScope;
use redb::{ReadableTable, WriteTransaction};
use serde::Deserialize;
use std::collections::BTreeSet;

/// Prefix of every named-sequence key a predecessor kept beside the rowid
/// counters.
const LEGACY_SEQUENCE_PREFIX: &str = "\0seq\0";

#[derive(Deserialize)]
struct CatalogEntry {
    name: String,
    columns: Vec<CatalogColumn>,
}

#[derive(Deserialize)]
struct CatalogColumn {
    name: String,
    serial: bool,
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Move the predecessor's named sequences and create the serial sequences.
pub(super) fn seed_named_sequences(write: &WriteTransaction) -> Result<(), String> {
    move_legacy_named_sequences(write)?;
    create_serial_sequences(write)
}

fn move_legacy_named_sequences(write: &WriteTransaction) -> Result<(), String> {
    let mut old = write.open_table(SQL_SEQ).map_err(err)?;
    let mut moved = Vec::new();
    for entry in old.range::<&str>(LEGACY_SEQUENCE_PREFIX..).map_err(err)? {
        let (key, value) = entry.map_err(err)?;
        if !key.value().starts_with(LEGACY_SEQUENCE_PREFIX) {
            break;
        }
        moved.push((key.value().to_string(), value.value()));
    }
    let mut sequences = write.open_table(SQL_SEQUENCES).map_err(err)?;
    for (key, value) in moved {
        sequences.insert(key.as_str(), value).map_err(err)?;
        old.remove(key.as_str()).map_err(err)?;
    }
    Ok(())
}

/// The tenant scopes whose SQL bootstrap scope the file has bound: the scopes
/// a sequence key can name.
fn served_scopes(write: &WriteTransaction) -> Result<BTreeSet<String>, String> {
    let bindings = write.open_table(SCOPE_BINDINGS).map_err(err)?;
    let mut scopes = BTreeSet::new();
    for entry in bindings.iter().map_err(err)? {
        let (_, bytes) = entry.map_err(err)?;
        let binding = decode_binding(bytes.value())?;
        let identity = &binding.identity;
        if let MutationScope::Native { resource, .. } = identity.scope() {
            if resource.as_str() == SQL_BOOTSTRAP_RESOURCE {
                scopes.insert(identity.tenant().as_str().to_string());
            }
        }
    }
    Ok(scopes)
}

fn create_serial_sequences(write: &WriteTransaction) -> Result<(), String> {
    let serial_columns = serial_columns(write)?;
    if serial_columns.is_empty() {
        return Ok(());
    }
    let scopes = served_scopes(write)?;
    if scopes.is_empty() {
        return Err(
            "SQL layout upgrade found serial columns but no tenant scope that served them"
                .to_string(),
        );
    }
    let counters = write.open_table(SQL_SEQ).map_err(err)?;
    let mut sequences = write.open_table(SQL_SEQUENCES).map_err(err)?;
    for (table, column) in &serial_columns {
        // The old serial value was `rowid + 1` and the counter is the next
        // rowid, so the first value never issued is `counter + 1`.
        let counter = counters
            .get(table.as_str())
            .map_err(err)?
            .map(|value| value.value())
            .unwrap_or(0);
        let high_mark = counter
            .checked_add(SERIAL_SEQUENCE_START)
            .ok_or_else(|| format!("serial counter of `{table}` is exhausted"))?;
        let name = serial_sequence_name(table, column);
        for scope in &scopes {
            for (field, value) in [
                ("start", SERIAL_SEQUENCE_START),
                ("increment", SERIAL_SEQUENCE_INCREMENT),
                ("cache", SERIAL_SEQUENCE_CACHE),
                ("high_mark", high_mark),
            ] {
                sequences
                    .insert(sequence_row_key(scope, &name, field).as_str(), value)
                    .map_err(err)?;
            }
        }
    }
    drop(counters);
    Ok(())
}

/// Every `(table, column)` of the catalog whose column is `SERIAL`.
fn serial_columns(write: &WriteTransaction) -> Result<Vec<(String, String)>, String> {
    let catalog = write.open_table(SQL_CATALOG).map_err(err)?;
    let mut found = Vec::new();
    for entry in catalog.iter().map_err(err)? {
        let (_, bytes) = entry.map_err(err)?;
        let table: CatalogEntry = rmp_serde::from_slice(bytes.value())
            .map_err(|error| format!("SQL catalog entry is unreadable: {error}"))?;
        for column in table.columns.iter().filter(|column| column.serial) {
            found.push((table.name.clone(), column.name.clone()));
        }
    }
    Ok(found)
}
