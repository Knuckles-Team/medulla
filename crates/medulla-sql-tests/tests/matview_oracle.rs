//! The differential oracle of SQL views (MEDULLA-INCREMENTAL-VIEWS-R009.6, R012.4, R013.4).
//!
//! A property test drives random inserts, updates (including key changes) and deletes against
//! the table store the wire commits to, with NULLs and duplicate non-key values among the rows.
//! After every commit the view rows must equal the defining `SELECT` as an unordered multiset,
//! checked by `assert_incremental_equals_recompute` (R003). Each shape is one case of
//! [`ORACLE_CASES`], and the last test refuses a shape whose named test does not exist.
//!
//! The file lives in `medulla-sql-tests` rather than the table store's own `tests/`: the oracle
//! needs the SQL compiler and executor, which the table store cannot depend on.
#![cfg(feature = "sql")]

use std::collections::BTreeMap;

use medulla_exec::incremental::oracle::{assert_incremental_equals_recompute, OracleSubject};
use medulla_graph_core::graph::GraphCore;
use medulla_result_types::row_predicate::{CmpOp, RowPredicate};
use medulla_sql::sql::exec_sql_typed_with_tables;
use medulla_sql_lang::sql::matview_compile::compile_incremental_sql;
use medulla_sql_lang::sql::typed_result::matview_result_schema;
use medulla_table_store::tables::{
    create_incremental_matview_txn, MatViewSpec, TableStore, TableTxn, TxnOp,
};
use medulla_tables::index::SecondaryIndexColumn;
use medulla_tables::schema::{Column, ColumnType, TableSchema};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "common/commit_batch.rs"]
mod commit_batch;

/// `(shape, the test that proves it)`: every shape an oracle case covers.
const ORACLE_CASES: &[(&str, &str)] = &[
    ("filter only", "supported_shapes_agree_after_every_commit"),
    (
        "projection with arithmetic",
        "supported_shapes_agree_after_every_commit",
    ),
    (
        "grouped count and sum",
        "supported_shapes_agree_after_every_commit",
    ),
    ("grouped avg", "supported_shapes_agree_after_every_commit"),
    ("distinct", "extremes_and_distinct_agree_after_every_commit"),
    (
        "grouped min",
        "extremes_and_distinct_agree_after_every_commit",
    ),
    (
        "grouped max",
        "extremes_and_distinct_agree_after_every_commit",
    ),
    (
        "inner equi-join with filter",
        "join_agrees_after_every_commit",
    ),
];

/// The cases one shape runs, and the steps of each. `MEDULLA_ORACLE_CASES` raises the count (the
/// served default keeps the whole file well inside a minute on a shared runner).
fn case_count() -> u64 {
    std::env::var("MEDULLA_ORACLE_CASES")
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .filter(|count| (1..=100_000).contains(count))
        .unwrap_or(32)
}

const STEPS_PER_CASE: usize = 20;

static COMMITS: AtomicU64 = AtomicU64::new(0);

/// A xorshift generator: the cases are reproducible from their seed alone.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

fn commit(store: &TableStore, ops: Vec<TxnOp>) {
    let mut txn = TableTxn::new();
    for op in ops {
        txn.push(op);
    }
    let n = COMMITS.fetch_add(1, Ordering::SeqCst) + 1;
    store
        .commit_txn_batch(
            &txn,
            &commit_batch::batch(store, &format!("oracle-{n}"), &format!("idem-oracle-{n}")),
            n,
        )
        .unwrap_or_else(|error| panic!("commit {n} failed: {error}"));
}

fn eq(column: &str, value: Value) -> RowPredicate {
    RowPredicate::Cmp {
        col: column.into(),
        op: CmpOp::Eq,
        value,
    }
}

fn column_type(store: &TableStore) -> impl Fn(&str, &str) -> Option<ColumnType> + '_ {
    move |table, column| {
        store
            .get_schema(table)
            .ok()
            .flatten()
            .and_then(|schema| schema.column(column).map(|c| c.ty))
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
        owner: "oracle".to_string(),
        refreshed_seq: 0,
        stale: false,
        program: None,
    };
    let txn = create_incremental_matview_txn(store, spec, &visible, program).unwrap();
    commit_txn(store, &txn);
}

fn commit_txn(store: &TableStore, txn: &TableTxn) {
    let n = COMMITS.fetch_add(1, Ordering::SeqCst) + 1;
    store
        .commit_txn_batch(
            txn,
            &commit_batch::batch(store, &format!("oracle-{n}"), &format!("idem-oracle-{n}")),
            n,
        )
        .unwrap();
}

/// The rows of `sql` as a multiset: row text to how many times it occurs.
fn multiset(store: &TableStore, sql: &str) -> Vec<(String, i64)> {
    let view = GraphCore::new().analysis_snapshot();
    let rows = exec_sql_typed_with_tables(&view, store, sql).unwrap().rows;
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for row in rows {
        let text = row
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("|");
        *counts.entry(text).or_default() += 1;
    }
    counts.into_iter().collect()
}

// -- one table -------------------------------------------------------------------------------

/// One change of the `orders` table.
#[derive(Clone, Debug)]
enum Step {
    Insert { id: i64, region: Value, qty: Value },
    Update { id: i64, set: Value },
    Delete { id: i64 },
}

fn random_region(rng: &mut Rng) -> Value {
    rng.pick(&[json!("eu"), json!("us"), json!("apac"), Value::Null])
        .clone()
}

fn random_qty(rng: &mut Rng) -> Value {
    rng.pick(&[
        Value::Null,
        json!(0),
        json!(1),
        json!(2),
        json!(2),
        json!(5),
        json!(8),
    ])
    .clone()
}

/// A random run over `orders`: inserts of fresh keys, updates of any column including the key,
/// and deletes, against ids that may or may not still exist.
fn order_steps(seed: u64) -> Vec<Step> {
    let mut rng = Rng::new(seed);
    let mut next_id = 1_i64;
    let mut steps = Vec::new();
    for _ in 0..STEPS_PER_CASE {
        let existing = rng.below(next_id.max(1) as u64) as i64 + 1;
        steps.push(match rng.below(5) {
            0 | 1 => {
                let id = next_id;
                next_id += 1;
                Step::Insert {
                    id,
                    region: random_region(&mut rng),
                    qty: random_qty(&mut rng),
                }
            }
            2 => Step::Update {
                id: existing,
                set: json!({"qty": random_qty(&mut rng)}),
            },
            3 => {
                // A key change to a fresh id, so the primary key never collides.
                let fresh = next_id;
                next_id += 1;
                let set = if rng.below(2) == 0 {
                    json!({"id": fresh})
                } else {
                    json!({"region": random_region(&mut rng)})
                };
                Step::Update { id: existing, set }
            }
            _ => Step::Delete { id: existing },
        });
    }
    steps
}

struct ViewSubject {
    store: TableStore,
    select: String,
    columns: String,
}

impl ViewSubject {
    fn new(select: &str, columns: &str) -> Self {
        let (store, path) = TableStore::open_temp().unwrap();
        // The temp path stays alive with the store for the case; it is removed with the process.
        std::mem::forget(path);
        commit(
            &store,
            vec![TxnOp::CreateTable {
                schema: TableSchema::new(
                    "orders",
                    vec![
                        Column::new("id", ColumnType::BigInt, false, true),
                        Column::new("region", ColumnType::Text, true, false),
                        Column::new("qty", ColumnType::BigInt, true, false),
                    ],
                ),
                if_not_exists: false,
            }],
        );
        create_view(&store, select);
        Self {
            store,
            select: select.to_string(),
            columns: columns.to_string(),
        }
    }
}

impl OracleSubject for ViewSubject {
    type Step = Step;

    fn apply_step(&mut self, step: &Step) {
        let op = match step {
            Step::Insert { id, region, qty } => TxnOp::Insert {
                table: "orders".into(),
                col_order: vec!["id".into(), "region".into(), "qty".into()],
                rows: vec![vec![json!(id), region.clone(), qty.clone()]],
            },
            Step::Update { id, set } => TxnOp::Update {
                table: "orders".into(),
                set: serde_json::from_value(set.clone()).unwrap(),
                selector: eq("id", json!(id)),
            },
            Step::Delete { id } => TxnOp::Delete {
                table: "orders".into(),
                selector: eq("id", json!(id)),
            },
        };
        commit(&self.store, vec![op]);
    }

    fn maintained(&self) -> Vec<(String, i64)> {
        multiset(&self.store, &format!("SELECT {} FROM mv", self.columns))
    }

    fn recomputed(&self) -> Vec<(String, i64)> {
        multiset(&self.store, &self.select)
    }
}

/// Run every case of one shape.
fn run_shape(name: &str, select: &str, columns: &str) {
    for case in 0..case_count() {
        let seed = 0x5eed_0000 + case;
        let mut subject = ViewSubject::new(select, columns);
        let steps = order_steps(seed);
        if let Err(mismatch) = assert_incremental_equals_recompute(&mut subject, &steps) {
            panic!("shape `{name}` seed {seed}: {mismatch}\nsteps: {steps:?}");
        }
    }
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R009.6
#[test]
fn supported_shapes_agree_after_every_commit() {
    run_shape(
        "filter only",
        "SELECT id, region, qty FROM orders WHERE qty > 1",
        "id, region, qty",
    );
    run_shape(
        "projection with arithmetic",
        "SELECT id, qty * 2 AS twice FROM orders",
        "id, twice",
    );
    run_shape(
        "grouped count and sum",
        "SELECT region, count(*) AS n, sum(qty) AS total FROM orders GROUP BY region",
        "region, n, total",
    );
    run_shape(
        "grouped avg",
        "SELECT region, avg(qty) AS mean FROM orders GROUP BY region",
        "region, mean",
    );
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R012.4
#[test]
fn extremes_and_distinct_agree_after_every_commit() {
    run_shape("distinct", "SELECT DISTINCT region FROM orders", "region");
    run_shape(
        "grouped min",
        "SELECT region, min(qty) AS lo FROM orders GROUP BY region",
        "region, lo",
    );
    run_shape(
        "grouped max",
        "SELECT region, max(qty) AS hi FROM orders GROUP BY region",
        "region, hi",
    );
}

// -- two tables ------------------------------------------------------------------------------

/// One transaction over `customers` and `sales`; it may change both tables.
#[derive(Clone, Debug)]
struct JoinStep {
    customer: Option<CustomerChange>,
    sale: Option<SaleChange>,
}

#[derive(Clone, Debug)]
enum CustomerChange {
    Insert { cid: i64, area: Value },
    Delete { cid: i64 },
}

#[derive(Clone, Debug)]
enum SaleChange {
    Insert {
        sid: i64,
        customer_id: Value,
        amount: Value,
    },
    Move {
        sid: i64,
        customer_id: Value,
    },
    Delete {
        sid: i64,
    },
}

fn join_steps(seed: u64) -> Vec<JoinStep> {
    let mut rng = Rng::new(seed);
    let (mut next_customer, mut next_sale) = (1_i64, 1_i64);
    let mut steps = Vec::new();
    for _ in 0..STEPS_PER_CASE {
        let customer = match rng.below(4) {
            0 | 1 => {
                let cid = next_customer;
                next_customer += 1;
                Some(CustomerChange::Insert {
                    cid,
                    area: rng
                        .pick(&[json!("north"), json!("south"), Value::Null])
                        .clone(),
                })
            }
            2 => Some(CustomerChange::Delete {
                cid: rng.below(next_customer.max(1) as u64) as i64 + 1,
            }),
            _ => None,
        };
        let target = |rng: &mut Rng| -> Value {
            if rng.below(6) == 0 {
                Value::Null
            } else {
                json!(rng.below(next_customer.max(1) as u64) as i64 + 1)
            }
        };
        let sale = match rng.below(4) {
            0 | 1 => {
                let sid = next_sale;
                next_sale += 1;
                Some(SaleChange::Insert {
                    sid,
                    customer_id: target(&mut rng),
                    amount: random_qty(&mut rng),
                })
            }
            2 => Some(SaleChange::Move {
                sid: rng.below(next_sale.max(1) as u64) as i64 + 1,
                customer_id: target(&mut rng),
            }),
            3 => Some(SaleChange::Delete {
                sid: rng.below(next_sale.max(1) as u64) as i64 + 1,
            }),
            _ => None,
        };
        steps.push(JoinStep { customer, sale });
    }
    steps
}

struct JoinSubject {
    store: TableStore,
}

const JOIN_SELECT: &str = "SELECT s.sid, c.area, s.amount FROM sales s JOIN customers c ON s.customer_id = c.cid WHERE s.amount > 1";

impl JoinSubject {
    fn new() -> Self {
        let (store, path) = TableStore::open_temp().unwrap();
        std::mem::forget(path);
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
        for schema in [&customers, &sales] {
            commit(
                &store,
                vec![TxnOp::CreateTable {
                    schema: schema.clone(),
                    if_not_exists: false,
                }],
            );
        }
        // Both join keys carry a secondary index: the compiler refuses the view otherwise.
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
        create_view(&store, JOIN_SELECT);
        Self { store }
    }
}

impl OracleSubject for JoinSubject {
    type Step = JoinStep;

    fn apply_step(&mut self, step: &JoinStep) {
        let mut ops = Vec::new();
        match &step.customer {
            Some(CustomerChange::Insert { cid, area }) => ops.push(TxnOp::Insert {
                table: "customers".into(),
                col_order: vec!["cid".into(), "area".into()],
                rows: vec![vec![json!(cid), area.clone()]],
            }),
            Some(CustomerChange::Delete { cid }) => ops.push(TxnOp::Delete {
                table: "customers".into(),
                selector: eq("cid", json!(cid)),
            }),
            None => {}
        }
        match &step.sale {
            Some(SaleChange::Insert {
                sid,
                customer_id,
                amount,
            }) => ops.push(TxnOp::Insert {
                table: "sales".into(),
                col_order: vec!["sid".into(), "customer_id".into(), "amount".into()],
                rows: vec![vec![json!(sid), customer_id.clone(), amount.clone()]],
            }),
            Some(SaleChange::Move { sid, customer_id }) => ops.push(TxnOp::Update {
                table: "sales".into(),
                set: serde_json::from_value(json!({"customer_id": customer_id})).unwrap(),
                selector: eq("sid", json!(sid)),
            }),
            Some(SaleChange::Delete { sid }) => ops.push(TxnOp::Delete {
                table: "sales".into(),
                selector: eq("sid", json!(sid)),
            }),
            None => {}
        }
        if !ops.is_empty() {
            commit(&self.store, ops);
        }
    }

    fn maintained(&self) -> Vec<(String, i64)> {
        multiset(&self.store, "SELECT sid, area, amount FROM mv")
    }

    fn recomputed(&self) -> Vec<(String, i64)> {
        multiset(&self.store, JOIN_SELECT)
    }
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R013.4
#[test]
fn join_agrees_after_every_commit() {
    for case in 0..case_count() {
        let seed = 0x7015_0000 + case;
        let mut subject = JoinSubject::new();
        let steps = join_steps(seed);
        if let Err(mismatch) = assert_incremental_equals_recompute(&mut subject, &steps) {
            panic!("shape `inner equi-join with filter` seed {seed}: {mismatch}\nsteps: {steps:?}");
        }
    }
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R004
#[test]
fn every_oracle_shape_names_a_test_in_this_file() {
    let source = std::fs::read_to_string(format!(
        "{}/tests/matview_oracle.rs",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    for (shape, case) in ORACLE_CASES {
        assert!(
            source.contains(&format!("fn {case}()")),
            "shape `{shape}` names {case}, which is not a test of this file"
        );
        assert!(
            source.matches(&format!("\"{shape}\"")).count() >= 2,
            "shape `{shape}` is never run"
        );
    }
}
