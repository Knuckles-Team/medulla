//! Incremental view maintenance in the commit (MEDULLA-INCREMENTAL-VIEWS-R009.3, R009.4,
//! R010.2).
//!
//! `maintain_views` runs inside the redb write transaction that applied the commit's table ops,
//! so the view result tables change with the source rows or not at all. It reads the programs
//! stored in the `__sql_matviews__` catalog and turns the transaction's row changes into
//! deltas of weight `-1` (the old image) and `+1` (the new image).

use std::collections::{BTreeMap, BTreeSet};

use medulla_result_types::row_predicate::RowPredicate;
use medulla_tables::schema::TableSchema;
use medulla_types::row_change::RowChange;
use redb::ReadableTable;
use serde_json::Value;

use super::{
    decode_stored, delete_in, get_schema_in, insert_in, map_err, row_map, SqlWrite, MATVIEWS, ROWS,
};
use crate::tables::matview::MatViewSpec;
use crate::tables::matview_incremental::{
    group_key_text, merged_row, row_key_text, GroupState, IncrementalViewProgram, Row, ViewOutput,
    ViewRowChanges, MV_KEY,
};

/// The incremental programs stored in the catalog, read in the commit's own write transaction.
pub(super) fn load_programs(wtx: &SqlWrite<'_>) -> Result<Vec<IncrementalViewProgram>, String> {
    let matviews = wtx.open_table(MATVIEWS)?;
    let mut programs = Vec::new();
    for entry in matviews.iter().map_err(map_err)? {
        let (_, value) = entry.map_err(map_err)?;
        let spec: MatViewSpec = decode_stored(value.value(), "matview")?;
        programs.extend(spec.program);
    }
    Ok(programs)
}

fn decode_image(bytes: &[u8]) -> Result<Row, String> {
    rmp_serde::from_slice::<Row>(bytes).map_err(|error| format!("decode row image: {error}"))
}

fn pk_names(schema: &TableSchema) -> Vec<String> {
    crate::tables::matview::pk_columns(schema)
}

/// One weighted source row of a view's table.
struct Weighted {
    weight: i64,
    key: String,
    row: Row,
}

/// The weighted source rows of `table` in commit order: `-1` for a change's old image and `+1`
/// for its new image, each keyed by its own primary key.
fn deltas_of(
    wtx: &SqlWrite<'_>,
    table: &str,
    changes: &[RowChange],
) -> Result<Vec<Weighted>, String> {
    let schema =
        get_schema_in(wtx, table)?.ok_or_else(|| format!("table `{table}` does not exist"))?;
    let pk = pk_names(&schema);
    let mut out = Vec::new();
    for change in changes.iter().filter(|change| change.table == table) {
        for (weight, image) in [(-1i64, &change.before), (1i64, &change.after)] {
            if let Some(bytes) = image {
                let row = decode_image(bytes)?;
                out.push(Weighted {
                    weight,
                    key: row_key_text(&pk, &row),
                    row,
                });
            }
        }
    }
    Ok(out)
}

/// The stored result rows of `view` whose `__mv_key` is in `keys`.
fn existing_rows(
    wtx: &SqlWrite<'_>,
    view: &str,
    schema: &TableSchema,
    keys: &BTreeSet<String>,
) -> Result<BTreeMap<String, Row>, String> {
    let rows = wtx.open_table(ROWS)?;
    let width = schema.columns().len();
    let mut found = BTreeMap::new();
    for entry in rows.range((view, 0u64)..=(view, u64::MAX)).map_err(map_err)? {
        let (_, value) = entry.map_err(map_err)?;
        let mut cells: Vec<medulla_tables::schema::Cell> = decode_stored(value.value(), "row")?;
        if cells.len() < width {
            cells.resize(width, medulla_tables::schema::Cell::Null);
        }
        let row = row_map(schema, &cells);
        if let Some(Value::String(key)) = row.get(MV_KEY) {
            if keys.contains(key) {
                found.insert(key.clone(), row);
            }
        }
    }
    Ok(found)
}

/// The visible columns of a stored result row (the hidden `__mv_*` columns removed).
fn visible(row: &Row) -> Row {
    row.iter()
        .filter(|(name, _)| !crate::tables::matview_incremental::is_hidden_column(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// Replaces the result rows named by `target` (`__mv_key` to the new row, `None` to delete) and
/// reports what was removed and written.
fn write_result(
    wtx: &SqlWrite<'_>,
    tenant_scope: &str,
    view: &str,
    schema: &TableSchema,
    before: &BTreeMap<String, Row>,
    target: BTreeMap<String, Option<Row>>,
) -> Result<ViewRowChanges, String> {
    let mut changes = ViewRowChanges {
        view: view.to_string(),
        ..ViewRowChanges::default()
    };
    // A row that is unchanged stays as it is.
    let touched: Vec<(String, Option<Row>)> = target
        .into_iter()
        .filter(|(key, new)| before.get(key) != new.as_ref())
        .collect();
    if touched.is_empty() {
        return Ok(changes);
    }
    let removed_keys: Vec<Value> = touched
        .iter()
        .filter(|(key, _)| before.contains_key(key))
        .map(|(key, _)| Value::String(key.clone()))
        .collect();
    if !removed_keys.is_empty() {
        delete_in(
            wtx,
            tenant_scope,
            view,
            &RowPredicate::In {
                col: MV_KEY.to_string(),
                values: removed_keys,
            },
        )?;
        changes.deleted.extend(
            touched
                .iter()
                .filter_map(|(key, _)| before.get(key))
                .map(visible),
        );
    }
    let columns: Vec<String> = schema.columns().iter().map(|c| c.name.clone()).collect();
    let mut rows: Vec<Vec<Value>> = Vec::new();
    for (_, new) in &touched {
        if let Some(row) = new {
            rows.push(
                columns
                    .iter()
                    .map(|name| row.get(name).cloned().unwrap_or(Value::Null))
                    .collect(),
            );
            changes.inserted.push(visible(row));
        }
    }
    if !rows.is_empty() {
        insert_in(wtx, tenant_scope, view, &columns, &rows)?;
    }
    Ok(changes)
}


/// The knob `MEDULLA_VIEW_JOIN_FANOUT` (MEDULLA-INCREMENTAL-VIEWS-R013.2): the most rows one
/// join probe may read per key. Default 10000, range [1, 1000000]; an out-of-range or unparsable
/// value falls back to the default with one warning. Read once at first use.
fn join_fanout() -> usize {
    use std::sync::OnceLock;
    const DEFAULT: usize = 10_000;
    static FANOUT: OnceLock<usize> = OnceLock::new();
    *FANOUT.get_or_init(|| match std::env::var("MEDULLA_VIEW_JOIN_FANOUT") {
        Err(_) => DEFAULT,
        Ok(text) => match text.trim().parse::<usize>() {
            Ok(value) if (1..=1_000_000).contains(&value) => value,
            _ => {
                tracing::warn!(
                    knob = "MEDULLA_VIEW_JOIN_FANOUT",
                    "out-of-range knob value; using the default {DEFAULT}"
                );
                DEFAULT
            }
        },
    })
}

/// Every row of `table` as a column map, read in the write transaction (the post-commit table).
fn table_rows(wtx: &SqlWrite<'_>, table: &str, schema: &TableSchema) -> Result<Vec<Row>, String> {
    let rows = wtx.open_table(ROWS)?;
    let width = schema.columns().len();
    let mut out = Vec::new();
    for entry in rows.range((table, 0u64)..=(table, u64::MAX)).map_err(map_err)? {
        let (_, value) = entry.map_err(map_err)?;
        let mut cells: Vec<medulla_tables::schema::Cell> = decode_stored(value.value(), "row")?;
        if cells.len() < width {
            cells.resize(width, medulla_tables::schema::Cell::Null);
        }
        out.push(row_map(schema, &cells));
    }
    Ok(out)
}

/// Probes `rows` for the ones whose `column` equals `key` (never for NULL), refusing a probe
/// that reads more than the fan-out budget.
fn probe<'a>(rows: &'a [Row], column: &str, key: &Value) -> Result<Vec<&'a Row>, String> {
    if key.is_null() {
        return Ok(Vec::new());
    }
    let matched: Vec<&Row> = rows.iter().filter(|row| row.get(column) == Some(key)).collect();
    if matched.len() > join_fanout() {
        return Err(format!(
            "STATE_BUDGET_EXCEEDED: a join probe on `{column}` matched {} rows, above MEDULLA_VIEW_JOIN_FANOUT {}",
            matched.len(),
            join_fanout()
        ));
    }
    Ok(matched)
}

/// The join delta of one commit: `da join B + A join db - da join db`, with `A` and `B` the
/// post-commit tables (MEDULLA-INCREMENTAL-VIEWS-R013.2). Output rows key on the pair of source
/// primary keys.
fn maintain_join(
    wtx: &SqlWrite<'_>,
    tenant_scope: &str,
    program: &IncrementalViewProgram,
    changes: &[RowChange],
) -> Result<ViewRowChanges, String> {
    let join = program
        .join
        .as_ref()
        .ok_or_else(|| "MATVIEW_PROGRAM: not a join view".to_string())?;
    let ViewOutput::Rows(_) = &program.output else {
        return Err("MATVIEW_PROGRAM: a join view outputs rows".to_string());
    };
    let left_schema = get_schema_in(wtx, &program.table)?
        .ok_or_else(|| format!("table `{}` does not exist", program.table))?;
    let right_schema = get_schema_in(wtx, &join.right)?
        .ok_or_else(|| format!("table `{}` does not exist", join.right))?;
    let (left_pk, right_pk) = (pk_names(&left_schema), pk_names(&right_schema));
    let da = deltas_of(wtx, &program.table, changes)?;
    let db = deltas_of(wtx, &join.right, changes)?;
    let a_now = table_rows(wtx, &program.table, &left_schema)?;
    let b_now = table_rows(wtx, &join.right, &right_schema)?;
    let schema = get_schema_in(wtx, &program.view)?.ok_or_else(|| {
        format!("MATVIEW_STATE_CORRUPT: result table `{}` is missing", program.view)
    })?;

    // (net weight, last entering merged row, pair key)
    let mut net: BTreeMap<String, (i64, Option<Row>)> = BTreeMap::new();
    let mut emit = |weight: i64, a: &Row, b: &Row| -> Result<(), String> {
        let key = format!("{}|{}", row_key_text(&left_pk, a), row_key_text(&right_pk, b));
        let merged = merged_row(&program.table, a, &join.right, b);
        let entry = net.entry(key).or_insert((0, None));
        entry.0 += weight;
        if weight > 0 {
            entry.1 = Some(merged);
        }
        Ok(())
    };
    for delta in &da {
        let key = delta.row.get(&join.left_key).cloned().unwrap_or(Value::Null);
        for b in probe(&b_now, &join.right_key, &key)? {
            emit(delta.weight, &delta.row, b)?;
        }
    }
    for delta in &db {
        let key = delta.row.get(&join.right_key).cloned().unwrap_or(Value::Null);
        for a in probe(&a_now, &join.left_key, &key)? {
            emit(delta.weight, a, &delta.row)?;
        }
    }
    for left in &da {
        for right in &db {
            let (l, r) = (left.row.get(&join.left_key), right.row.get(&join.right_key));
            if l.is_some() && l == r && !l.is_some_and(Value::is_null) {
                emit(-(left.weight * right.weight), &left.row, &right.row)?;
            }
        }
    }
    let keys: BTreeSet<String> = net.keys().cloned().collect();
    let before = existing_rows(wtx, &program.view, &schema, &keys)?;
    let mut target: BTreeMap<String, Option<Row>> = BTreeMap::new();
    for (key, (weight, entering)) in net {
        let existing = i64::from(before.contains_key(&key));
        if existing + weight <= 0 {
            if existing == 1 {
                target.insert(key, None);
            }
            continue;
        }
        if let Some(merged) = entering {
            if program.admits(&merged)? {
                target.insert(key.clone(), Some(program.project(&merged, &key)?));
            } else if existing == 1 {
                target.insert(key, None);
            }
        }
    }
    write_result(wtx, tenant_scope, &program.view, &schema, &before, target)
}

/// Maintains one view over the deltas of its table.
fn maintain_one(
    wtx: &SqlWrite<'_>,
    tenant_scope: &str,
    program: &IncrementalViewProgram,
    changes: &[RowChange],
) -> Result<ViewRowChanges, String> {
    if program.join.is_some() {
        return maintain_join(wtx, tenant_scope, program, changes);
    }
    let deltas = deltas_of(wtx, &program.table, changes)?;
    let schema = get_schema_in(wtx, &program.view)?
        .ok_or_else(|| format!("MATVIEW_STATE_CORRUPT: result table `{}` is missing", program.view))?;
    match &program.output {
        ViewOutput::Rows(_) => {
            // The state a key ends in after the changes, replayed in commit order.
            let mut target: BTreeMap<String, Option<Row>> = BTreeMap::new();
            for delta in &deltas {
                if !program.admits(&delta.row)? {
                    continue;
                }
                if delta.weight < 0 {
                    target.insert(delta.key.clone(), None);
                } else {
                    target.insert(delta.key.clone(), Some(program.project(&delta.row, &delta.key)?));
                }
            }
            let keys: BTreeSet<String> = target.keys().cloned().collect();
            let before = existing_rows(wtx, &program.view, &schema, &keys)?;
            write_result(wtx, tenant_scope, &program.view, &schema, &before, target)
        }
        ViewOutput::Groups { .. } | ViewOutput::Distinct(_) => {
            let (keys, aggs) = program.group_shape();
            let (keys, aggs) = (&keys[..], &aggs[..]);
            let mut touched: BTreeMap<String, Vec<(i64, &Row, Vec<Value>)>> = BTreeMap::new();
            for delta in &deltas {
                if !program.admits(&delta.row)? {
                    continue;
                }
                let values = program.group_values(&delta.row)?;
                touched
                    .entry(group_key_text(&values))
                    .or_default()
                    .push((delta.weight, &delta.row, values));
            }
            let names: BTreeSet<String> = touched.keys().cloned().collect();
            let before = existing_rows(wtx, &program.view, &schema, &names)?;
            let mut source: Option<Vec<Row>> = None;
            let mut target: BTreeMap<String, Option<Row>> = BTreeMap::new();
            for (key, group_deltas) in touched {
                let mut group = match before.get(&key) {
                    Some(stored) => GroupState::from_stored(keys, aggs, stored)?,
                    None => GroupState::new(group_deltas[0].2.clone(), aggs.len()),
                };
                for (weight, row, _) in &group_deltas {
                    group.apply(aggs, *weight, row)?;
                }
                if group.count < 0 {
                    return Err(format!(
                        "MATVIEW_STATE_CORRUPT: group {key} of `{}` went below zero rows",
                        program.view
                    ));
                }
                if group.count > 0 && group.needs_recompute() {
                    recompute_group_extreme(wtx, program, &key, &mut group, aggs, &mut source)?;
                }
                let new = if group.count == 0 {
                    None
                } else {
                    Some(group.to_row(keys, aggs, &key)?)
                };
                target.insert(key, new);
            }
            write_result(wtx, tenant_scope, &program.view, &schema, &before, target)
        }
    }
}

/// Recomputes the retracted extremes of one group from the group's rows in the post-commit
/// source table, inside the commit's write transaction (MEDULLA-INCREMENTAL-VIEWS-R012.2). The
/// table is read once per commit and only the one group's rows are folded.
fn recompute_group_extreme(
    wtx: &SqlWrite<'_>,
    program: &IncrementalViewProgram,
    group_key: &str,
    group: &mut GroupState,
    aggs: &[(String, crate::tables::matview_incremental::ExactAgg)],
    source: &mut Option<Vec<Row>>,
) -> Result<(), String> {
    if source.is_none() {
        let schema = get_schema_in(wtx, &program.table)?
            .ok_or_else(|| format!("table `{}` does not exist", program.table))?;
        *source = Some(table_rows(wtx, &program.table, &schema)?);
    }
    let mut members: Vec<&Row> = Vec::new();
    for row in source.as_deref().unwrap_or_default() {
        if program.admits(row)? && group_key_text(&program.group_values(row)?) == group_key {
            members.push(row);
        }
    }
    group.recompute_extremes(aggs, &members)
}

/// Applies the commit's `changes` to every program whose table a change touches, in the write
/// transaction `wtx`, and returns the view rows each program wrote and deleted. A change list
/// that touches no program's table does nothing.
pub(super) fn maintain_views(
    wtx: &SqlWrite<'_>,
    tenant_scope: &str,
    changes: &[RowChange],
    programs: &[IncrementalViewProgram],
) -> Result<Vec<ViewRowChanges>, String> {
    let mut out = Vec::new();
    for program in programs {
        let touches = changes.iter().any(|change| {
            change.table == program.table
                || program.join.as_ref().is_some_and(|join| change.table == join.right)
        });
        if !touches {
            continue;
        }
        let applied = maintain_one(wtx, tenant_scope, program, changes)?;
        if !applied.deleted.is_empty() || !applied.inserted.is_empty() {
            out.push(applied);
        }
    }
    Ok(out)
}
