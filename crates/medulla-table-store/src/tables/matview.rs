//! Materialized views (MEDULLA-ENGINE-PARITY-R014.1, R014.2).
//!
//! A materialized view is a user table of stored result rows plus one
//! [`MatViewSpec`] in the redb catalog table `__sql_matviews__`. The spec is
//! written only by [`TxnOp::PutMatView`] and removed by [`TxnOp::DropMatView`];
//! the builders below assemble the one [`TableTxn`] each statement commits, so a
//! store-level test can commit them without the wire.

use medulla_tables::schema::{Column, ColumnType, TableConstraint, TableSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::matview_incremental::{
    mv_nonnull_column, mv_sum_column, ExactAgg, IncrementalViewProgram, Row, ViewOutput,
    MV_COUNT, MV_KEY,
};
use super::store::{TableStore, TableTxn, TxnOp};

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
    /// The incremental program of a view the commit maintains
    /// (MEDULLA-INCREMENTAL-VIEWS-R009.3). `None` for a recompute view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<super::matview_incremental::IncrementalViewProgram>,
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
/// that read any of the `written` tables, are not yet stale and carry no incremental program
/// (R009.5: a maintained view is never stale), each with
/// `stale = true`. The commit stores them in the same transaction that wrote the
/// tables, so a materialized view is never fresh over a changed input.
pub fn mark_matviews_stale(specs: Vec<MatViewSpec>, written: &[&str]) -> Vec<MatViewSpec> {
    specs
        .into_iter()
        .filter(|spec| !spec.stale && spec.program.is_none() && spec.inputs.iter().any(|input| written.contains(&input.as_str())))
        .map(|mut spec| {
            spec.stale = true;
            spec
        })
        .collect()
}

/// The primary-key column names of `schema` in column order, empty for a table without one.
pub fn pk_columns(schema: &TableSchema) -> Vec<String> {
    let mut names: Vec<String> = schema
        .columns()
        .iter()
        .filter(|column| column.primary_key)
        .map(|column| column.name.clone())
        .collect();
    for constraint in schema.constraints() {
        if let TableConstraint::PrimaryKey { columns, .. } = constraint {
            names.extend(columns.iter().cloned());
        }
    }
    let order = |name: &String| schema.column_index(name).unwrap_or(usize::MAX);
    names.sort_by_key(order);
    names.dedup();
    names
}

/// The result table of an incremental view: the visible output columns of `visible`, then the
/// hidden `__mv_*` columns the delta code keeps (MEDULLA-INCREMENTAL-VIEWS-R009.3, R009.4), the
/// last being the primary key `__mv_key`.
pub fn incremental_result_schema(
    visible: &TableSchema,
    program: &IncrementalViewProgram,
) -> TableSchema {
    let mut columns: Vec<Column> = visible.columns().to_vec();
    let (_, aggs) = program.group_shape();
    if !matches!(program.output, ViewOutput::Rows(_)) {
        columns.push(Column::new(MV_COUNT, ColumnType::BigInt, false, false));
    }
    for (name, agg) in &aggs {
        if matches!(agg, ExactAgg::Sum(_) | ExactAgg::Avg(_)) {
            columns.push(Column::new(mv_sum_column(name), ColumnType::Text, false, false));
            columns.push(Column::new(mv_nonnull_column(name), ColumnType::BigInt, false, false));
        }
    }
    columns.push(Column::new(MV_KEY, ColumnType::Text, false, true));
    TableSchema::new(visible.name.clone(), columns)
}

/// `CREATE MATERIALIZED VIEW` of an incremental view: the result table with its hidden columns,
/// the initial rows and the spec that carries `program`, in one transaction. The initial rows
/// are the source rows folded through the same delta arithmetic as a commit (R009.5): the
/// defining `SELECT` is not run for them.
pub fn create_incremental_matview_txn(
    store: &TableStore,
    mut spec: MatViewSpec,
    visible: &TableSchema,
    program: IncrementalViewProgram,
) -> Result<TableTxn, String> {
    let source = store
        .get_schema(&program.table)?
        .ok_or_else(|| format!("table `{}` does not exist", program.table))?;
    let pk = pk_columns(&source);
    let mut rows: Vec<(String, Row)> = Vec::new();
    for cells in store.scan(&program.table)? {
        let mut row = Row::new();
        for (index, column) in source.columns().iter().enumerate() {
            let cell = cells.get(index).cloned().unwrap_or(medulla_tables::schema::Cell::Null);
            row.insert(column.name.clone(), cell.to_typed_json(column.ty));
        }
        rows.push((super::matview_incremental::row_key_text(&pk, &row), row));
    }
    let initial = program.initial_rows(&rows)?;
    let schema = incremental_result_schema(visible, &program);
    let col_order: Vec<String> = schema.columns().iter().map(|c| c.name.clone()).collect();
    let values = initial
        .iter()
        .map(|row| {
            col_order
                .iter()
                .map(|name| row.get(name).cloned().unwrap_or(Value::Null))
                .collect()
        })
        .collect();
    spec.program = Some(program);
    Ok(create_matview_txn(spec, schema, col_order, values))
}
