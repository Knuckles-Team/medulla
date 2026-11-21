//! The `ORDER BY` and `SKIP` stages (MEDULLA-FEDERATED-QUERY-R734).
//!
//! `ORDER BY` is a stable sort over the named fields, read the way `GROUP` reads a key (the
//! row's typed cell, else its node property, `label` the node type), with the row id as the
//! last key so a page is repeatable. A row that lacks a key sorts after every row that has
//! it, in either direction. A field no input row carries refuses `UQL_UNKNOWN_FIELD`.
//! `SKIP n` drops the first `n` rows; `SKIP n |> LIMIT k` is the page. An `n` above
//! [`MAX_SKIP_ROWS`] refuses `UQL_SKIP_LIMIT`.

use std::cmp::Ordering;

use medulla_query_types::wire::OrderKey;
use medulla_types::contract::ServerErrorCode;

use super::group::field;
use super::PlanCtx;
use crate::rowset::{Cell, RowSet};

/// The most rows a `SKIP` may page past.
pub const MAX_SKIP_ROWS: u64 = 100_000;

fn refusal(code: ServerErrorCode, detail: &str) -> String {
    format!("{}: {detail}", code.as_str())
}

/// Order two present cells: numbers by value, text by bytes, booleans false before true;
/// cells of different kinds order numbers, then text, then booleans.
fn compare(a: &Cell, b: &Cell) -> Ordering {
    match (a, b) {
        (Cell::Text(x), Cell::Text(y)) => x.cmp(y),
        (Cell::Bool(x), Cell::Bool(y)) => x.cmp(y),
        _ => match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => kind_rank(a).cmp(&kind_rank(b)),
        },
    }
}

fn kind_rank(cell: &Cell) -> u8 {
    match cell {
        Cell::Text(_) => 1,
        _ => 2,
    }
}

/// `ORDER BY keys`: a stable sort with the row id as the last key.
pub(super) fn order(ctx: &PlanCtx, input: RowSet, keys: &[OrderKey]) -> Result<RowSet, String> {
    for key in keys {
        let carried = input
            .rows()
            .iter()
            .any(|row| field(ctx, &input, row, &key.field).is_some());
        if !input.is_empty() && !carried {
            return Err(refusal(
                ServerErrorCode::UqlUnknownField,
                &format!("no input row carries the field `{}`", key.field),
            ));
        }
    }
    // Read every key once per row, not once per comparison.
    let values: std::collections::HashMap<String, Vec<Option<Cell>>> = input
        .rows()
        .iter()
        .map(|row| {
            let cells = keys
                .iter()
                .map(|key| field(ctx, &input, row, &key.field))
                .collect();
            (row.id.clone(), cells)
        })
        .collect();
    Ok(input.clone().sorted_by(|a, b| {
        let (left, right) = (&values[&a.id], &values[&b.id]);
        for (index, key) in keys.iter().enumerate() {
            let ordering = match (&left[index], &right[index]) {
                (Some(x), Some(y)) if key.descending => compare(y, x),
                (Some(x), Some(y)) => compare(x, y),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        a.id.cmp(&b.id)
    }))
}

/// `SKIP n`: drop the first `n` rows.
pub(super) fn skip(input: RowSet, n: u64) -> Result<RowSet, String> {
    if n > MAX_SKIP_ROWS {
        return Err(refusal(
            ServerErrorCode::UqlSkipLimit,
            &format!("SKIP {n} pages past {MAX_SKIP_ROWS} rows; narrow the query with WHERE"),
        ));
    }
    Ok(input.skip(usize::try_from(n).unwrap_or(usize::MAX)))
}
