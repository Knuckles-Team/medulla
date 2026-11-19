//! The `GROUP BY … AGGREGATE …` stage (MEDULLA-FEDERATED-QUERY-R733).
//!
//! One output row per distinct key tuple. A key or an aggregated field is read from the
//! input row's typed cells first, then from its node's stored properties; the key `label`
//! reads the node label. The output columns are the keys (text) then the aggregates
//! (`COUNT` as int, `SUM`/`MIN`/`MAX`/`AVG` as float), written to the one typed tabular
//! channel of the `RowSet` (MEDULLA-FEDERATED-QUERY-R737); there is no second result
//! shape. The output row id is the key tuple joined by `|`, and groups come out in key
//! order so a result repeats.
//!
//! Refusals: more groups than `Budget::max_groups` (the knob `uql.group.max_groups`)
//! refuse `UQL_GROUP_LIMIT`; a key or field that no input row carries refuses
//! `UQL_UNKNOWN_FIELD`. A refused stage returns no rows.

use std::collections::BTreeMap;

use medulla_types::contract::ServerErrorCode;
use medulla_query_types::wire::{GroupAgg, GroupAggFunc};
use serde_json::Value;

use super::PlanCtx;
use crate::rowset::{Cell, Row, RowSet};

/// The key that reads a node's type.
pub const LABEL_KEY: &str = "label";
/// The separator of the key tuple in an output row id.
pub const KEY_SEPARATOR: &str = "|";

fn refusal(code: ServerErrorCode, detail: &str) -> String {
    format!("{}: {detail}", code.as_str())
}

/// A row's value under `name`: its typed cell, else its node's property (`label` reads
/// the node type).
fn field(ctx: &PlanCtx, input: &RowSet, row: &Row, name: &str) -> Option<Cell> {
    if let Some(cell) = input.cell(&row.id, name) {
        return Some(cell.clone());
    }
    let blob = ctx.view.node_properties.get(row.id.as_str())?;
    let props = medulla_types::msgpack::decode_property_value(blob.as_slice()).ok()?;
    if name == LABEL_KEY {
        // The node's label, read from the same fields the label index and `Scan` read
        // (MEDULLA-FEDERATED-QUERY-R022.1): `type`, `node_type`, `label`, then `labels`.
        return medulla_graph_core::graph::NODE_LABEL_KEYS
            .iter()
            .find_map(|key| props.get(*key).and_then(Value::as_str))
            .or_else(|| {
                props
                    .get("labels")
                    .and_then(Value::as_array)
                    .and_then(|labels| labels.iter().find_map(Value::as_str))
            })
            .map(|label| Cell::Text(label.to_string()));
    }
    match props.get(name)? {
        Value::String(text) => Some(Cell::Text(text.clone())),
        Value::Bool(flag) => Some(Cell::Bool(*flag)),
        Value::Number(number) => number
            .as_i64()
            .map(Cell::Int)
            .or_else(|| number.as_f64().map(Cell::Float)),
        _ => None,
    }
}

/// The text of a key cell.
fn key_text(cell: &Cell) -> String {
    match cell {
        Cell::Text(text) => text.clone(),
        Cell::Int(value) => value.to_string(),
        Cell::Float(value) => value.to_string(),
        Cell::Bool(flag) => flag.to_string(),
    }
}

/// The running state of one aggregate over one group.
#[derive(Clone, Debug, Default)]
struct Acc {
    count: i64,
    sum: f64,
    min: Option<f64>,
    max: Option<f64>,
    numeric: i64,
}

impl Acc {
    fn add(&mut self, value: Option<&Cell>, counts_all: bool) {
        if counts_all || value.is_some() {
            self.count += 1;
        }
        if let Some(number) = value.and_then(Cell::as_f64) {
            self.numeric += 1;
            self.sum += number;
            self.min = Some(self.min.map_or(number, |m| m.min(number)));
            self.max = Some(self.max.map_or(number, |m| m.max(number)));
        }
    }

    fn finish(&self, func: GroupAggFunc) -> Option<Cell> {
        match func {
            GroupAggFunc::Count => Some(Cell::Int(self.count)),
            GroupAggFunc::Sum => (self.numeric > 0).then_some(Cell::Float(self.sum)),
            GroupAggFunc::Min => self.min.map(Cell::Float),
            GroupAggFunc::Max => self.max.map(Cell::Float),
            GroupAggFunc::Avg => {
                (self.numeric > 0).then(|| Cell::Float(self.sum / self.numeric as f64))
            }
        }
    }
}

/// One group: its key cells and one accumulator per aggregate.
struct Group {
    keys: Vec<Option<Cell>>,
    accs: Vec<Acc>,
}

/// Refuse a key or aggregated field that no input row carries.
fn check_fields(
    ctx: &PlanCtx,
    input: &RowSet,
    keys: &[String],
    aggs: &[GroupAgg],
) -> Result<(), String> {
    if input.is_empty() {
        return Ok(());
    }
    let names = keys
        .iter()
        .chain(aggs.iter().filter_map(|agg| agg.field.as_ref()));
    for name in names {
        let carried = input
            .rows()
            .iter()
            .any(|row| field(ctx, input, row, name).is_some());
        if !carried {
            return Err(refusal(
                ServerErrorCode::UqlUnknownField,
                &format!("no input row carries the field `{name}`"),
            ));
        }
    }
    Ok(())
}

/// `GROUP BY keys AGGREGATE aggs`: one row per distinct key tuple, in key order.
pub(super) fn group(
    ctx: &PlanCtx,
    input: RowSet,
    keys: &[String],
    aggs: &[GroupAgg],
) -> Result<RowSet, String> {
    check_fields(ctx, &input, keys, aggs)?;
    let max_groups = ctx.budget.max_groups;
    let mut groups: BTreeMap<Vec<String>, Group> = BTreeMap::new();
    for row in input.rows() {
        let key_cells: Vec<Option<Cell>> = keys
            .iter()
            .map(|key| field(ctx, &input, row, key))
            .collect();
        let tuple: Vec<String> = key_cells
            .iter()
            .map(|cell| cell.as_ref().map(key_text).unwrap_or_default())
            .collect();
        if !groups.contains_key(&tuple) && groups.len() >= max_groups {
            return Err(refusal(
                ServerErrorCode::UqlGroupLimit,
                &format!(
                    "GROUP produced more than {max_groups} groups \
                     (knob uql.group.max_groups); add a WHERE or group by fewer keys"
                ),
            ));
        }
        let entry = groups.entry(tuple).or_insert_with(|| Group {
            keys: key_cells,
            accs: vec![Acc::default(); aggs.len()],
        });
        for (acc, agg) in entry.accs.iter_mut().zip(aggs) {
            let value = agg
                .field
                .as_ref()
                .and_then(|name| field(ctx, &input, row, name));
            acc.add(value.as_ref(), agg.field.is_none());
        }
    }
    let ids: Vec<String> = groups
        .keys()
        .map(|tuple| tuple.join(KEY_SEPARATOR))
        .collect();
    let mut out = RowSet::from_ids(ids.iter().cloned());
    for (id, group) in ids.iter().zip(groups.values()) {
        // Every key is written (a row without the key groups under the empty text), so the
        // key columns always lead the aggregates.
        for (name, cell) in keys.iter().zip(&group.keys) {
            let text = cell.as_ref().map(key_text).unwrap_or_default();
            out = out.with_cell(id, name, Cell::Text(text));
        }
        for (agg, acc) in aggs.iter().zip(&group.accs) {
            if let Some(cell) = acc.finish(agg.func) {
                out = out.with_cell(id, &agg.column(), cell);
            }
        }
    }
    Ok(out)
}
