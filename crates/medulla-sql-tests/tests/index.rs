//! Partial and expression indexes on user tables
//! (MEDULLA-ENGINE-PARITY-R010.1). These tests drive the production
//! `TableStore` index catalog and its write-path maintenance.
#![cfg(feature = "sql")]

use medulla_graph_core::graph::{GraphCore, GraphView};
use medulla_result_types::row_predicate::RowPredicate;
use medulla_sql::sql::exec_sql_typed_with_tables;
use medulla_sql_lang::sql::classify::{classify, StatementKind};
use medulla_table_store::tables::{IndexCatalogTxnOp, TableStore, TableTxn, TxnOp};
use medulla_tables::index::{
    IndexExpr, SecondaryIndexColumn, SecondaryIndexLookup, SecondaryIndexSpec,
    SECONDARY_INDEX_SCHEMA_VERSION,
};
use medulla_tables::schema::{Cell, CheckExpr, CmpOp, Column, ColumnType, TableSchema};
use serde_json::json;

fn scoped_store(tag: &str) -> TableStore {
    let path = std::env::temp_dir().join(format!(
        "medulla_index_{tag}_{}_{}.redb",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    TableStore::open_scoped(
        &path,
        "tenant-a",
        medulla_table_store::tables::store::dev_scope_grant::dev_verifier(),
        medulla_table_store::tables::store::dev_scope_grant::DEV_PRINCIPAL,
        medulla_table_store::tables::store::dev_scope_grant::DEV_PROOF,
    )
    .unwrap()
}

fn accounts_schema() -> TableSchema {
    TableSchema::new(
        "accounts",
        vec![
            Column::new("id", ColumnType::BigInt, false, false),
            Column::new("email", ColumnType::Text, false, false),
            Column::new("status", ColumnType::Text, false, false),
        ],
    )
}

fn insert(store: &TableStore, id: i64, email: &str, status: &str) -> Result<usize, String> {
    store.insert_rows(
        "accounts",
        &["id".into(), "email".into(), "status".into()],
        &[vec![id.into(), email.into(), status.into()]],
    )
}

fn active() -> CheckExpr {
    CheckExpr::Cmp {
        column: "status".into(),
        op: CmpOp::Eq,
        value: json!("active"),
    }
}

// spec: MEDULLA-ENGINE-PARITY-R010.1
#[test]
fn partial_index_holds_only_matching_rows() {
    let store = scoped_store("partial");
    let schema = accounts_schema();
    store.create_table(&schema, false).unwrap();
    insert(&store, 1, "a@x", "active").unwrap();
    insert(&store, 2, "b@x", "closed").unwrap();
    let spec = store
        .secondary_index_spec(
            "accounts",
            "accounts_active_email",
            vec![SecondaryIndexColumn::ascending("email")],
            &schema,
        )
        .unwrap()
        .with_predicate(active(), &schema)
        .unwrap();
    // Built over existing rows: only the matching one is indexed.
    assert!(store.create_secondary_index(&spec, false).unwrap());
    let covered = |s: &TableStore| {
        s.secondary_index_covered_rows("accounts", "accounts_active_email")
            .unwrap()
            .unwrap()
    };
    assert_eq!(covered(&store).len(), 1);
    // Later writes maintain it in the same transaction as the row.
    insert(&store, 3, "c@x", "active").unwrap();
    insert(&store, 4, "d@x", "closed").unwrap();
    let rows = covered(&store);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r[2] == Cell::Text("active".into())));
    // A plain lookup never trusts a partial index: it returns `None` (scan).
    let lookup = SecondaryIndexLookup::Eq {
        column: "email".into(),
        value: Cell::Text("a@x".into()),
    };
    assert_eq!(
        store.secondary_index_rows("accounts", &lookup).unwrap(),
        None
    );
    // Leaving the predicate removes the entry; entering it adds one.
    store
        .update_where(
            "accounts",
            json!({"status": "closed"}).as_object().unwrap(),
            &RowPredicate::IsNotNull { col: "id".into() },
        )
        .unwrap();
    assert_eq!(covered(&store).len(), 0);
}

// spec: MEDULLA-ENGINE-PARITY-R010.1
#[test]
fn unique_partial_index_is_unique_among_matching_rows() {
    let store = scoped_store("unique");
    let schema = accounts_schema();
    store.create_table(&schema, false).unwrap();
    let spec = store
        .secondary_index_spec(
            "accounts",
            "accounts_active_email_uq",
            vec![SecondaryIndexColumn::ascending("email")],
            &schema,
        )
        .unwrap()
        .with_predicate(active(), &schema)
        .unwrap()
        .into_unique();
    store.create_secondary_index(&spec, false).unwrap();
    insert(&store, 1, "dup@x", "active").unwrap();
    // The same email outside the predicate is not covered, so it is allowed.
    insert(&store, 2, "dup@x", "closed").unwrap();
    // A second covered row with that email is refused.
    let refused = insert(&store, 3, "dup@x", "active").unwrap_err();
    assert!(refused.starts_with("UNIQUE_VIOLATION"), "{refused}");
}

// spec: MEDULLA-ENGINE-PARITY-R010.1
#[test]
fn expression_index_keys_the_expression_result() {
    let store = scoped_store("expr");
    let schema = accounts_schema();
    store.create_table(&schema, false).unwrap();
    insert(&store, 1, "Ann@X", "active").unwrap();
    insert(&store, 2, "bob@x", "active").unwrap();
    let spec = store
        .secondary_index_spec(
            "accounts",
            "accounts_lower_email",
            vec![SecondaryIndexColumn::expression("email", IndexExpr::Lower)],
            &schema,
        )
        .unwrap()
        .into_unique();
    store.create_secondary_index(&spec, false).unwrap();
    // Equal after lower(): refused although the raw values differ.
    let refused = insert(&store, 3, "ANN@x", "active").unwrap_err();
    assert!(refused.starts_with("UNIQUE_VIOLATION"), "{refused}");
    // A plain lookup on the raw column cannot use an expression index.
    let lookup = SecondaryIndexLookup::Eq {
        column: "email".into(),
        value: Cell::Text("ann@x".into()),
    };
    assert_eq!(
        store.secondary_index_rows("accounts", &lookup).unwrap(),
        None
    );
}

// spec: MEDULLA-ENGINE-PARITY-R010.1
#[test]
fn expressions_outside_the_closed_set_are_refused() {
    for name in ["now", "random", "nextval", "md5"] {
        let error = IndexExpr::parse(name).unwrap_err();
        assert!(error.starts_with("INDEX_EXPR_NONDETERMINISTIC"), "{error}");
    }
    for (name, expr) in [
        ("LOWER", IndexExpr::Lower),
        ("upper", IndexExpr::Upper),
        ("abs", IndexExpr::Abs),
        ("Length", IndexExpr::Length),
    ] {
        assert_eq!(IndexExpr::parse(name).unwrap(), expr);
    }
    // An expression applied to a column type it does not accept is refused too.
    let store = scoped_store("exprtype");
    let schema = accounts_schema();
    store.create_table(&schema, false).unwrap();
    let error = store
        .secondary_index_spec(
            "accounts",
            "bad",
            vec![SecondaryIndexColumn::expression("id", IndexExpr::Lower)],
            &schema,
        )
        .unwrap_err();
    assert!(error.starts_with("INDEX_EXPR_NONDETERMINISTIC"), "{error}");
}

// spec: MEDULLA-ENGINE-PARITY-R010.1
#[test]
fn version_one_specs_are_not_read_without_rebuild() {
    assert_eq!(SECONDARY_INDEX_SCHEMA_VERSION, 2);
    let store = scoped_store("v1");
    let schema = accounts_schema();
    store.create_table(&schema, false).unwrap();
    let mut old: SecondaryIndexSpec = store
        .secondary_index_spec(
            "accounts",
            "accounts_v1",
            vec![SecondaryIndexColumn::ascending("email")],
            &schema,
        )
        .unwrap();
    old.schema_version = 1;
    let error = store.create_secondary_index(&old, false).unwrap_err();
    assert!(error.contains("unsupported schema version 1"), "{error}");
}

/// Drive one `CREATE INDEX` / `DROP INDEX` statement through the classifier and the
/// store's catalog transaction, the same two steps the served path takes.
fn apply_index_sql(store: &TableStore, sql: &str) -> Result<usize, String> {
    let mut txn = TableTxn::new();
    match classify(sql)? {
        StatementKind::CreateIndex(plan) => {
            txn.push(TxnOp::IndexCatalog(IndexCatalogTxnOp::PutSecondaryIndex {
                plan,
            }));
        }
        StatementKind::DropIndex { name, .. } => {
            let table = store
                .secondary_index_table(&name)?
                .ok_or_else(|| format!("index `{name}` does not exist"))?;
            txn.push(TxnOp::IndexCatalog(IndexCatalogTxnOp::DropIndex {
                table,
                name,
            }));
        }
        other => return Err(format!("not an index statement: {other:?}")),
    }
    store.commit_txn(&txn)
}

fn empty_view() -> GraphView {
    GraphCore::new().analysis_snapshot()
}

fn plan_text(store: &TableStore, sql: &str) -> String {
    let result = exec_sql_typed_with_tables(&empty_view(), store, &format!("EXPLAIN {sql}"))
        .expect("explain");
    result
        .rows
        .iter()
        .flat_map(|row| {
            row.iter()
                .map(|cell| cell.as_str().unwrap_or_default().to_string())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn ids(store: &TableStore, sql: &str) -> Vec<i64> {
    let result = exec_sql_typed_with_tables(&empty_view(), store, sql).expect("select");
    let mut out: Vec<i64> = result
        .rows
        .iter()
        .map(|row| row[0].as_i64().expect("integer id"))
        .collect();
    out.sort_unstable();
    out
}

// spec: MEDULLA-ENGINE-PARITY-R010.2
#[test]
fn planner_uses_partial_index() {
    let store = scoped_store("planner");
    store.create_table(&accounts_schema(), false).unwrap();
    insert(&store, 1, "a@x", "active").unwrap();
    insert(&store, 2, "b@x", "closed").unwrap();
    insert(&store, 3, "c@x", "active").unwrap();
    apply_index_sql(
        &store,
        "CREATE UNIQUE INDEX accounts_active_email ON accounts (email) WHERE status = 'active'",
    )
    .unwrap();
    insert(&store, 4, "d@x", "active").unwrap();
    insert(&store, 5, "e@x", "closed").unwrap();

    // The query's WHERE carries the index predicate as one conjunct: the index serves
    // it, EXPLAIN names it, and the answer equals what a scan returns.
    let implied = "SELECT id FROM accounts WHERE status = 'active' AND id > 1";
    let text = plan_text(&store, implied);
    assert!(
        text.contains("IndexScanExec: index=accounts_active_email"),
        "{text}"
    );
    assert_eq!(ids(&store, implied), vec![3, 4]);
    assert_eq!(
        ids(&store, "SELECT id FROM accounts WHERE status = 'active'"),
        vec![1, 3, 4]
    );

    // A query that does not imply the predicate falls back to the scan: closed rows
    // are still found, and no index is named.
    let other = "SELECT id FROM accounts WHERE status = 'closed'";
    assert!(!plan_text(&store, other).contains("IndexScanExec"));
    assert_eq!(ids(&store, other), vec![2, 5]);
    let unfiltered = "SELECT id FROM accounts WHERE id > 1";
    assert!(!plan_text(&store, unfiltered).contains("IndexScanExec"));
    assert_eq!(ids(&store, unfiltered), vec![2, 3, 4, 5]);

    // Dropping the index returns every query to the scan.
    apply_index_sql(&store, "DROP INDEX accounts_active_email").unwrap();
    assert!(!plan_text(&store, implied).contains("IndexScanExec"));
    assert_eq!(ids(&store, implied), vec![3, 4]);
}

// spec: MEDULLA-ENGINE-PARITY-R010.2
#[test]
fn create_index_decodes_columns_expressions_and_predicate() {
    let StatementKind::CreateIndex(plan) = classify(
        "CREATE UNIQUE INDEX IF NOT EXISTS by_mail ON accounts (lower(email), id DESC) WHERE status = 'active' AND id > 0",
    )
    .unwrap() else {
        panic!("expected CreateIndex");
    };
    assert!(plan.unique && plan.if_not_exists);
    assert_eq!(plan.name.as_deref(), Some("by_mail"));
    assert_eq!(plan.table, "accounts");
    assert_eq!(plan.columns.len(), 2);
    assert_eq!(plan.columns[0].column, "email");
    assert_eq!(plan.columns[0].expr, Some(IndexExpr::Lower));
    assert!(plan.columns[1].descending && plan.columns[1].expr.is_none());
    assert!(matches!(plan.predicate, Some(CheckExpr::And(_, _))));

    let StatementKind::CreateIndex(unnamed) = classify("CREATE INDEX ON accounts (email)").unwrap()
    else {
        panic!("expected CreateIndex");
    };
    assert_eq!(unnamed.resolved_name(), "accounts_email_idx");
    // An ANN index is still the ANN registration, not a scalar index.
    assert!(matches!(
        classify("CREATE INDEX ON docs USING hnsw (emb vector_l2_ops)").unwrap(),
        StatementKind::CreateAnnIndex(_)
    ));
}

// spec: MEDULLA-ENGINE-PARITY-R010.2
#[test]
fn nondeterministic_expression_refuses() {
    for sql in [
        "CREATE INDEX i ON accounts (now())",
        "CREATE INDEX i ON accounts (random())",
        "CREATE INDEX i ON accounts (nextval('s'))",
        "CREATE INDEX i ON accounts ((id + 1))",
        "CREATE INDEX i ON accounts (substr(email, 1, 2))",
    ] {
        let error = classify(sql).unwrap_err();
        assert!(
            error.starts_with("INDEX_EXPR_NONDETERMINISTIC"),
            "{sql}: {error}"
        );
    }
    // An allowed function on a column type it does not accept is refused when the
    // index is created, with the same code.
    let store = scoped_store("nondeterministic");
    store.create_table(&accounts_schema(), false).unwrap();
    let error = apply_index_sql(&store, "CREATE INDEX bad ON accounts (abs(email))").unwrap_err();
    assert!(error.contains("INDEX_EXPR_NONDETERMINISTIC"), "{error}");
    assert!(store.list_secondary_indexes(None).unwrap().is_empty());
}
