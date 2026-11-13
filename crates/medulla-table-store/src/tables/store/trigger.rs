//! Row-event triggers (MEDULLA-ENGINE-PARITY-R012.1).
//!
//! A trigger is a statechart stored in its table's schema record. Every write
//! path (`insert_in`, `update_rows_in`, `delete_in`) calls the functions below
//! inside the write transaction, so a refusal aborts the whole statement.
//!
//! The machine sees the row as flat context keys `new.<column>` and
//! `old.<column>` (typed JSON, as `row_map` renders them) and receives the event
//! `before_insert`, `after_update`, and so on, in the chart's initial state. A
//! `Before` machine rewrites the row by assigning `new.<column>` keys; keys it
//! removes are dropped from an insert and left unchanged by an update. An `After`
//! machine may only emit effects: its assignments are ignored. An `Emit` action
//! whose signal is `refuse` or `refuse:<message>` aborts the statement with
//! `TRIGGER_REFUSED`.

use medulla_statechart::transition::transition;
use medulla_statechart::{Action, Context, EventInput};
use medulla_tables::schema::{Cell, TableSchema};
use medulla_tables::trigger::{
    TriggerEvent, TriggerSpec, TriggerTiming, TRIGGER_EXISTS, TRIGGER_MACHINE_INVALID,
    TRIGGER_REFUSED, TRIGGER_UNKNOWN,
};
use serde_json::{Map, Value};

use super::{
    get_schema_in, insert_plain_in, map_err, resolve_update_assignments, row_map, SqlWrite,
    TableStore, CATALOG, MAX_SQL_STORED_VALUE_BYTES,
};

fn refusal_message(signal: &str) -> Option<String> {
    if signal == "refuse" {
        return Some("refused".to_string());
    }
    signal
        .strip_prefix("refuse:")
        .map(|message| message.trim().to_string())
}

/// Run every trigger of `schema` that matches `timing` and `event`, in creation
/// order. Returns the (possibly rewritten) `new` map; the rewrite is only
/// honoured for `Before`.
fn run_triggers(
    schema: &TableSchema,
    timing: TriggerTiming,
    event: TriggerEvent,
    old: Option<&Map<String, Value>>,
    new: Option<Map<String, Value>>,
) -> Result<Option<Map<String, Value>>, String> {
    let mut current = new;
    for trigger in schema
        .triggers()
        .iter()
        .filter(|t| t.timing == timing && t.reacts_to(event))
    {
        let mut context = Context::new();
        if let Some(old) = old {
            for (column, value) in old {
                context.set(format!("old.{column}"), value.clone());
            }
        }
        if let Some(new) = &current {
            for (column, value) in new {
                context.set(format!("new.{column}"), value.clone());
            }
        }
        let outcome = transition(
            &trigger.machine,
            &trigger.machine.initial,
            &context,
            &EventInput::new(trigger.event_name(event)),
        )
        .map_err(|error| {
            format!(
                "{TRIGGER_MACHINE_INVALID}: trigger `{}` on `{}`: {error}",
                trigger.name, schema.name
            )
        })?;
        for action in &outcome.actions {
            if let Action::Emit { signal } = action {
                if let Some(message) = refusal_message(signal) {
                    return Err(format!(
                        "{TRIGGER_REFUSED}: trigger `{}` on `{}`: {message}",
                        trigger.name, schema.name
                    ));
                }
            }
        }
        if timing == TriggerTiming::Before {
            if let Some(current) = current.as_mut() {
                let mut next = Map::new();
                for (key, value) in &outcome.next_context.0 {
                    if let Some(column) = key.strip_prefix("new.") {
                        next.insert(column.to_string(), value.clone());
                    }
                }
                *current = next;
            }
        }
    }
    Ok(current)
}

fn has_trigger(schema: &TableSchema, timing: TriggerTiming, event: TriggerEvent) -> bool {
    schema
        .triggers()
        .iter()
        .any(|t| t.timing == timing && t.reacts_to(event))
}

/// `INSERT`: before triggers rewrite or refuse each row, the row is written, then
/// after triggers see the stored row.
pub(super) fn insert_triggered_in(
    wtx: &SqlWrite<'_>,
    tenant_scope: &str,
    table: &str,
    col_order: &[String],
    rows: &[Vec<Value>],
    schema: &TableSchema,
) -> Result<Vec<Vec<Cell>>, String> {
    let mut inserted: Vec<Vec<Cell>> = Vec::with_capacity(rows.len());
    for row in rows {
        let supplied: Map<String, Value> =
            col_order.iter().cloned().zip(row.iter().cloned()).collect();
        let rewritten = run_triggers(
            schema,
            TriggerTiming::Before,
            TriggerEvent::Insert,
            None,
            Some(supplied),
        )?
        .unwrap_or_default();
        // The statement's own column order first, then columns a trigger added.
        let mut order: Vec<String> = col_order
            .iter()
            .filter(|column| rewritten.contains_key(column.as_str()))
            .cloned()
            .collect();
        let mut added: Vec<&String> = rewritten
            .keys()
            .filter(|column| !col_order.contains(column))
            .collect();
        added.sort();
        order.extend(added.into_iter().cloned());
        let values: Vec<Value> = order
            .iter()
            .map(|column| rewritten[column].clone())
            .collect();
        let cells = insert_plain_in(wtx, tenant_scope, table, &order, &[values])?;
        for stored in &cells {
            run_triggers(
                schema,
                TriggerTiming::After,
                TriggerEvent::Insert,
                None,
                Some(row_map(schema, stored)),
            )?;
        }
        inserted.extend(cells);
    }
    Ok(inserted)
}

/// Whether an insert into `schema` has any trigger to run.
pub(super) fn insert_has_triggers(schema: &TableSchema) -> bool {
    has_trigger(schema, TriggerTiming::Before, TriggerEvent::Insert)
        || has_trigger(schema, TriggerTiming::After, TriggerEvent::Insert)
}

/// Before-update: `cells` already carry the statement's `SET`; the triggers may
/// rewrite or refuse the row.
pub(super) fn before_update(
    schema: &TableSchema,
    old_cells: &[Cell],
    cells: &mut [Cell],
) -> Result<(), String> {
    if !has_trigger(schema, TriggerTiming::Before, TriggerEvent::Update) {
        return Ok(());
    }
    let old = row_map(schema, old_cells);
    let new = row_map(schema, cells);
    let next = run_triggers(
        schema,
        TriggerTiming::Before,
        TriggerEvent::Update,
        Some(&old),
        Some(new.clone()),
    )?
    .unwrap_or_default();
    let changes: Map<String, Value> = next
        .into_iter()
        .filter(|(column, value)| new.get(column) != Some(value))
        .collect();
    if changes.is_empty() {
        return Ok(());
    }
    for (index, cell) in resolve_update_assignments(schema, &schema.name, &changes)? {
        cells[index] = cell;
    }
    Ok(())
}

/// After-update: effects only.
pub(super) fn after_update(
    schema: &TableSchema,
    old_cells: &[Cell],
    cells: &[Cell],
) -> Result<(), String> {
    if !has_trigger(schema, TriggerTiming::After, TriggerEvent::Update) {
        return Ok(());
    }
    run_triggers(
        schema,
        TriggerTiming::After,
        TriggerEvent::Update,
        Some(&row_map(schema, old_cells)),
        Some(row_map(schema, cells)),
    )?;
    Ok(())
}

/// Before-delete: a trigger may refuse; there is no new row to rewrite.
pub(super) fn before_delete(schema: &TableSchema, old_cells: &[Cell]) -> Result<(), String> {
    delete_event(schema, TriggerTiming::Before, old_cells)
}

/// After-delete: effects only.
pub(super) fn after_delete(schema: &TableSchema, old_cells: &[Cell]) -> Result<(), String> {
    delete_event(schema, TriggerTiming::After, old_cells)
}

fn delete_event(
    schema: &TableSchema,
    timing: TriggerTiming,
    old_cells: &[Cell],
) -> Result<(), String> {
    if !has_trigger(schema, timing, TriggerEvent::Delete) {
        return Ok(());
    }
    run_triggers(
        schema,
        timing,
        TriggerEvent::Delete,
        Some(&row_map(schema, old_cells)),
        None,
    )?;
    Ok(())
}

/// Write `schema` back to the catalog without touching its indexes: a trigger
/// change does not alter the table's shape.
fn put_triggers_in(wtx: &SqlWrite<'_>, schema: &TableSchema) -> Result<(), String> {
    schema.validate()?;
    let blob = rmp_serde::to_vec_named(schema).map_err(|e| format!("encode schema: {e}"))?;
    if blob.len() > MAX_SQL_STORED_VALUE_BYTES {
        return Err("encoded SQL schema exceeds storage value limit".to_string());
    }
    let mut catalog = wtx.open_table(CATALOG)?;
    catalog
        .insert(schema.name.as_str(), blob.as_slice())
        .map_err(map_err)?;
    Ok(())
}

impl TableStore {
    /// Attach a trigger to its table (stored in the table's schema record).
    pub fn create_trigger(&self, spec: &TriggerSpec) -> Result<(), String> {
        spec.validate()?;
        self.authority
            .maintain("create-trigger", &spec.table, |wtx| {
                let mut schema = get_schema_in(wtx, &spec.table)?
                    .ok_or_else(|| format!("table `{}` does not exist", spec.table))?;
                if schema.triggers().iter().any(|t| t.name == spec.name) {
                    return Err(format!(
                        "{TRIGGER_EXISTS}: trigger `{}` already exists on `{}`",
                        spec.name, spec.table
                    ));
                }
                schema.triggers_mut().push(spec.clone());
                put_triggers_in(wtx, &schema)
            })
    }

    /// Remove a trigger from its table.
    pub fn drop_trigger(&self, table: &str, name: &str) -> Result<(), String> {
        self.authority.maintain("drop-trigger", table, |wtx| {
            let mut schema = get_schema_in(wtx, table)?
                .ok_or_else(|| format!("table `{table}` does not exist"))?;
            let before = schema.triggers().len();
            schema.triggers_mut().retain(|t| t.name != name);
            if schema.triggers().len() == before {
                return Err(format!(
                    "{TRIGGER_UNKNOWN}: trigger `{name}` does not exist on `{table}`"
                ));
            }
            put_triggers_in(wtx, &schema)
        })
    }
}
