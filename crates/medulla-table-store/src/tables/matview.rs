//! Materialized views (MEDULLA-ENGINE-PARITY-R014.1, R014.2).
//!
//! A materialized view is a user table of stored result rows plus one
//! [`MatViewSpec`] in the redb catalog table `__sql_matviews__`. The spec is
//! written only by [`TxnOp::PutMatView`] and removed by [`TxnOp::DropMatView`];
//! the builders below assemble the one [`TableTxn`] each statement commits, so a
//! store-level test can commit them without the wire.

use medulla_tables::schema::TableSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::store::{TableTxn, TxnOp};

/// The error-string prefix of a create over a name that is already a table, a
/// view or a materialized view (SQLSTATE `42P07`).
pub const MATVIEW_EXISTS: &str = "MATVIEW_EXISTS";
/// The error-string prefix of a refresh or drop of a view that does not exist
/// (SQLSTATE `42P01`).
pub const MATVIEW_UNKNOWN: &str = "MATVIEW_UNKNOWN";

/// One materialized view: its stored select, the user tables the select reads,
/// the creating actor, and the store `commit_seq` its rows reflect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatViewSpec {
    pub name: String,
    pub select_sql: String,
    pub inputs: Vec<String>,
    pub owner: String,
    pub refreshed_seq: u64,
    pub stale: bool,
}

/// The ops that store the result rows of `schema` and record `spec`.
fn result_ops(
    txn: &mut TableTxn,
    spec: MatViewSpec,
    schema: TableSchema,
    col_order: Vec<String>,
    rows: Vec<Vec<Value>>,
) {
    txn.push(TxnOp::CreateTable {
        schema,
        if_not_exists: false,
    });
    if !rows.is_empty() {
        txn.push(TxnOp::Insert {
            table: spec.name.clone(),
            col_order,
            rows,
        });
    }
    txn.push(TxnOp::PutMatView { spec });
}

/// `CREATE MATERIALIZED VIEW`: the result table, its rows and the spec in one
/// transaction.
pub fn create_matview_txn(
    spec: MatViewSpec,
    schema: TableSchema,
    col_order: Vec<String>,
    rows: Vec<Vec<Value>>,
) -> TableTxn {
    let mut txn = TableTxn::new();
    result_ops(&mut txn, spec, schema, col_order, rows);
    txn
}

/// `REFRESH MATERIALIZED VIEW`: drop and recreate the result table with the new
/// rows and record the new spec, so the replacement is atomic.
pub fn refresh_matview_txn(
    spec: MatViewSpec,
    schema: TableSchema,
    col_order: Vec<String>,
    rows: Vec<Vec<Value>>,
) -> TableTxn {
    let mut txn = TableTxn::new();
    txn.push(TxnOp::DropTable {
        name: spec.name.clone(),
        if_exists: false,
    });
    result_ops(&mut txn, spec, schema, col_order, rows);
    txn
}

/// `DROP MATERIALIZED VIEW`: remove the spec and the result table.
pub fn drop_matview_txn(name: &str) -> TableTxn {
    let mut txn = TableTxn::new();
    txn.push(TxnOp::DropMatView {
        name: name.to_string(),
    });
    txn.push(TxnOp::DropTable {
        name: name.to_string(),
        if_exists: false,
    });
    txn
}

/// The user tables a transaction writes: the tables of its row ops and of the
/// DDL that rewrites a table's rows or shape (MEDULLA-ENGINE-PARITY-R014.2).
pub fn written_tables(txn: &TableTxn) -> Vec<&str> {
    let mut tables: Vec<&str> = Vec::new();
    for op in &txn.ops {
        match op {
            TxnOp::Insert { table, .. }
            | TxnOp::Update { table, .. }
            | TxnOp::Delete { table, .. }
            | TxnOp::AddColumn { table, .. }
            | TxnOp::DropColumn { table, .. }
            | TxnOp::RenameColumn { table, .. }
            | TxnOp::AlterColumnType { table, .. }
            | TxnOp::DropConstraint { table, .. }
            | TxnOp::AddConstraint { table, .. } => tables.push(table.as_str()),
            TxnOp::DropTable { name, .. } => tables.push(name.as_str()),
            TxnOp::RenameTable { table, new_name } => {
                tables.push(table.as_str());
                tables.push(new_name.as_str());
            }
            _ => {}
        }
    }
    tables
}

/// `mark_matviews_stale` (MEDULLA-ENGINE-PARITY-R014.2): of `specs`, the ones
/// that read any of the `written` tables and are not yet stale, each with
/// `stale = true`. The commit stores them in the same transaction that wrote the
/// tables, so a materialized view is never fresh over a changed input.
pub fn mark_matviews_stale(specs: Vec<MatViewSpec>, written: &[&str]) -> Vec<MatViewSpec> {
    specs
        .into_iter()
        .filter(|spec| !spec.stale && spec.inputs.iter().any(|input| written.contains(&input.as_str())))
        .map(|mut spec| {
            spec.stale = true;
            spec
        })
        .collect()
}
