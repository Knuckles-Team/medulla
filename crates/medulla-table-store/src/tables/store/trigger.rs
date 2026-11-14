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
//!
//! Nesting (MEDULLA-ENGINE-PARITY-R012.2): an `After` machine may also run
//! `Custom` effects that write to a table, `insert_row`, `update_rows` and
//! `delete_rows`, whose `args` name the `table` and carry the row (`row`), the
//! assignments (`set`) and an equality selector (`where`); a value of the form
//! `{"from": "<context key>"}` reads the machine's context (`new.<column>`,
//! `old.<column>`). The write goes through the same write path, so the table's
//! own triggers react in turn, one level deeper. A chain deeper than the
//! registry knob `tables.trigger_depth_max` refuses `TRIGGER_DEPTH`. Every
//! reaction appends a `TriggerReaction` to the append-only ledger in the same
//! transaction, and a trigger reacts at most once per event of one write chain
//! (`TRIGGER_REACTION_DUPLICATE`). Any refusal aborts the whole statement.

use redb::ReadableTable;
use std::sync::atomic::{AtomicU64, Ordering};

use medulla_result_types::row_predicate::{CmpOp, RowPredicate};
use medulla_statechart::transition::transition;
use medulla_statechart::{Action, Context, EventInput};
use medulla_storage::{trigger_reaction_key, trigger_statement_counter_key, SQL_TRIGGER_REACTIONS};
use medulla_tables::schema::{Cell, TableSchema};
use medulla_tables::trigger::{
    TriggerEvent, TriggerReaction, TriggerSpec, TriggerTiming, TRIGGER_DEPTH, TRIGGER_EXISTS,
    TRIGGER_MACHINE_INVALID, TRIGGER_REACTION_DUPLICATE, TRIGGER_REFUSED, TRIGGER_UNKNOWN,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::{
    delete_in_at, get_schema_in, insert_in_at, insert_plain_in, map_err,
    resolve_update_assignments, row_map, update_rows_in_at, SqlWrite, TableStore, CATALOG,
    MAX_SQL_STORED_VALUE_BYTES,
};

/// The resolved registry knob `tables.trigger_depth_max`. The store cannot see
/// the registry, so the composition root installs the resolved value; until
/// then the registry default applies.
static DEPTH_MAX: AtomicU64 = AtomicU64::new(medulla_tables::TRIGGER_DEPTH_MAX_DEFAULT);

/// Install the resolved `tables.trigger_depth_max` (MEDULLA-ENGINE-PARITY-R012.2).
pub fn install_trigger_depth_max(max: u64) {
    DEPTH_MAX.store(max, Ordering::Relaxed);
}

/// Where a write sits in a chain of nested trigger writes.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Chain {
    /// Trigger levels above this write; 0 for a client statement.
    depth: u64,
    /// The write chain's id in the reaction ledger, allocated at the first
    /// reaction of a top-level row event.
    statement: Option<u64>,
}

impl Chain {
    /// A client statement's write.
    pub(super) fn top() -> Self {
        Self::default()
    }
}

fn next_statement(wtx: &SqlWrite<'_>, scope: &str) -> Result<u64, String> {
    let key = trigger_statement_counter_key(scope);
    let mut ledger = wtx.open_table(SQL_TRIGGER_REACTIONS)?;
    let current: u64 = {
        let stored = ledger.get(key.as_str()).map_err(map_err)?;
        match stored {
            Some(value) => rmp_serde::from_slice(value.value())
                .map_err(|e| format!("decode trigger statement counter: {e}"))?,
            None => 0,
        }
    };
    let next = current
        .checked_add(1)
        .ok_or_else(|| "trigger statement counter exhausted".to_string())?;
    let blob = rmp_serde::to_vec_named(&next).map_err(|e| format!("encode counter: {e}"))?;
    ledger
        .insert(key.as_str(), blob.as_slice())
        .map_err(map_err)?;
    Ok(next)
}

fn append_reaction(
    wtx: &SqlWrite<'_>,
    scope: &str,
    reaction: &TriggerReaction,
) -> Result<(), String> {
    let key = trigger_reaction_key(
        scope,
        reaction.statement_id,
        &reaction.event_id,
        &reaction.trigger,
    );
    let mut ledger = wtx.open_table(SQL_TRIGGER_REACTIONS)?;
    if ledger.get(key.as_str()).map_err(map_err)?.is_some() {
        return Err(format!(
            "{TRIGGER_REACTION_DUPLICATE}: trigger `{}` already reacted to event {} of this write",
            reaction.trigger, reaction.event_id
        ));
    }
    let blob = rmp_serde::to_vec_named(reaction).map_err(|e| format!("encode reaction: {e}"))?;
    ledger
        .insert(key.as_str(), blob.as_slice())
        .map_err(map_err)?;
    Ok(())
}

/// One row event a trigger reacts to.
struct RowEvent<'a> {
    timing: TriggerTiming,
    event: TriggerEvent,
    old: Option<&'a Map<String, Value>>,
    new: Option<Map<String, Value>>,
}

/// The content digest of one row event: the table, timing, kind and both rows.
fn event_digest(
    table: &str,
    timing: TriggerTiming,
    event: TriggerEvent,
    old: Option<&Map<String, Value>>,
    new: Option<&Map<String, Value>>,
) -> String {
    let body = json!([table, timing.as_str(), event.as_str(), old, new]);
    hex::encode(Sha256::digest(body.to_string().as_bytes()))
}

/// The value of an effect argument: a literal, or `{"from": "<context key>"}`.
fn effect_value(value: &Value, context: &Context) -> Value {
    if let Some(key) = value
        .as_object()
        .filter(|object| object.len() == 1)
        .and_then(|object| object.get("from"))
        .and_then(Value::as_str)
    {
        return context.get(key).cloned().unwrap_or(Value::Null);
    }
    value.clone()
}

fn effect_object(
    args: &Value,
    field: &str,
    context: &Context,
) -> Result<Map<String, Value>, String> {
    let object = args
        .get(field)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{TRIGGER_MACHINE_INVALID}: effect needs an object `{field}`"))?;
    Ok(object
        .iter()
        .map(|(column, value)| (column.clone(), effect_value(value, context)))
        .collect())
}

fn effect_selector(args: &Value, context: &Context) -> Result<RowPredicate, String> {
    let selector = effect_object(args, "where", context)?;
    if selector.is_empty() {
        return Err(format!(
            "{TRIGGER_MACHINE_INVALID}: effect needs a non-empty `where`"
        ));
    }
    Ok(RowPredicate::And(
        selector
            .into_iter()
            .map(|(col, value)| RowPredicate::Cmp {
                col,
                op: CmpOp::Eq,
                value,
            })
            .collect(),
    ))
}

/// Run one table-writing effect of an `After` machine as a nested write.
fn run_write_effect(
    wtx: &SqlWrite<'_>,
    scope: &str,
    nested: Chain,
    name: &str,
    args: &Value,
    context: &Context,
) -> Result<(), String> {
    let table = args
        .get("table")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{TRIGGER_MACHINE_INVALID}: effect `{name}` needs a `table`"))?;
    match name {
        "insert_row" => {
            let row = effect_object(args, "row", context)?;
            let columns: Vec<String> = row.keys().cloned().collect();
            let values: Vec<Value> = row.values().cloned().collect();
            insert_in_at(wtx, scope, nested, table, &columns, &[values]).map(|_| ())
        }
        "update_rows" => {
            let set = effect_object(args, "set", context)?;
            let selector = effect_selector(args, context)?;
            update_rows_in_at(wtx, scope, nested, table, &set, &selector).map(|_| ())
        }
        "delete_rows" => {
            let selector = effect_selector(args, context)?;
            delete_in_at(wtx, scope, nested, table, &selector).map(|_| ())
        }
        other => Err(format!(
            "{TRIGGER_MACHINE_INVALID}: unknown trigger effect `{other}`"
        )),
    }
}

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
    wtx: &SqlWrite<'_>,
    scope: &str,
    chain: Chain,
    schema: &TableSchema,
    row_event: RowEvent<'_>,
) -> Result<Option<Map<String, Value>>, String> {
    let RowEvent {
        timing,
        event,
        old,
        new,
    } = row_event;
    let mut current = new;
    let mut chain = chain;
    for trigger in schema
        .triggers()
        .iter()
        .filter(|t| t.timing == timing && t.reacts_to(event))
    {
        let level = chain.depth.saturating_add(1);
        let max = DEPTH_MAX.load(Ordering::Relaxed);
        if level > max {
            return Err(format!(
                "{TRIGGER_DEPTH}: trigger `{}` on `{}` nests deeper than {max}",
                trigger.name, schema.name
            ));
        }
        let statement = match chain.statement {
            Some(statement) => statement,
            None => {
                let statement = next_statement(wtx, scope)?;
                chain.statement = Some(statement);
                statement
            }
        };
        append_reaction(
            wtx,
            scope,
            &TriggerReaction {
                event_id: event_digest(&schema.name, timing, event, old, current.as_ref()),
                trigger: trigger.name.clone(),
                statement_id: statement,
            },
        )?;
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
        let nested = Chain {
            depth: level,
            statement: Some(statement),
        };
        for action in &outcome.actions {
            match action {
                Action::Emit { signal } => {
                    if let Some(message) = refusal_message(signal) {
                        return Err(format!(
                            "{TRIGGER_REFUSED}: trigger `{}` on `{}`: {message}",
                            trigger.name, schema.name
                        ));
                    }
                }
                Action::Custom { name, args } => {
                    if timing != TriggerTiming::After {
                        return Err(format!(
                            "{TRIGGER_MACHINE_INVALID}: trigger `{}` on `{}`: only an after trigger may write",
                            trigger.name, schema.name
                        ));
                    }
                    run_write_effect(wtx, scope, nested, name, args, &outcome.next_context)?;
                }
                _ => {}
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
    chain: Chain,
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
            wtx,
            tenant_scope,
            chain,
            schema,
            RowEvent {
                timing: TriggerTiming::Before,
                event: TriggerEvent::Insert,
                old: None,
                new: Some(supplied),
            },
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
                wtx,
                tenant_scope,
                chain,
                schema,
                RowEvent {
                    timing: TriggerTiming::After,
                    event: TriggerEvent::Insert,
                    old: None,
                    new: Some(row_map(schema, stored)),
                },
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
    wtx: &SqlWrite<'_>,
    scope: &str,
    chain: Chain,
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
        wtx,
        scope,
        chain,
        schema,
        RowEvent {
            timing: TriggerTiming::Before,
            event: TriggerEvent::Update,
            old: Some(&old),
            new: Some(new.clone()),
        },
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
    wtx: &SqlWrite<'_>,
    scope: &str,
    chain: Chain,
    schema: &TableSchema,
    old_cells: &[Cell],
    cells: &[Cell],
) -> Result<(), String> {
    if !has_trigger(schema, TriggerTiming::After, TriggerEvent::Update) {
        return Ok(());
    }
    run_triggers(
        wtx,
        scope,
        chain,
        schema,
        RowEvent {
            timing: TriggerTiming::After,
            event: TriggerEvent::Update,
            old: Some(&row_map(schema, old_cells)),
            new: Some(row_map(schema, cells)),
        },
    )?;
    Ok(())
}

/// Before-delete: a trigger may refuse; there is no new row to rewrite.
pub(super) fn before_delete(
    wtx: &SqlWrite<'_>,
    scope: &str,
    chain: Chain,
    schema: &TableSchema,
    old_cells: &[Cell],
) -> Result<(), String> {
    delete_event(wtx, scope, chain, schema, TriggerTiming::Before, old_cells)
}

/// After-delete: effects only.
pub(super) fn after_delete(
    wtx: &SqlWrite<'_>,
    scope: &str,
    chain: Chain,
    schema: &TableSchema,
    old_cells: &[Cell],
) -> Result<(), String> {
    delete_event(wtx, scope, chain, schema, TriggerTiming::After, old_cells)
}

fn delete_event(
    wtx: &SqlWrite<'_>,
    scope: &str,
    chain: Chain,
    schema: &TableSchema,
    timing: TriggerTiming,
    old_cells: &[Cell],
) -> Result<(), String> {
    if !has_trigger(schema, timing, TriggerEvent::Delete) {
        return Ok(());
    }
    run_triggers(
        wtx,
        scope,
        chain,
        schema,
        RowEvent {
            timing,
            event: TriggerEvent::Delete,
            old: Some(&row_map(schema, old_cells)),
            new: None,
        },
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

/// Attach `spec` to its table inside the write transaction `wtx`.
pub(super) fn create_trigger_in(wtx: &SqlWrite<'_>, spec: &TriggerSpec) -> Result<(), String> {
    spec.validate()?;
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
}

/// Remove trigger `name` from `table` inside the write transaction `wtx`.
pub(super) fn drop_trigger_in(wtx: &SqlWrite<'_>, table: &str, name: &str) -> Result<(), String> {
    let mut schema =
        get_schema_in(wtx, table)?.ok_or_else(|| format!("table `{table}` does not exist"))?;
    let before = schema.triggers().len();
    schema.triggers_mut().retain(|t| t.name != name);
    if schema.triggers().len() == before {
        return Err(format!(
            "{TRIGGER_UNKNOWN}: trigger `{name}` does not exist on `{table}`"
        ));
    }
    put_triggers_in(wtx, &schema)
}

impl TableStore {
    /// Attach a trigger to its table (stored in the table's schema record).
    pub fn create_trigger(&self, spec: &TriggerSpec) -> Result<(), String> {
        spec.validate()?;
        self.authority
            .maintain("create-trigger", &spec.table, |wtx| {
                create_trigger_in(wtx, spec)
            })
    }

    /// Remove a trigger from its table.
    pub fn drop_trigger(&self, table: &str, name: &str) -> Result<(), String> {
        self.authority.maintain("drop-trigger", table, |wtx| {
            drop_trigger_in(wtx, table, name)
        })
    }
}

/// Where a `CREATE TRIGGER` statement finds the stored statechart it names.
/// The table store cannot see the statechart store, so the composition root
/// installs one source (MEDULLA-ENGINE-PARITY-R012.2).
pub trait TriggerMachineSource: Send + Sync {
    /// The stored definition `machine_id` names in the engine rooted at
    /// `persist_dir`, or the reason it cannot be read.
    fn machine(
        &self,
        persist_dir: &std::path::Path,
        machine_id: &str,
    ) -> Result<medulla_statechart::StatechartDef, String>;
}

static MACHINE_SOURCE: std::sync::OnceLock<std::sync::Arc<dyn TriggerMachineSource>> =
    std::sync::OnceLock::new();

/// Install the machine source. Set once by the composition root; a second
/// install is ignored.
pub fn install_trigger_machine_source(source: std::sync::Arc<dyn TriggerMachineSource>) {
    let _ = MACHINE_SOURCE.set(source);
}

/// Build the `TriggerSpec` a `CREATE TRIGGER … EXECUTE MACHINE` statement
/// names, reading the machine through the installed source. An unknown id, or
/// an engine with no statechart store, refuses `TRIGGER_MACHINE_INVALID`.
pub fn resolve_trigger_spec(
    persist_dir: &std::path::Path,
    table: &str,
    name: &str,
    timing: TriggerTiming,
    events: Vec<TriggerEvent>,
    machine_id: &str,
) -> Result<TriggerSpec, String> {
    let source = MACHINE_SOURCE.get().ok_or_else(|| {
        format!("{TRIGGER_MACHINE_INVALID}: no statechart store serves trigger machines")
    })?;
    let machine = source
        .machine(persist_dir, machine_id)
        .map_err(|reason| format!("{TRIGGER_MACHINE_INVALID}: machine `{machine_id}`: {reason}"))?;
    let spec = TriggerSpec {
        table: table.to_string(),
        name: name.to_string(),
        timing,
        events,
        machine_id: machine_id.to_string(),
        machine,
    };
    spec.validate()?;
    Ok(spec)
}
