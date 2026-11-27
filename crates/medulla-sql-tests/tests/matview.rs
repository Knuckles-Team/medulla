//! Materialized views (MEDULLA-ENGINE-PARITY-R014.1), driven through the table
//! store builders the wire commits.
#![cfg(feature = "sql")]

use medulla_graph_core::graph::GraphCore;
use medulla_sql::sql::exec_sql_typed_with_tables;
use medulla_sql_lang::sql::classify::{classify, StatementKind};
use medulla_sql_lang::sql::typed_result::matview_result_schema;
use medulla_table_store::tables::{
    create_matview_txn, drop_matview_txn, refresh_matview_txn, MatViewSpec, TableStore, TableTxn,
    TxnOp,
};
use medulla_tables::schema::{Column, ColumnType, TableSchema};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "common/commit_batch.rs"]
mod commit_batch;

static COMMITS: AtomicU64 = AtomicU64::new(0);

/// Commit through the batch path, which records the writes the store's
/// `commit_seq` counts.
fn commit(store: &TableStore, txn: &TableTxn) {
    let n = COMMITS.fetch_add(1, Ordering::SeqCst) + 1;
    store
        .commit_txn_batch(
            txn,
            &commit_batch::batch(store, &format!("mv-{n}"), &format!("idem-mv-{n}")),
            n,
        )
        .unwrap();
}

fn input_table(store: &TableStore) {
    let mut txn = TableTxn::new();
    txn.push(TxnOp::CreateTable {
        schema: TableSchema::new(
            "orders",
            vec![
                Column::new("id", ColumnType::BigInt, false, true),
                Column::new("sku", ColumnType::Text, true, false),
            ],
        ),
        if_not_exists: false,
    });
    commit(store, &txn);
}

fn insert_order(store: &TableStore, id: i64, sku: &str) {
    let mut txn = TableTxn::new();
    txn.push(TxnOp::Insert {
        table: "orders".to_string(),
        col_order: vec!["id".to_string(), "sku".to_string()],
        rows: vec![vec![json!(id), json!(sku)]],
    });
    commit(store, &txn);
}

/// The view's builders as the wire calls them: run the select, then commit.
fn build(
    store: &TableStore,
    select_sql: &str,
    make: fn(MatViewSpec, TableSchema, Vec<String>, Vec<Vec<serde_json::Value>>) -> TableTxn,
) -> MatViewSpec {
    let view = GraphCore::new().analysis_snapshot();
    let seq = store.commit_seq();
    let read = exec_sql_typed_with_tables(&view, store, select_sql).unwrap();
    let spec = MatViewSpec {
        name: "mv".to_string(),
        select_sql: select_sql.to_string(),
        inputs: vec!["orders".to_string()],
        owner: "tester".to_string(),
        refreshed_seq: seq,
        stale: false,
        program: None,
    };
    let schema = matview_result_schema("mv", &read.columns);
    let col_order = read.columns.iter().map(|c| c.name.clone()).collect();
    commit(store, &make(spec.clone(), schema, col_order, read.rows));
    spec
}

fn mv_rows(store: &TableStore) -> usize {
    let view = GraphCore::new().analysis_snapshot();
    exec_sql_typed_with_tables(&view, store, "SELECT id FROM mv")
        .unwrap()
        .rows
        .len()
}

// spec: MEDULLA-ENGINE-PARITY-R014.1
#[test]
fn refresh_recomputes_and_records_input_versions() {
    let (store, _path) = TableStore::open_temp().unwrap();
    input_table(&store);
    insert_order(&store, 1, "a");
    let StatementKind::CreateMatView(plan) =
        classify("CREATE MATERIALIZED VIEW mv AS SELECT id, sku FROM orders").unwrap()
    else {
        panic!("expected CreateMatView");
    };
    let created = build(&store, &plan.select_sql, create_matview_txn);
    assert_eq!(mv_rows(&store), 1);
    let stored = store.get_matview("mv").unwrap().expect("spec recorded");
    assert_eq!(stored.inputs, vec!["orders".to_string()]);
    assert_eq!(stored.refreshed_seq, created.refreshed_seq);

    insert_order(&store, 2, "b");
    assert_eq!(mv_rows(&store), 1, "a read of the view does not recompute");
    let refreshed = build(&store, &stored.select_sql, refresh_matview_txn);
    assert_eq!(mv_rows(&store), 2, "refresh recomputes the result rows");
    let after = store.get_matview("mv").unwrap().expect("spec kept");
    assert_eq!(after.inputs, vec!["orders".to_string()]);
    assert!(
        after.refreshed_seq > created.refreshed_seq,
        "refreshed_seq advanced"
    );
    assert_eq!(after.refreshed_seq, refreshed.refreshed_seq);

    commit(&store, &drop_matview_txn("mv"));
    assert!(store.get_matview("mv").unwrap().is_none());
    assert!(store.get_schema("mv").unwrap().is_none());
}
