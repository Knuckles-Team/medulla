//! `EDGES [:rel] [WHERE pred]` (MEDULLA-FEDERATED-QUERY-R735): the edges of the caller's
//! snapshot as rows.
//!
//! `TRAVERSE` returns nodes; the relationship and graph views need edges. This source walks
//! `view.edge_properties` in `(source, target)` key order and each key's stored blobs in stored
//! order, the `(source, target, ord)` triple `GetEdgesPage` pages by. An edge is kept when it
//! is live at the plan's `AS OF` pin, its `relationship` equals `rel` when one is given, and
//! every relational predicate holds over its columns. The row id is `source|target|ord`.
//!
//! The view is the served snapshot already narrowed by the caller's read authority, the same
//! one `MATCH` reads, so this source sees exactly the edges a node read would traverse and adds
//! no second scope check.

use serde_json::{Map, Value};

use super::PlanCtx;
use crate::pred_eval;
use crate::rowset::{Cell, RowSet};
use medulla_query_types::wire::Pred;

/// The column an edge's relationship reads under.
const RELATIONSHIP: &str = "relationship";

/// The typed cell of a scalar property; a non-scalar property is not projected.
fn scalar_cell(value: &Value) -> Option<Cell> {
    match value {
        Value::Bool(b) => Some(Cell::Bool(*b)),
        Value::Number(n) => Some(match n.as_i64() {
            Some(i) => Cell::Int(i),
            None => Cell::Float(n.as_f64()?),
        }),
        Value::String(s) => Some(Cell::Text(s.clone())),
        _ => None,
    }
}

/// Run one `EDGES` stage (a source: its input is ignored, like `MATCH`).
pub(super) fn scan_edges(
    ctx: &PlanCtx,
    rel: Option<&str>,
    preds: &[Pred],
) -> Result<RowSet, String> {
    if let Some(pred) = preds.iter().find(|p| !pred_eval::is_relational(p)) {
        return Err(format!(
            "EDGES filters on the edge's columns with relational predicates only; \
             `{pred:?}` is not one"
        ));
    }
    let mut keys: Vec<&(String, String)> = ctx.view.edge_properties.keys().collect();
    keys.sort_unstable();
    let mut kept: Vec<(String, Map<String, Value>)> = Vec::new();
    let mut scanned = 0usize;
    for key in keys {
        let (source, target) = key;
        for (ord, blob) in ctx.view.edge_properties[key].iter().enumerate() {
            scanned += 1;
            ctx.budget.check_traversal(scanned)?;
            let Some(mut props) = medulla_types::msgpack::decode_property_value(blob.as_slice())
                .ok()
                .and_then(|value| value.as_object().cloned())
            else {
                continue;
            };
            if !super::pin_admits(ctx.as_of_pin, &props) {
                continue;
            }
            let relationship = props
                .get(RELATIONSHIP)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if rel.is_some_and(|wanted| relationship != wanted) {
                continue;
            }
            let id = format!("{source}|{target}|{ord}");
            props.insert("id".into(), Value::String(id.clone()));
            props.insert("source".into(), Value::String(source.clone()));
            props.insert("target".into(), Value::String(target.clone()));
            props.insert("rel".into(), Value::String(relationship));
            if pred_eval::all_hold(&props, preds) {
                kept.push((id, props));
            }
        }
    }
    let mut rows = RowSet::from_ids(kept.iter().map(|(id, _)| id.clone()));
    for (id, props) in &kept {
        for (name, value) in props {
            if let Some(cell) = scalar_cell(value) {
                rows = rows.with_cell(id, name, cell);
            }
        }
    }
    Ok(rows)
}
