//! Constraints with deferred checking and prefixed violation codes
//! (MEDULLA-ENGINE-PARITY-R011.1), driven through the production `TableStore`.
#![cfg(feature = "sql")]

use std::path::{Path, PathBuf};

use medulla_sql_lang::sql::classify::{classify, StatementKind};
use medulla_table_store::tables::{TableStore, TableTxn, TxnOp};
use medulla_tables::schema::{Column, ColumnType, RefAction, TableConstraint, TableSchema};
use serde_json::json;

/// The pending bound the composition root installs from the registry knob
/// `tables.deferred_fk_pending_max`.
const PENDING_MAX: usize = 8;

fn open(path: &Path) -> TableStore {
    let store = TableStore::open_scoped(
        path,
        "tenant-a",
        medulla_table_store::tables::store::dev_scope_grant::dev_verifier(),
        medulla_table_store::tables::store::dev_scope_grant::DEV_PRINCIPAL,
        medulla_table_store::tables::store::dev_scope_grant::DEV_PROOF,
    )
    .unwrap();
    store.install_deferred_fk_pending_max(PENDING_MAX);
    store
}

fn fresh_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "medulla_constraint_{tag}_{}_{}.redb",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn parent_and_child(store: &TableStore, deferrable: bool, initially_deferred: bool) {
    store
        .create_table(
            &TableSchema::new(
                "parents",
                vec![Column::new("id", ColumnType::BigInt, false, true)],
            ),
            false,
        )
        .unwrap();
    let fk = TableConstraint::ForeignKey {
        name: Some("children_parent_fk".to_string()),
        columns: vec!["parent_id".to_string()],
        ref_table: "parents".to_string(),
        ref_columns: vec!["id".to_string()],
        on_delete: RefAction::NoAction,
        on_update: RefAction::NoAction,
        deferrable,
        initially_deferred,
    };
    store
        .create_table(
            &TableSchema::new(
                "children",
                vec![
                    Column::new("id", ColumnType::BigInt, false, true),
                    Column::new("parent_id", ColumnType::BigInt, true, false),
                ],
            )
            .with_constraints(vec![fk]),
            false,
        )
        .unwrap();
}

fn insert(table: &str, cols: &[&str], row: Vec<serde_json::Value>) -> TxnOp {
    TxnOp::Insert {
        table: table.to_string(),
        col_order: cols.iter().map(|c| c.to_string()).collect(),
        rows: vec![row],
    }
}

// spec: MEDULLA-ENGINE-PARITY-R011.1
#[test]
fn deferred_foreign_key_checks_at_commit() {
    let path = fresh_path("deferred");
    let store = open(&path);
    parent_and_child(&store, true, true);

    // The child comes before its parent in one transaction: resolved at the end.
    let mut txn = TableTxn::new();
    txn.push(insert(
        "children",
        &["id", "parent_id"],
        vec![json!(1), json!(10)],
    ));
    txn.push(insert("parents", &["id"], vec![json!(10)]));
    store.commit_txn(&txn).unwrap();
    assert_eq!(store.scan("children").unwrap().len(), 1);

    // A reference still unresolved after the last op refuses, names the
    // constraint, and writes nothing.
    let mut txn = TableTxn::new();
    txn.push(insert(
        "children",
        &["id", "parent_id"],
        vec![json!(2), json!(99)],
    ));
    let error = store.commit_txn(&txn).unwrap_err();
    assert!(error.starts_with("FOREIGN_KEY_VIOLATION: "), "{error}");
    assert!(error.contains("children_parent_fk"), "{error}");
    assert_eq!(store.scan("children").unwrap().len(), 1);

    // Deleting a referenced parent is refused at the end unless the child moves.
    let mut txn = TableTxn::new();
    txn.push(TxnOp::Delete {
        table: "parents".to_string(),
        selector: medulla_result_types::row_predicate::RowPredicate::Cmp {
            col: "id".to_string(),
            op: medulla_result_types::row_predicate::CmpOp::Eq,
            value: json!(10),
        },
    });
    let error = store.commit_txn(&txn).unwrap_err();
    assert!(error.starts_with("FOREIGN_KEY_VIOLATION: "), "{error}");
    assert_eq!(store.scan("parents").unwrap().len(), 1);

    // The one-shot write paths are single-statement transactions: they check too.
    let error = store
        .insert_rows(
            "children",
            &["id".to_string(), "parent_id".to_string()],
            &[vec![json!(3), json!(77)]],
        )
        .unwrap_err();
    assert!(error.starts_with("FOREIGN_KEY_VIOLATION: "), "{error}");
    let _ = std::fs::remove_file(&path);
}

// spec: MEDULLA-ENGINE-PARITY-R011.1
#[test]
fn immediate_foreign_key_checks_per_statement() {
    let path = fresh_path("immediate");
    let store = open(&path);
    parent_and_child(&store, false, false);
    let mut txn = TableTxn::new();
    txn.push(insert(
        "children",
        &["id", "parent_id"],
        vec![json!(1), json!(10)],
    ));
    txn.push(insert("parents", &["id"], vec![json!(10)]));
    let error = store.commit_txn(&txn).unwrap_err();
    assert!(error.starts_with("FOREIGN_KEY_VIOLATION: "), "{error}");
    assert!(store.scan("children").unwrap().is_empty());
    // DEFERRABLE INITIALLY IMMEDIATE still checks per statement.
    let path2 = fresh_path("immediate2");
    let store2 = open(&path2);
    parent_and_child(&store2, true, false);
    let mut txn = TableTxn::new();
    txn.push(insert(
        "children",
        &["id", "parent_id"],
        vec![json!(1), json!(10)],
    ));
    txn.push(insert("parents", &["id"], vec![json!(10)]));
    assert!(store2.commit_txn(&txn).is_err());
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&path2);
}

// spec: MEDULLA-ENGINE-PARITY-R011.1
#[test]
fn violations_carry_the_four_codes() {
    let path = fresh_path("codes");
    let store = open(&path);
    let mut checked = Column::new("n", ColumnType::BigInt, true, false);
    checked.check = Some(medulla_tables::schema::ColCheck {
        op: medulla_tables::schema::CmpOp::Gt,
        value: json!(0),
    });
    store
        .create_table(
            &TableSchema::new(
                "t",
                vec![
                    Column::new("id", ColumnType::BigInt, false, true),
                    Column::new("name", ColumnType::Text, false, false),
                    checked,
                ],
            ),
            false,
        )
        .unwrap();
    let cols: Vec<String> = ["id", "name", "n"].iter().map(|c| c.to_string()).collect();
    store
        .insert_rows("t", &cols, &[vec![json!(1), json!("a"), json!(5)]])
        .unwrap();
    let unique = store
        .insert_rows("t", &cols, &[vec![json!(1), json!("b"), json!(5)]])
        .unwrap_err();
    assert!(unique.starts_with("UNIQUE_VIOLATION: "), "{unique}");
    let not_null = store
        .insert_rows("t", &cols, &[vec![json!(2), json!(null), json!(5)]])
        .unwrap_err();
    assert!(not_null.starts_with("NOT_NULL_VIOLATION: "), "{not_null}");
    assert!(not_null.contains("`name`"), "{not_null}");
    let check = store
        .insert_rows("t", &cols, &[vec![json!(3), json!("c"), json!(-1)]])
        .unwrap_err();
    assert!(check.starts_with("CHECK_VIOLATION: "), "{check}");
    let _ = std::fs::remove_file(&path);
}

// spec: MEDULLA-ENGINE-PARITY-R011.1
#[test]
fn classify_parses_deferrable_initially_deferred() {
    let kind = classify(
        "CREATE TABLE c (id BIGINT PRIMARY KEY, p BIGINT, \
         CONSTRAINT c_p_fk FOREIGN KEY (p) REFERENCES parents(id) DEFERRABLE INITIALLY DEFERRED)",
    )
    .unwrap();
    let StatementKind::CreateTable(plan) = kind else {
        panic!("expected CREATE TABLE");
    };
    assert!(plan.constraints.iter().any(|c| matches!(
        c,
        TableConstraint::ForeignKey {
            deferrable: true,
            initially_deferred: true,
            ..
        }
    )));
    assert!(classify(
        "CREATE TABLE d (id BIGINT PRIMARY KEY, p BIGINT, \
         FOREIGN KEY (p) REFERENCES parents(id) NOT DEFERRABLE INITIALLY DEFERRED)"
    )
    .is_err());
}

// spec: MEDULLA-ENGINE-PARITY-R011.1
#[test]
fn deferred_check_touches_only_the_keys_the_transaction_wrote() {
    let path = fresh_path("probe");
    let store = open(&path);
    parent_and_child(&store, true, true);
    let cols: Vec<String> = ["id".to_string(), "parent_id".to_string()].to_vec();
    // A large child table: every row already resolved, written in batches the
    // pending bound allows.
    store
        .insert_rows("parents", &["id".to_string()], &[vec![json!(1)]])
        .unwrap();
    for batch in 0..20 {
        let rows: Vec<Vec<serde_json::Value>> = (0..PENDING_MAX)
            .map(|i| vec![json!(batch * 100 + i as i64 + 10), json!(1)])
            .collect();
        store.insert_rows("children", &cols, &rows).unwrap();
    }
    let before = store.deferred_fk_keys_checked();

    // One new child row of the large table: one key checked, not the table.
    let mut txn = TableTxn::new();
    txn.push(insert(
        "children",
        &["id", "parent_id"],
        vec![json!(5000), json!(1)],
    ));
    store.commit_txn(&txn).unwrap();
    assert_eq!(store.deferred_fk_keys_checked() - before, 1);

    // Updating a child row that does not touch its key checks nothing.
    let before = store.deferred_fk_keys_checked();
    let mut txn = TableTxn::new();
    txn.push(TxnOp::Update {
        table: "children".to_string(),
        set: serde_json::from_value(json!({"id": 5001})).unwrap(),
        selector: medulla_result_types::row_predicate::RowPredicate::Cmp {
            col: "id".to_string(),
            op: medulla_result_types::row_predicate::CmpOp::Eq,
            value: json!(5000),
        },
    });
    store.commit_txn(&txn).unwrap();
    assert_eq!(store.deferred_fk_keys_checked() - before, 0);

    // Deleting one referenced parent key checks that one key; a child row that
    // still references it refuses.
    let before = store.deferred_fk_keys_checked();
    let mut txn = TableTxn::new();
    txn.push(TxnOp::Delete {
        table: "parents".to_string(),
        selector: medulla_result_types::row_predicate::RowPredicate::Cmp {
            col: "id".to_string(),
            op: medulla_result_types::row_predicate::CmpOp::Eq,
            value: json!(1),
        },
    });
    let error = store.commit_txn(&txn).unwrap_err();
    assert!(error.starts_with("FOREIGN_KEY_VIOLATION: "), "{error}");
    assert_eq!(store.deferred_fk_keys_checked() - before, 1);
    let _ = std::fs::remove_file(&path);
}

// spec: MEDULLA-ENGINE-PARITY-R011.1
#[test]
fn deferred_pending_set_is_bounded_by_the_knob() {
    let path = fresh_path("bound");
    let store = open(&path);
    parent_and_child(&store, true, true);
    let parents: Vec<Vec<serde_json::Value>> =
        (0..=PENDING_MAX as i64).map(|i| vec![json!(i)]).collect();
    store
        .insert_rows("parents", &["id".to_string()], &parents)
        .unwrap();
    let cols: Vec<String> = ["id".to_string(), "parent_id".to_string()].to_vec();
    // One distinct parent key per child row: one more than the bound.
    let rows: Vec<Vec<serde_json::Value>> = (0..=PENDING_MAX as i64)
        .map(|i| vec![json!(i + 100), json!(i)])
        .collect();
    let error = store.insert_rows("children", &cols, &rows).unwrap_err();
    assert!(
        error.starts_with("DEFERRED_FK_PENDING_EXCEEDED: "),
        "{error}"
    );
    assert!(store.scan("children").unwrap().is_empty());
    let _ = std::fs::remove_file(&path);
}

// spec: MEDULLA-ENGINE-PARITY-R011.1
#[test]
fn ephemeral_store_without_an_installed_bound_commits_a_deferred_foreign_key() {
    // An ephemeral store (no composition root installed the knob value) uses the
    // registry default, never zero, so a deferred-key transaction commits.
    let (store, path) = TableStore::open_temp().unwrap();
    parent_and_child(&store, true, true);
    let mut txn = TableTxn::new();
    txn.push(insert(
        "children",
        &["id", "parent_id"],
        vec![json!(1), json!(10)],
    ));
    txn.push(insert("parents", &["id"], vec![json!(10)]));
    store.commit_txn(&txn).unwrap();
    assert_eq!(store.scan("children").unwrap().len(), 1);
    let _ = std::fs::remove_file(&path);
}
