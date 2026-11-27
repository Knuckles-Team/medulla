//! Incremental view maintenance in the commit (MEDULLA-INCREMENTAL-VIEWS-R009.3, R009.4,
//! R012.2, R013.2), driven through the table store the wire commits to. Each case compares the
//! view rows with the defining `SELECT` as an unordered multiset after every commit.
#![cfg(feature = "sql")]

use medulla_graph_core::graph::GraphCore;
use medulla_sql::sql::exec_sql_typed_with_tables;
use medulla_sql_lang::sql::matview_compile::compile_incremental_sql;
use medulla_sql_lang::sql::typed_result::matview_result_schema;
use medulla_table_store::tables::matview_incremental::{
    EquiJoin, IncrementalViewProgram, RowExpr, ViewOutput,
};
use medulla_table_store::tables::{
    create_incremental_matview_txn, MatViewSpec, TableStore, TableTxn, TxnOp,
};
use medulla_tables::schema::{Column, ColumnType, TableSchema};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "common/commit_batch.rs"]
mod commit_batch;

static COMMITS: AtomicU64 = AtomicU64::new(0);

fn commit(store: &TableStore, txn: &TableTxn) {
    let n = COMMITS.fetch_add(1, Ordering::SeqCst) + 1;
    store
        .commit_txn_batch(
            txn,
            &commit_batch::batch(store, &format!("mvi-{n}"), &format!("idem-mvi-{n}")),
            n,
        )
        .unwrap();
}

fn run(store: &TableStore, ops: Vec<TxnOp>) {
    let mut txn = TableTxn::new();
    for op in ops {
        txn.push(op);
    }
    commit(store, &txn);
}

fn create_table(store: &TableStore, name: &str, columns: Vec<Column>) {
    run(
        store,
        vec![TxnOp::CreateTable {
            schema: TableSchema::new(name, columns),
            if_not_exists: false,
        }],
    );
}

fn orders(store: &TableStore) {
    create_table(
        store,
        "orders",
        vec![
            Column::new("id", ColumnType::BigInt, false, true),
            Column::new("region", ColumnType::Text, true, false),
            Column::new("qty", ColumnType::BigInt, true, false),
        ],
    );
}

fn insert(store: &TableStore, table: &str, cols: &[&str], row: Vec<Value>) {
    run(
        store,
        vec![TxnOp::Insert {
            table: table.to_string(),
            col_order: cols.iter().map(|c| c.to_string()).collect(),
            rows: vec![row],
        }],
    );
}

fn insert_order(store: &TableStore, id: i64, region: Value, qty: Value) {
    insert(
        store,
        "orders",
        &["id", "region", "qty"],
        vec![json!(id), region, qty],
    );
}

fn sql(store: &TableStore, text: &str) -> Vec<Vec<Value>> {
    let view = GraphCore::new().analysis_snapshot();
    exec_sql_typed_with_tables(&view, store, text).unwrap().rows
}

fn as_multiset(rows: Vec<Vec<Value>>) -> Vec<String> {
    let mut texts: Vec<String> = rows
        .into_iter()
        .map(|row| {
            row.iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect();
    texts.sort();
    texts
}

fn column_type(store: &TableStore) -> impl Fn(&str, &str) -> Option<ColumnType> + '_ {
    move |table, column| {
        store
            .get_schema(table)
            .ok()
            .flatten()
            .and_then(|schema| schema.column(column).map(|c| c.ty.clone()))
    }
}

fn spec(select_sql: &str, inputs: &[&str]) -> MatViewSpec {
    MatViewSpec {
        name: "mv".to_string(),
        select_sql: select_sql.to_string(),
        inputs: inputs.iter().map(|s| s.to_string()).collect(),
        owner: "tester".to_string(),
        refreshed_seq: 0,
        stale: false,
        program: None,
    }
}

/// Creates the view `mv` as the wire does for an incremental view.
fn create_view(store: &TableStore, select_sql: &str) {
    let program = compile_incremental_sql(
        "mv",
        select_sql,
        &column_type(store),
        &|table: &str, column: &str| {
            medulla_table_store::tables::join_key_indexed(store, table, column)
        },
    )
    .unwrap_or_else(|refusal| panic!("{select_sql}: {refusal}"));
    create_view_from(store, select_sql, program);
}

fn create_view_from(store: &TableStore, select_sql: &str, program: IncrementalViewProgram) {
    let view = GraphCore::new().analysis_snapshot();
    let typed = exec_sql_typed_with_tables(&view, store, select_sql).unwrap();
    let visible = matview_result_schema("mv", &typed.columns);
    let inputs = [program.table.clone()]
        .into_iter()
        .chain(program.join.iter().map(|j| j.right.clone()))
        .collect::<Vec<_>>();
    let inputs: Vec<&str> = inputs.iter().map(String::as_str).collect();
    let txn = create_incremental_matview_txn(store, spec(select_sql, &inputs), &visible, program)
        .unwrap();
    commit(store, &txn);
}

/// The view must equal its defining select, and must stay fresh.
fn assert_fresh(store: &TableStore, select_sql: &str, columns: &str) {
    assert_eq!(
        as_multiset(sql(store, &format!("SELECT {columns} FROM mv"))),
        as_multiset(sql(store, select_sql)),
        "{select_sql}"
    );
    assert!(
        !store.get_matview("mv").unwrap().unwrap().stale,
        "an incremental view is never stale"
    );
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R009.3
#[test]
fn rows_view_tracks_insert_update_delete() {
    let (store, _path) = TableStore::open_temp().unwrap();
    orders(&store);
    insert_order(&store, 1, json!("eu"), json!(5));
    insert_order(&store, 2, json!("us"), json!(1));
    let select = "SELECT id, qty * 2 AS twice FROM orders WHERE qty > 1";
    create_view(&store, select);
    assert_fresh(&store, select, "id, twice");
    insert_order(&store, 3, json!("eu"), json!(7));
    insert_order(&store, 4, json!("eu"), Value::Null);
    assert_fresh(&store, select, "id, twice");
    // An update that leaves the filter, one that enters it, and a key change.
    run(
        &store,
        vec![TxnOp::Update {
            table: "orders".into(),
            set: serde_json::from_value(json!({"qty": 0})).unwrap(),
            selector: medulla_result_types::row_predicate::RowPredicate::Cmp {
                col: "id".into(),
                op: medulla_result_types::row_predicate::CmpOp::Eq,
                value: json!(1),
            },
        }],
    );
    assert_fresh(&store, select, "id, twice");
    run(
        &store,
        vec![TxnOp::Update {
            table: "orders".into(),
            set: serde_json::from_value(json!({"id": 30})).unwrap(),
            selector: medulla_result_types::row_predicate::RowPredicate::Cmp {
                col: "id".into(),
                op: medulla_result_types::row_predicate::CmpOp::Eq,
                value: json!(3),
            },
        }],
    );
    assert_fresh(&store, select, "id, twice");
    run(
        &store,
        vec![TxnOp::Delete {
            table: "orders".into(),
            selector: medulla_result_types::row_predicate::RowPredicate::Cmp {
                col: "id".into(),
                op: medulla_result_types::row_predicate::CmpOp::Eq,
                value: json!(30),
            },
        }],
    );
    assert_fresh(&store, select, "id, twice");
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R009.4
#[test]
fn groups_count_sum_avg_and_empty_group_leaves() {
    let (store, _path) = TableStore::open_temp().unwrap();
    orders(&store);
    insert_order(&store, 1, json!("eu"), json!(5));
    insert_order(&store, 2, json!("eu"), Value::Null);
    insert_order(&store, 3, json!("us"), json!(4));
    let select = "SELECT region, count(*) AS n, sum(qty) AS total, avg(qty) AS mean FROM orders GROUP BY region";
    create_view(&store, select);
    assert_fresh(&store, select, "region, n, total, mean");
    insert_order(&store, 4, json!("eu"), json!(2));
    insert_order(&store, 5, Value::Null, json!(9));
    assert_fresh(&store, select, "region, n, total, mean");
    // The last row of a group leaves: the group's row is deleted.
    run(
        &store,
        vec![TxnOp::Delete {
            table: "orders".into(),
            selector: medulla_result_types::row_predicate::RowPredicate::Cmp {
                col: "id".into(),
                op: medulla_result_types::row_predicate::CmpOp::Eq,
                value: json!(3),
            },
        }],
    );
    assert_fresh(&store, select, "region, n, total, mean");
    assert!(sql(&store, "SELECT region FROM mv WHERE region = 'us'").is_empty());
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R012.2
#[test]
fn retracting_the_minimum_recomputes_only_that_group() {
    let (store, _path) = TableStore::open_temp().unwrap();
    orders(&store);
    for (id, region, qty) in [(1, "eu", 3), (2, "eu", 8), (3, "us", 1), (4, "us", 6)] {
        insert_order(&store, id, json!(region), json!(qty));
    }
    let select = "SELECT region, min(qty) AS lo, max(qty) AS hi FROM orders GROUP BY region";
    create_view(&store, select);
    assert_fresh(&store, select, "region, lo, hi");
    let delete = |id: i64| TxnOp::Delete {
        table: "orders".into(),
        selector: medulla_result_types::row_predicate::RowPredicate::Cmp {
            col: "id".into(),
            op: medulla_result_types::row_predicate::CmpOp::Eq,
            value: json!(id),
        },
    };
    run(&store, vec![delete(1)]);
    assert_fresh(&store, select, "region, lo, hi");
    run(&store, vec![delete(4)]);
    assert_fresh(&store, select, "region, lo, hi");
    let distinct = "SELECT DISTINCT region FROM orders";
    run(
        &store,
        vec![
            TxnOp::DropMatView { name: "mv".into() },
            TxnOp::DropTable {
                name: "mv".into(),
                if_exists: false,
            },
        ],
    );
    create_view(&store, distinct);
    insert_order(&store, 9, json!("eu"), json!(1));
    assert_fresh(&store, distinct, "region");
    run(&store, vec![delete(2)]);
    run(&store, vec![delete(9)]);
    assert_fresh(&store, distinct, "region");
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R013.2
#[test]
fn join_delta_equals_rejoin_for_both_sides() {
    let (store, _path) = TableStore::open_temp().unwrap();
    create_table(
        &store,
        "customers",
        vec![
            Column::new("cid", ColumnType::BigInt, false, true),
            Column::new("area", ColumnType::Text, true, false),
        ],
    );
    create_table(
        &store,
        "sales",
        vec![
            Column::new("sid", ColumnType::BigInt, false, true),
            Column::new("customer_id", ColumnType::BigInt, true, false),
            Column::new("amount", ColumnType::BigInt, true, false),
        ],
    );
    let select =
        "SELECT s.sid, c.area, s.amount FROM sales s JOIN customers c ON s.customer_id = c.cid";
    let program = IncrementalViewProgram {
        view: "mv".into(),
        table: "sales".into(),
        join: Some(EquiJoin {
            right: "customers".into(),
            left_key: "customer_id".into(),
            right_key: "cid".into(),
        }),
        filter: None,
        output: ViewOutput::Rows(vec![
            ("sid".into(), RowExpr::Column("sid".into())),
            ("area".into(), RowExpr::Column("area".into())),
            ("amount".into(), RowExpr::Column("amount".into())),
        ]),
    };
    insert(
        &store,
        "customers",
        &["cid", "area"],
        vec![json!(1), json!("north")],
    );
    insert(
        &store,
        "sales",
        &["sid", "customer_id", "amount"],
        vec![json!(10), json!(1), json!(5)],
    );
    create_view_from(&store, select, program);
    assert_fresh(&store, select, "sid, area, amount");
    // A new sale, a new customer that matches an existing sale, and both in one transaction.
    insert(
        &store,
        "sales",
        &["sid", "customer_id", "amount"],
        vec![json!(11), json!(2), json!(7)],
    );
    insert(
        &store,
        "customers",
        &["cid", "area"],
        vec![json!(2), json!("south")],
    );
    assert_fresh(&store, select, "sid, area, amount");
    run(
        &store,
        vec![
            TxnOp::Insert {
                table: "customers".into(),
                col_order: vec!["cid".into(), "area".into()],
                rows: vec![vec![json!(3), json!("east")]],
            },
            TxnOp::Insert {
                table: "sales".into(),
                col_order: vec!["sid".into(), "customer_id".into(), "amount".into()],
                rows: vec![vec![json!(12), json!(3), json!(9)]],
            },
        ],
    );
    assert_fresh(&store, select, "sid, area, amount");
    run(
        &store,
        vec![TxnOp::Delete {
            table: "customers".into(),
            selector: medulla_result_types::row_predicate::RowPredicate::Cmp {
                col: "cid".into(),
                op: medulla_result_types::row_predicate::CmpOp::Eq,
                value: json!(1),
            },
        }],
    );
    assert_fresh(&store, select, "sid, area, amount");
}

/// A sink that keeps every view change the commit path hands it.
#[derive(Debug, Default)]
struct RecordingSink {
    views: std::sync::Mutex<Vec<(String, medulla_types::row_change::ViewRowChange)>>,
}

impl medulla_types::row_change::RowChangeSink for RecordingSink {
    fn publish(&self, _graph: &str, _changes: Vec<medulla_types::row_change::RowChange>) {}

    fn publish_views(&self, graph: &str, views: Vec<medulla_types::row_change::ViewRowChange>) {
        let mut seen = self.views.lock().unwrap();
        seen.extend(views.into_iter().map(|view| (graph.to_string(), view)));
    }
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R010.2
#[test]
fn maintain_views_reports_view_row_changes() {
    use medulla_types::row_change::RowChangeKind;
    let (store, _path) = TableStore::open_temp().unwrap();
    orders(&store);
    let sink = std::sync::Arc::new(RecordingSink::default());
    store.install_row_change_sink(sink.clone());
    insert_order(&store, 1, json!("eu"), json!(5));
    let select = "SELECT region, count(*) AS n FROM orders GROUP BY region";
    create_view(&store, select);
    sink.views.lock().unwrap().clear();

    // A new group publishes only the insert of its row.
    insert_order(&store, 2, json!("us"), json!(1));
    {
        let seen = sink.views.lock().unwrap();
        assert_eq!(seen.len(), 1, "one view changed");
        let (_, view) = &seen[0];
        assert_eq!(view.view, "mv");
        assert_eq!(view.changes.len(), 1);
        assert_eq!(view.changes[0].kind, RowChangeKind::Insert);
        assert!(view.changes[0].before.is_none() && view.changes[0].after.is_some());
        assert_eq!(view.changes[0].table, "mv");
    }

    // A grown group publishes the delete of its old row, then the insert of the new one, and
    // hidden columns never reach the feed.
    sink.views.lock().unwrap().clear();
    insert_order(&store, 3, json!("eu"), json!(2));
    let seen = sink.views.lock().unwrap();
    let kinds: Vec<_> = seen[0].1.changes.iter().map(|change| change.kind).collect();
    assert_eq!(kinds, [RowChangeKind::Delete, RowChangeKind::Insert]);
    let old: serde_json::Map<String, Value> =
        rmp_serde::from_slice(seen[0].1.changes[0].before.as_ref().unwrap()).unwrap();
    let new: serde_json::Map<String, Value> =
        rmp_serde::from_slice(seen[0].1.changes[1].after.as_ref().unwrap()).unwrap();
    assert_eq!(
        (old["region"].clone(), old["n"].clone()),
        (json!("eu"), json!(1))
    );
    assert_eq!(
        (new["region"].clone(), new["n"].clone()),
        (json!("eu"), json!(2))
    );
    assert!(old
        .keys()
        .chain(new.keys())
        .all(|name| !name.starts_with("__mv_")));
    drop(seen);

    // A commit that touches no view publishes nothing.
    sink.views.lock().unwrap().clear();
    create_table(
        &store,
        "unrelated",
        vec![Column::new("id", ColumnType::BigInt, false, true)],
    );
    assert!(sink.views.lock().unwrap().is_empty());
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R013.2
#[test]
fn join_probe_through_secondary_indexes_equals_rejoin() {
    use medulla_tables::index::SecondaryIndexColumn;
    let (store, _path) = TableStore::open_temp().unwrap();
    let customers = TableSchema::new(
        "customers",
        vec![
            Column::new("cid", ColumnType::BigInt, false, true),
            Column::new("area", ColumnType::Text, true, false),
        ],
    );
    let sales = TableSchema::new(
        "sales",
        vec![
            Column::new("sid", ColumnType::BigInt, false, true),
            Column::new("customer_id", ColumnType::BigInt, true, false),
            Column::new("amount", ColumnType::BigInt, true, false),
        ],
    );
    create_table(&store, "customers", customers.columns().to_vec());
    create_table(&store, "sales", sales.columns().to_vec());
    // Both join keys carry a secondary index, so a probe is an index range read.
    for (schema, table, column) in [
        (&customers, "customers", "cid"),
        (&sales, "sales", "customer_id"),
    ] {
        let spec = store
            .secondary_index_spec(
                table,
                &format!("{table}_{column}"),
                vec![SecondaryIndexColumn::ascending(column)],
                schema,
            )
            .unwrap();
        assert!(store.create_secondary_index(&spec, false).unwrap());
    }
    let select =
        "SELECT s.sid, c.area, s.amount FROM sales s JOIN customers c ON s.customer_id = c.cid";
    let program = IncrementalViewProgram {
        view: "mv".into(),
        table: "sales".into(),
        join: Some(EquiJoin {
            right: "customers".into(),
            left_key: "customer_id".into(),
            right_key: "cid".into(),
        }),
        filter: None,
        output: ViewOutput::Rows(vec![
            ("sid".into(), RowExpr::Column("sid".into())),
            ("area".into(), RowExpr::Column("area".into())),
            ("amount".into(), RowExpr::Column("amount".into())),
        ]),
    };
    insert(
        &store,
        "customers",
        &["cid", "area"],
        vec![json!(1), json!("north")],
    );
    insert(
        &store,
        "sales",
        &["sid", "customer_id", "amount"],
        vec![json!(10), json!(1), json!(5)],
    );
    create_view_from(&store, select, program);
    assert_fresh(&store, select, "sid, area, amount");
    // Both sides in one transaction, a key change on the indexed column, and a delete.
    run(
        &store,
        vec![
            TxnOp::Insert {
                table: "customers".into(),
                col_order: vec!["cid".into(), "area".into()],
                rows: vec![
                    vec![json!(2), json!("south")],
                    vec![json!(3), json!("east")],
                ],
            },
            TxnOp::Insert {
                table: "sales".into(),
                col_order: vec!["sid".into(), "customer_id".into(), "amount".into()],
                rows: vec![
                    vec![json!(11), json!(2), json!(7)],
                    vec![json!(12), json!(3), json!(9)],
                    vec![json!(13), json!(2), json!(1)],
                ],
            },
        ],
    );
    assert_fresh(&store, select, "sid, area, amount");
    run(
        &store,
        vec![TxnOp::Update {
            table: "sales".into(),
            set: serde_json::from_value(json!({"customer_id": 3})).unwrap(),
            selector: medulla_result_types::row_predicate::RowPredicate::Cmp {
                col: "sid".into(),
                op: medulla_result_types::row_predicate::CmpOp::Eq,
                value: json!(11),
            },
        }],
    );
    assert_fresh(&store, select, "sid, area, amount");
    run(
        &store,
        vec![TxnOp::Delete {
            table: "customers".into(),
            selector: medulla_result_types::row_predicate::RowPredicate::Cmp {
                col: "cid".into(),
                op: medulla_result_types::row_predicate::CmpOp::Eq,
                value: json!(2),
            },
        }],
    );
    assert_fresh(&store, select, "sid, area, amount");
}
