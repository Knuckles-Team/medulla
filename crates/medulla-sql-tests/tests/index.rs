//! Partial and expression indexes on user tables
//! (MEDULLA-ENGINE-PARITY-R010.1). These tests drive the production
//! `TableStore` index catalog and its write-path maintenance.
#![cfg(feature = "sql")]

use medulla_result_types::row_predicate::RowPredicate;
use medulla_table_store::tables::TableStore;
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
