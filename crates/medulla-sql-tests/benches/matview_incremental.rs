//! Nightly budget for SQL views (MEDULLA-INCREMENTAL-VIEWS-R009.8).
//!
//! 1,000-row changes are committed to a 1,000,000-row table, first with no view and then with
//! the view of each group, through `TableStore::commit_txn_batch`: the added commit time is the
//! p99 with the view minus the p99 without it. Groups `eval` (a projection with arithmetic),
//! `rows` (a filter), `groups` (grouped count, sum and avg), `extremes` (grouped min and max with
//! distinct) and `join` (an indexed inner equi-join). Budget: at most 50 ms added per 1,000-row
//! change and at most 64 MiB of resident growth. One record per group is appended to
//! `target/matview_incremental/records.jsonl`; a breach exits non-zero. A service level of the
//! nightly suite, never a pull request gate.
//!
//! The fixtures are those of `tests/matview_oracle.rs`. Run:
//! `cargo bench -p medulla-sql-tests --features sql --bench matview_incremental`;
//! `MEDULLA_BENCH_ROWS` shrinks the table for a smoke run.
#![allow(dead_code)]

use std::io::Write;
use std::time::Instant;

use medulla_graph_core::graph::GraphCore;
use medulla_result_types::row_predicate::RowPredicate;
use medulla_sql::sql::exec_sql_typed_with_tables;
use medulla_sql_lang::sql::matview_compile::compile_incremental_sql;
use medulla_sql_lang::sql::typed_result::matview_result_schema;
use medulla_table_store::tables::{
    create_incremental_matview_txn, MatViewSpec, TableStore, TableTxn, TxnOp,
};
use medulla_tables::index::SecondaryIndexColumn;
use medulla_tables::schema::{Column, ColumnType, TableSchema};
use serde_json::{json, Value};

#[path = "../tests/common/commit_batch.rs"]
mod commit_batch;

/// Rows per change, and the budgets.
const CHANGE_ROWS: i64 = 1_000;
const ROUNDS: usize = 30;
const ADDED_COMMIT_BUDGET_MS: f64 = 50.0;
const RESIDENT_GROWTH_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

fn table_rows() -> i64 {
    std::env::var("MEDULLA_BENCH_ROWS")
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .filter(|rows| (10_000..=10_000_000).contains(rows))
        .unwrap_or(1_000_000)
}

fn commit(store: &TableStore, ops: Vec<TxnOp>, n: &mut u64) {
    let mut txn = TableTxn::new();
    for op in ops {
        txn.push(op);
    }
    *n += 1;
    store
        .commit_txn_batch(
            &txn,
            &commit_batch::batch(store, &format!("mvb-{n}"), &format!("idem-mvb-{n}")),
            *n,
        )
        .unwrap_or_else(|error| panic!("commit {n}: {error}"));
}

/// The resident set of this process in bytes (`/proc/self/statm`; 0 where unavailable).
fn resident_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|text| text.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map_or(0, |pages| pages * 4096)
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

fn create_view(store: &TableStore, select_sql: &str, n: &mut u64) {
    let program = compile_incremental_sql(
        "mv",
        select_sql,
        &column_type(store),
        &|table: &str, column: &str| {
            medulla_table_store::tables::join_key_indexed(store, table, column)
        },
    )
    .unwrap_or_else(|refusal| panic!("{select_sql}: {refusal}"));
    let view = GraphCore::new().analysis_snapshot();
    let typed = exec_sql_typed_with_tables(&view, store, select_sql).unwrap();
    let visible = matview_result_schema("mv", &typed.columns);
    let inputs: Vec<String> = [program.table.clone()]
        .into_iter()
        .chain(program.join.iter().map(|join| join.right.clone()))
        .collect();
    let spec = MatViewSpec {
        name: "mv".to_string(),
        select_sql: select_sql.to_string(),
        inputs,
        owner: "bench".to_string(),
        refreshed_seq: 0,
        stale: false,
        program: None,
    };
    let txn = create_incremental_matview_txn(store, spec, &visible, program).unwrap();
    commit_txn(store, &txn, n);
}

fn commit_txn(store: &TableStore, txn: &TableTxn, n: &mut u64) {
    *n += 1;
    store
        .commit_txn_batch(
            txn,
            &commit_batch::batch(store, &format!("mvb-{n}"), &format!("idem-mvb-{n}")),
            *n,
        )
        .unwrap();
}

fn drop_view(store: &TableStore, n: &mut u64) {
    commit(
        store,
        vec![
            TxnOp::DropMatView { name: "mv".into() },
            TxnOp::DropTable {
                name: "mv".into(),
                if_exists: false,
            },
        ],
        n,
    );
}

fn orders_schema() -> TableSchema {
    TableSchema::new(
        "orders",
        vec![
            Column::new("id", ColumnType::BigInt, false, true),
            Column::new("region", ColumnType::Text, true, false),
            Column::new("qty", ColumnType::BigInt, true, false),
        ],
    )
}

fn sales_schema() -> TableSchema {
    TableSchema::new(
        "sales",
        vec![
            Column::new("sid", ColumnType::BigInt, false, true),
            Column::new("customer_id", ColumnType::BigInt, true, false),
            Column::new("amount", ColumnType::BigInt, true, false),
        ],
    )
}

fn customers_schema() -> TableSchema {
    TableSchema::new(
        "customers",
        vec![
            Column::new("cid", ColumnType::BigInt, false, true),
            Column::new("area", ColumnType::Text, true, false),
        ],
    )
}

fn insert_rows(table: &str, cols: &[&str], rows: Vec<Vec<Value>>) -> TxnOp {
    TxnOp::Insert {
        table: table.into(),
        col_order: cols.iter().map(|c| c.to_string()).collect(),
        rows,
    }
}

fn ids_in(column: &str, ids: impl Iterator<Item = i64>) -> RowPredicate {
    RowPredicate::In {
        col: column.into(),
        values: ids.map(|id| json!(id)).collect(),
    }
}

/// Seed `table` with `rows` rows in chunks of 50,000.
fn seed(store: &TableStore, table: &str, cols: &[&str], rows: i64, n: &mut u64, row: &dyn Fn(i64) -> Vec<Value>) {
    let mut next = 1;
    while next <= rows {
        let upper = (next + 49_999).min(rows);
        let chunk: Vec<Vec<Value>> = (next..=upper).map(row).collect();
        commit(store, vec![insert_rows(table, cols, chunk)], n);
        next = upper + 1;
    }
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    let index = ((sorted.len() as f64 - 1.0) * fraction).ceil() as usize;
    sorted[index.min(sorted.len() - 1)]
}

/// The commit times of `ROUNDS` rounds of one insert, one update and one delete of 1,000 rows
/// each over `table`, in milliseconds, sorted. `fresh` is the first id the inserts use.
fn change_times(
    store: &TableStore,
    table: &str,
    cols: &[&str],
    key: &str,
    set_column: &str,
    fresh: i64,
    n: &mut u64,
    row: &dyn Fn(i64) -> Vec<Value>,
) -> Vec<f64> {
    let mut times = Vec::new();
    let mut next = fresh;
    for round in 0..ROUNDS as i64 {
        let inserted: Vec<Vec<Value>> = (next..next + CHANGE_ROWS).map(row).collect();
        let existing = round * CHANGE_ROWS + 1;
        let ops = [
            insert_rows(table, cols, inserted),
            TxnOp::Update {
                table: table.into(),
                set: serde_json::from_value(json!({ set_column: round % 7 + 2 })).unwrap(),
                selector: ids_in(key, existing..existing + CHANGE_ROWS),
            },
            TxnOp::Delete {
                table: table.into(),
                selector: ids_in(key, next..next + CHANGE_ROWS / 2),
            },
        ];
        for op in ops {
            let started = Instant::now();
            commit(store, vec![op], n);
            times.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        next += CHANGE_ROWS;
    }
    times.sort_by(f64::total_cmp);
    times
}

struct Record {
    group: &'static str,
    rows: i64,
    p50_ms: f64,
    p99_ms: f64,
    added_p99_ms: f64,
    resident_growth_bytes: u64,
}

fn emit(record: &Record) -> bool {
    let breached = record.added_p99_ms > ADDED_COMMIT_BUDGET_MS
        || record.resident_growth_bytes > RESIDENT_GROWTH_BUDGET_BYTES;
    let line = format!(
        "{{\"group\":\"{}\",\"rows\":{},\"p50_ms\":{:.3},\"p99_ms\":{:.3},\"added_p99_ms\":{:.3},\"resident_growth_bytes\":{},\"breached\":{}}}",
        record.group,
        record.rows,
        record.p50_ms,
        record.p99_ms,
        record.added_p99_ms,
        record.resident_growth_bytes,
        breached
    );
    println!("{line}");
    let directory = std::path::Path::new("target/matview_incremental");
    if std::fs::create_dir_all(directory).is_ok() {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join("records.jsonl"))
        {
            let _ = writeln!(file, "{line}");
        }
    }
    breached
}

/// One group over `orders`: the commit times with no view, then with the view `select`.
fn orders_group(
    store: &TableStore,
    group: &'static str,
    select: &str,
    rows: i64,
    n: &mut u64,
    fresh: &mut i64,
) -> bool {
    let regions = ["eu", "us", "apac", "latam"];
    let row = |id: i64| vec![json!(id), json!(regions[(id % 4) as usize]), json!(id % 9)];
    let cols = ["id", "region", "qty"];
    let baseline = change_times(store, "orders", &cols, "id", "qty", *fresh, n, &row);
    *fresh += CHANGE_ROWS * ROUNDS as i64;
    let before = resident_bytes();
    create_view(store, select, n);
    let with_view = change_times(store, "orders", &cols, "id", "qty", *fresh, n, &row);
    *fresh += CHANGE_ROWS * ROUNDS as i64;
    let growth = resident_bytes().saturating_sub(before);
    drop_view(store, n);
    emit(&Record {
        group,
        rows,
        p50_ms: percentile(&with_view, 0.5),
        p99_ms: percentile(&with_view, 0.99),
        added_p99_ms: percentile(&with_view, 0.99) - percentile(&baseline, 0.99),
        resident_growth_bytes: growth,
    })
}

/// The join group: `sales` against an indexed `customers`.
fn join_group(store: &TableStore, rows: i64, n: &mut u64) -> bool {
    let customers = 10_000_i64;
    let sale = move |sid: i64| vec![json!(sid), json!(sid % customers + 1), json!(sid % 9)];
    let cols = ["sid", "customer_id", "amount"];
    let mut fresh = rows + 1;
    let baseline = change_times(store, "sales", &cols, "sid", "amount", fresh, n, &sale);
    fresh += CHANGE_ROWS * ROUNDS as i64;
    let before = resident_bytes();
    create_view(
        store,
        "SELECT s.sid, c.area, s.amount FROM sales s JOIN customers c ON s.customer_id = c.cid",
        n,
    );
    let with_view = change_times(store, "sales", &cols, "sid", "amount", fresh, n, &sale);
    let growth = resident_bytes().saturating_sub(before);
    drop_view(store, n);
    emit(&Record {
        group: "join",
        rows,
        p50_ms: percentile(&with_view, 0.5),
        p99_ms: percentile(&with_view, 0.99),
        added_p99_ms: percentile(&with_view, 0.99) - percentile(&baseline, 0.99),
        resident_growth_bytes: growth,
    })
}

fn main() {
    let rows = table_rows();
    let mut n = 0_u64;
    let (store, _path) = TableStore::open_temp().unwrap();
    commit(
        &store,
        vec![
            TxnOp::CreateTable {
                schema: orders_schema(),
                if_not_exists: false,
            },
            TxnOp::CreateTable {
                schema: customers_schema(),
                if_not_exists: false,
            },
            TxnOp::CreateTable {
                schema: sales_schema(),
                if_not_exists: false,
            },
        ],
        &mut n,
    );
    let regions = ["eu", "us", "apac", "latam"];
    seed(&store, "orders", &["id", "region", "qty"], rows, &mut n, &|id| {
        vec![json!(id), json!(regions[(id % 4) as usize]), json!(id % 9)]
    });
    seed(&store, "customers", &["cid", "area"], 10_000, &mut n, &|id| {
        vec![json!(id), json!(format!("area-{}", id % 50))]
    });
    seed(&store, "sales", &["sid", "customer_id", "amount"], rows, &mut n, &|sid| {
        vec![json!(sid), json!(sid % 10_000 + 1), json!(sid % 9)]
    });
    for (schema, table, column) in [
        (customers_schema(), "customers", "cid"),
        (sales_schema(), "sales", "customer_id"),
    ] {
        let spec = store
            .secondary_index_spec(
                table,
                &format!("{table}_{column}"),
                vec![SecondaryIndexColumn::ascending(column)],
                &schema,
            )
            .unwrap();
        store.create_secondary_index(&spec, false).unwrap();
    }
    let mut fresh = rows + 1;
    let mut breached = false;
    for (group, select) in [
        ("eval", "SELECT id, qty * 2 + 1 AS q FROM orders"),
        ("rows", "SELECT id, region, qty FROM orders WHERE qty > 3"),
        (
            "groups",
            "SELECT region, count(*) AS n, sum(qty) AS total, avg(qty) AS mean FROM orders WHERE qty > 0 GROUP BY region",
        ),
        (
            "extremes",
            "SELECT region, min(qty) AS lo, max(qty) AS hi FROM orders GROUP BY region",
        ),
    ] {
        breached |= orders_group(&store, group, select, rows, &mut n, &mut fresh);
    }
    breached |= join_group(&store, rows, &mut n);
    if breached {
        eprintln!("matview_incremental: a budget was breached");
        std::process::exit(1);
    }
}
