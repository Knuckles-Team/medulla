//! Deferred foreign keys checked at the end of a transaction
//! (MEDULLA-ENGINE-PARITY-R011.1).
//!
//! A transaction records the keys it touched under a deferred foreign key: the
//! child keys it wrote and the parent keys it deleted or changed. After the last
//! op, before the commit point, only those keys are checked, so the cost of a
//! commit follows the keys the transaction touched and never the size of a
//! table. Each lookup goes through a secondary index on the key's first column
//! when one exists. The pending set is bounded by the registry knob
//! `tables.deferred_fk_pending_max` (a store without an installed value uses the
//! knob's registry default); a transaction that touches more keys is
//! refused `DEFERRED_FK_PENDING_EXCEEDED` and writes nothing.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use redb::ReadableTable;

use medulla_tables::schema::{Cell, TableConstraint, TableSchema};

use super::{
    decode_stored, get_schema_in, list_secondary_indexes_write, list_tables_in, map_err,
    row_exists_with_key_in, rowid_from_entry_key, secondary_entry_range, typed_cells_equal,
    validate_secondary_spec, SecondaryIndexLookup, SqlWrite, MAX_SECONDARY_INDEX_CANDIDATES, ROWS,
    SECONDARY_INDEX_ENTRIES,
};

/// Refusal when one transaction touches more deferred keys than the knob allows.
pub const DEFERRED_FK_PENDING_EXCEEDED: &str = "DEFERRED_FK_PENDING_EXCEEDED";

/// The store-wide settings and metric of the deferred-key pass. Clones of a
/// `TableStore` share one value.
#[derive(Debug, Default)]
pub(super) struct DeferredFkState {
    max_pending: OnceLock<usize>,
    checked: AtomicU64,
}

impl DeferredFkState {
    /// Install the pending bound (`tables.deferred_fk_pending_max`). Set once.
    pub(super) fn install_max_pending(&self, max: usize) {
        let _ = self.max_pending.set(max);
    }

    /// The bound in force: the value the composition root installed from the
    /// registry knob, else the knob's registry default, never zero.
    pub(super) fn max_pending(&self) -> usize {
        self.max_pending.get().copied().unwrap_or(
            usize::try_from(medulla_tables::DEFERRED_FK_PENDING_MAX_DEFAULT).unwrap_or(usize::MAX),
        )
    }

    /// Keys checked by every end-of-transaction pass since the store opened.
    pub(super) fn keys_checked(&self) -> u64 {
        self.checked.load(Ordering::Relaxed)
    }
}

/// One key a transaction touched under a deferred foreign key.
struct PendingKey {
    /// The table that declares the foreign key.
    child_table: String,
    /// Position of the constraint in the child table's schema.
    constraint: usize,
    key: Vec<Cell>,
}

/// The keys one transaction touched under deferred foreign keys.
#[derive(Default)]
pub(super) struct DeferredFkPending {
    /// Child keys written: each must resolve against the parent.
    children: Vec<PendingKey>,
    /// Parent keys deleted or changed: no child may still reference them.
    parents: Vec<PendingKey>,
    seen: HashSet<String>,
}

/// One deferred foreign key of a table, with its position in the schema.
struct DeferredKey<'a> {
    child_table: &'a str,
    position: usize,
    columns: &'a [String],
    ref_table: &'a str,
}

fn deferred_keys_of<'a>(schema: &'a TableSchema) -> Vec<DeferredKey<'a>> {
    schema
        .constraints()
        .iter()
        .enumerate()
        .filter_map(|(position, constraint)| match constraint {
            TableConstraint::ForeignKey {
                columns, ref_table, ..
            } if constraint.is_deferred_foreign_key() => Some(DeferredKey {
                child_table: schema.name.as_str(),
                position,
                columns,
                ref_table,
            }),
            _ => None,
        })
        .collect()
}

/// The values of `columns` in `cells`; `None` when any is NULL (MATCH SIMPLE).
fn key_of(schema: &TableSchema, columns: &[String], cells: &[Cell]) -> Option<Vec<Cell>> {
    let mut key = Vec::with_capacity(columns.len());
    for column in columns {
        let index = schema.column_index(column)?;
        match cells.get(index) {
            Some(Cell::Null) | None => return None,
            Some(cell) => key.push(cell.clone()),
        }
    }
    Some(key)
}

fn keys_equal(schema: &TableSchema, columns: &[String], left: &[Cell], right: &[Cell]) -> bool {
    columns
        .iter()
        .zip(left.iter().zip(right))
        .all(|(column, (a, b))| {
            schema
                .column_index(column)
                .is_some_and(|index| typed_cells_equal(a, b, schema.columns()[index].ty))
        })
}

/// Every deferred key in the catalog that references `table`.
fn referencing_deferred_keys(
    wtx: &SqlWrite<'_>,
    table: &str,
) -> Result<Vec<(TableSchema, usize)>, String> {
    let mut out = Vec::new();
    for name in list_tables_in(wtx)? {
        let Some(schema) = get_schema_in(wtx, &name)? else {
            continue;
        };
        for key in deferred_keys_of(&schema) {
            if key.ref_table == table {
                out.push((schema.clone(), key.position));
            }
        }
    }
    Ok(out)
}

impl DeferredFkPending {
    fn push(
        &mut self,
        state: &DeferredFkState,
        child: bool,
        child_table: &str,
        constraint: usize,
        key: Vec<Cell>,
    ) -> Result<(), String> {
        let signature = format!(
            "{}\u{0}{child_table}\u{0}{constraint}\u{0}{}",
            u8::from(child),
            serde_json::to_string(&key).map_err(|error| format!("encode key: {error}"))?
        );
        if !self.seen.insert(signature) {
            return Ok(());
        }
        let max = state.max_pending();
        if self.children.len().saturating_add(self.parents.len()) >= max {
            return Err(format!(
                "{DEFERRED_FK_PENDING_EXCEEDED}: the transaction touches more deferred foreign keys than tables.deferred_fk_pending_max ({max})"
            ));
        }
        let pending = PendingKey {
            child_table: child_table.to_string(),
            constraint,
            key,
        };
        if child {
            self.children.push(pending);
        } else {
            self.parents.push(pending);
        }
        Ok(())
    }

    fn record_child(
        &mut self,
        state: &DeferredFkState,
        schema: &TableSchema,
        old: Option<&[Cell]>,
        new: &[Cell],
    ) -> Result<(), String> {
        for key in deferred_keys_of(schema) {
            let Some(new_key) = key_of(schema, key.columns, new) else {
                continue;
            };
            if let Some(old_key) = old.and_then(|old| key_of(schema, key.columns, old)) {
                if keys_equal(schema, key.columns, &old_key, &new_key) {
                    continue;
                }
            }
            self.push(state, true, key.child_table, key.position, new_key)?;
        }
        Ok(())
    }

    fn record_parent(
        &mut self,
        state: &DeferredFkState,
        parent: &TableSchema,
        referencing: &[(TableSchema, usize)],
        old: &[Cell],
        new: Option<&[Cell]>,
    ) -> Result<(), String> {
        for (child, position) in referencing {
            let TableConstraint::ForeignKey { ref_columns, .. } = &child.constraints()[*position]
            else {
                continue;
            };
            let Some(old_key) = key_of(parent, ref_columns, old) else {
                continue;
            };
            if let Some(new_key) = new.and_then(|new| key_of(parent, ref_columns, new)) {
                if keys_equal(parent, ref_columns, &old_key, &new_key) {
                    continue;
                }
            }
            self.push(state, false, &child.name, *position, old_key)?;
        }
        Ok(())
    }

    /// Record the keys rows inserted into `table` touch.
    pub(super) fn record_insert(
        &mut self,
        state: &DeferredFkState,
        wtx: &SqlWrite<'_>,
        table: &str,
        inserted: &[Vec<Cell>],
    ) -> Result<(), String> {
        let Some(schema) = get_schema_in(wtx, table)? else {
            return Ok(());
        };
        if deferred_keys_of(&schema).is_empty() {
            return Ok(());
        }
        for cells in inserted {
            self.record_child(state, &schema, None, cells)?;
        }
        Ok(())
    }

    /// Record the keys updated rows of `table` touch, as `(old, new)` cells.
    pub(super) fn record_update(
        &mut self,
        state: &DeferredFkState,
        wtx: &SqlWrite<'_>,
        table: &str,
        changes: &[(Vec<Cell>, Vec<Cell>)],
    ) -> Result<(), String> {
        let Some(schema) = get_schema_in(wtx, table)? else {
            return Ok(());
        };
        let referencing = referencing_deferred_keys(wtx, table)?;
        if referencing.is_empty() && deferred_keys_of(&schema).is_empty() {
            return Ok(());
        }
        for (old, new) in changes {
            self.record_child(state, &schema, Some(old), new)?;
            self.record_parent(state, &schema, &referencing, old, Some(new))?;
        }
        Ok(())
    }

    /// Record the keys deleted rows of `table` touch.
    pub(super) fn record_delete(
        &mut self,
        state: &DeferredFkState,
        wtx: &SqlWrite<'_>,
        table: &str,
        removed: &[Vec<Cell>],
    ) -> Result<(), String> {
        let Some(schema) = get_schema_in(wtx, table)? else {
            return Ok(());
        };
        let referencing = referencing_deferred_keys(wtx, table)?;
        if referencing.is_empty() {
            return Ok(());
        }
        for old in removed {
            self.record_parent(state, &schema, &referencing, old, None)?;
        }
        Ok(())
    }

    /// The end-of-transaction pass: check every recorded key, once. A refusal
    /// aborts the transaction.
    pub(super) fn finish(
        &self,
        state: &DeferredFkState,
        wtx: &SqlWrite<'_>,
        tenant_scope: &str,
    ) -> Result<(), String> {
        for pending in &self.children {
            let Some((schema, columns, ref_table, ref_columns, name)) =
                resolve(wtx, &pending.child_table, pending.constraint)?
            else {
                continue;
            };
            state.checked.fetch_add(1, Ordering::Relaxed);
            let ref_schema = referenced_schema(wtx, &schema, &ref_table)?;
            if key_present(
                wtx,
                tenant_scope,
                &ref_table,
                &ref_schema,
                &ref_columns,
                &pending.key,
            )? {
                continue;
            }
            // The key does not resolve. A later op may have removed or moved the
            // child row that carried it; only a row still carrying it refuses.
            if key_present(
                wtx,
                tenant_scope,
                &schema.name,
                &schema,
                &columns,
                &pending.key,
            )? {
                return Err(format!(
                    "FOREIGN_KEY_VIOLATION: insert or update on table `{}` violates foreign key constraint `{name}`",
                    schema.name
                ));
            }
        }
        for pending in &self.parents {
            let Some((schema, columns, ref_table, ref_columns, name)) =
                resolve(wtx, &pending.child_table, pending.constraint)?
            else {
                continue;
            };
            state.checked.fetch_add(1, Ordering::Relaxed);
            let ref_schema = referenced_schema(wtx, &schema, &ref_table)?;
            if key_present(
                wtx,
                tenant_scope,
                &ref_table,
                &ref_schema,
                &ref_columns,
                &pending.key,
            )? {
                continue;
            }
            if key_present(
                wtx,
                tenant_scope,
                &schema.name,
                &schema,
                &columns,
                &pending.key,
            )? {
                return Err(format!(
                    "FOREIGN_KEY_VIOLATION: update or delete on table `{ref_table}` violates foreign key constraint `{name}` on table `{}`",
                    schema.name
                ));
            }
        }
        Ok(())
    }
}

type Resolved = (TableSchema, Vec<String>, String, Vec<String>, String);

/// The deferred key a pending entry names, as it stands now; `None` when the
/// transaction dropped the table or the constraint.
fn resolve(
    wtx: &SqlWrite<'_>,
    child_table: &str,
    position: usize,
) -> Result<Option<Resolved>, String> {
    let Some(schema) = get_schema_in(wtx, child_table)? else {
        return Ok(None);
    };
    let Some(constraint) = schema.constraints().get(position) else {
        return Ok(None);
    };
    let TableConstraint::ForeignKey {
        columns,
        ref_table,
        ref_columns,
        ..
    } = constraint
    else {
        return Ok(None);
    };
    if !constraint.is_deferred_foreign_key() {
        return Ok(None);
    }
    let name = TableSchema::constraint_display_name(child_table, constraint);
    Ok(Some((
        schema.clone(),
        columns.clone(),
        ref_table.clone(),
        ref_columns.clone(),
        name,
    )))
}

fn referenced_schema(
    wtx: &SqlWrite<'_>,
    child: &TableSchema,
    ref_table: &str,
) -> Result<TableSchema, String> {
    if ref_table == child.name {
        return Ok(child.clone());
    }
    get_schema_in(wtx, ref_table)?.ok_or_else(|| {
        format!(
            "FOREIGN KEY on table `{}`: referenced table `{ref_table}` does not exist",
            child.name
        )
    })
}

/// Does a row of `table` hold `key` in `columns`? One probe through a secondary
/// index on the first column when one exists, else one scan of the table.
fn key_present(
    wtx: &SqlWrite<'_>,
    tenant_scope: &str,
    table: &str,
    schema: &TableSchema,
    columns: &[String],
    key: &[Cell],
) -> Result<bool, String> {
    if let Some(found) = indexed_key_present(wtx, tenant_scope, table, schema, columns, key)? {
        return Ok(found);
    }
    row_exists_with_key_in(wtx, table, schema, columns, key)
}

/// The indexed probe; `None` when no usable index serves the first column or the
/// index returns more candidates than the engine's candidate bound.
fn indexed_key_present(
    wtx: &SqlWrite<'_>,
    tenant_scope: &str,
    table: &str,
    schema: &TableSchema,
    columns: &[String],
    key: &[Cell],
) -> Result<Option<bool>, String> {
    let (Some(first), Some(first_value)) = (columns.first(), key.first()) else {
        return Ok(None);
    };
    let Some(spec) = list_secondary_indexes_write(wtx, tenant_scope, table)?
        .into_iter()
        .find(|spec| {
            spec.serves_plain_lookup(first) && validate_secondary_spec(spec, schema).is_ok()
        })
    else {
        return Ok(None);
    };
    let lookup = SecondaryIndexLookup::Eq {
        column: first.clone(),
        value: first_value.clone(),
    };
    let Some((low, high)) = secondary_entry_range(&spec, schema, &lookup)? else {
        return Ok(None);
    };
    let mut rowids = Vec::new();
    {
        let entries = wtx.open_table(SECONDARY_INDEX_ENTRIES)?;
        for item in entries
            .range(low.as_str()..high.as_str())
            .map_err(map_err)?
        {
            let (entry, _) = item.map_err(map_err)?;
            let Some(rowid) = rowid_from_entry_key(entry.value()) else {
                return Ok(None);
            };
            if rowids.len() >= MAX_SECONDARY_INDEX_CANDIDATES {
                return Ok(None);
            }
            rowids.push(rowid);
        }
    }
    let width = schema.columns().len();
    let rows = wtx.open_table(ROWS)?;
    for rowid in rowids {
        let Some(value) = rows.get((table, rowid)).map_err(map_err)? else {
            // A directory entry without its row: the scan is the authority.
            return Ok(None);
        };
        let mut cells: Vec<Cell> = decode_stored(value.value(), "row")?;
        if cells.len() < width {
            cells.resize(width, Cell::Null);
        }
        let Some(row_key) = key_of(schema, columns, &cells) else {
            continue;
        };
        if keys_equal(schema, columns, &row_key, key) {
            return Ok(Some(true));
        }
    }
    Ok(Some(false))
}
