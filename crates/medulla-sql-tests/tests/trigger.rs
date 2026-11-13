//! Row-event triggers (MEDULLA-ENGINE-PARITY-R012.1), driven through the
//! production `TableStore` write paths.
#![cfg(feature = "sql")]

use std::path::{Path, PathBuf};

use medulla_result_types::row_predicate::{CmpOp, RowPredicate};
use medulla_statechart::{Action, ActionValue, Guard, State, StatechartDef, Transition};
use medulla_table_store::tables::TableStore;
use medulla_tables::schema::{Cell, Column, ColumnType, TableSchema};
use medulla_tables::trigger::{TriggerEvent, TriggerSpec, TriggerTiming};
use serde_json::{json, Map, Value};

fn open(path: &Path) -> TableStore {
    TableStore::open_scoped(
        path,
        "tenant-a",
        medulla_table_store::tables::store::dev_scope_grant::dev_verifier(),
        medulla_table_store::tables::store::dev_scope_grant::DEV_PRINCIPAL,
        medulla_table_store::tables::store::dev_scope_grant::DEV_PROOF,
    )
    .unwrap()
}

fn fresh_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "medulla_trigger_{tag}_{}_{}.redb",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn tickets(store: &TableStore) {
    store
        .create_table(
            &TableSchema::new(
                "tickets",
                vec![
                    Column::new("id", ColumnType::BigInt, false, true),
                    Column::new("status", ColumnType::Text, true, false),
                ],
            ),
            false,
        )
        .unwrap();
}

/// A one-state chart that reacts to `event` when `guard` holds, running `actions`.
fn chart(event: &str, guard: Option<Guard>, actions: Vec<Action>) -> StatechartDef {
    let mut transition = Transition::new("idle", event, "idle").with_actions(actions);
    if let Some(guard) = guard {
        transition = transition.with_guard(guard);
    }
    StatechartDef {
        name: format!("trigger_{event}"),
        schema_version: 1,
        states: vec![State::new("idle")],
        alphabet: vec![event.to_string()],
        transitions: vec![transition],
        initial: "idle".to_string(),
        finals: Vec::new(),
        meta: Default::default(),
    }
}

fn assign(key: &str, value: Value) -> Action {
    Action::Assign {
        key: key.to_string(),
        value: ActionValue::Const { value },
    }
}

fn refuse(message: &str) -> Action {
    Action::Emit {
        signal: format!("refuse:{message}"),
    }
}

fn trigger(
    name: &str,
    timing: TriggerTiming,
    event: TriggerEvent,
    machine: StatechartDef,
) -> TriggerSpec {
    TriggerSpec::new("tickets", name, timing, vec![event], machine)
}

fn cols() -> Vec<String> {
    vec!["id".to_string(), "status".to_string()]
}

fn id_is(id: i64) -> RowPredicate {
    RowPredicate::Cmp {
        col: "id".to_string(),
        op: CmpOp::Eq,
        value: json!(id),
    }
}

fn set_status(status: &str) -> Map<String, Value> {
    let mut set = Map::new();
    set.insert("status".to_string(), json!(status));
    set
}

fn status_of(store: &TableStore, id: i64) -> Cell {
    store
        .scan("tickets")
        .unwrap()
        .into_iter()
        .find(|row| row[0] == Cell::Int(id))
        .map(|row| row[1].clone())
        .unwrap()
}

// spec: MEDULLA-ENGINE-PARITY-R012.1
#[test]
fn before_insert_rewrites_row() {
    let path = fresh_path("rewrite");
    let store = open(&path);
    tickets(&store);
    store
        .create_trigger(&trigger(
            "default_status",
            TriggerTiming::Before,
            TriggerEvent::Insert,
            chart(
                "before_insert",
                Some(Guard::Missing {
                    key: "new.status".to_string(),
                }),
                vec![assign("new.status", json!("open"))],
            ),
        ))
        .unwrap();
    // The trigger supplies the omitted column.
    store
        .insert_rows("tickets", &["id".to_string()], &[vec![json!(1)]])
        .unwrap();
    assert_eq!(status_of(&store, 1), Cell::Text("open".into()));
    // A supplied value is left alone (the guard is false: the machine is a no-op).
    store
        .insert_rows("tickets", &cols(), &[vec![json!(2), json!("closed")]])
        .unwrap();
    assert_eq!(status_of(&store, 2), Cell::Text("closed".into()));
    // The trigger lives in the table's schema record: it survives a restart.
    drop(store);
    let store = open(&path);
    assert_eq!(
        store
            .get_schema("tickets")
            .unwrap()
            .unwrap()
            .triggers()
            .len(),
        1
    );
    store
        .insert_rows("tickets", &["id".to_string()], &[vec![json!(3)]])
        .unwrap();
    assert_eq!(status_of(&store, 3), Cell::Text("open".into()));
    // Dropping the trigger stops the rewrite.
    store.drop_trigger("tickets", "default_status").unwrap();
    store
        .insert_rows("tickets", &["id".to_string()], &[vec![json!(4)]])
        .unwrap();
    assert_eq!(status_of(&store, 4), Cell::Null);
    let _ = std::fs::remove_file(&path);
}

// spec: MEDULLA-ENGINE-PARITY-R012.1
#[test]
fn refuse_aborts_the_statement() {
    let path = fresh_path("refuse");
    let store = open(&path);
    tickets(&store);
    store
        .create_trigger(&trigger(
            "no_banned",
            TriggerTiming::Before,
            TriggerEvent::Insert,
            chart(
                "before_insert",
                Some(Guard::Eq {
                    key: "new.status".to_string(),
                    value: json!("banned"),
                }),
                vec![refuse("status banned")],
            ),
        ))
        .unwrap();
    let error = store
        .insert_rows(
            "tickets",
            &cols(),
            &[vec![json!(1), json!("ok")], vec![json!(2), json!("banned")]],
        )
        .unwrap_err();
    assert!(error.starts_with("TRIGGER_REFUSED: "), "{error}");
    assert!(error.contains("status banned"), "{error}");
    assert!(
        store.scan("tickets").unwrap().is_empty(),
        "the whole statement aborts, including the row before the refused one"
    );
    let _ = std::fs::remove_file(&path);
}

// spec: MEDULLA-ENGINE-PARITY-R012.1
#[test]
fn update_and_delete_triggers_see_old_and_new() {
    let path = fresh_path("update");
    let store = open(&path);
    tickets(&store);
    store
        .insert_rows("tickets", &cols(), &[vec![json!(1), json!("open")]])
        .unwrap();
    // Before update: rewrite the status a client sets.
    store
        .create_trigger(&trigger(
            "normalise",
            TriggerTiming::Before,
            TriggerEvent::Update,
            chart(
                "before_update",
                Some(Guard::Eq {
                    key: "new.status".to_string(),
                    value: json!("done"),
                }),
                vec![assign("new.status", json!("closed"))],
            ),
        ))
        .unwrap();
    store
        .update_where("tickets", &set_status("done"), &id_is(1))
        .unwrap();
    assert_eq!(status_of(&store, 1), Cell::Text("closed".into()));
    // Before update refuses a change away from `closed`, reading `old`.
    store
        .create_trigger(&trigger(
            "closed_is_final",
            TriggerTiming::Before,
            TriggerEvent::Update,
            chart(
                "before_update",
                Some(Guard::Eq {
                    key: "old.status".to_string(),
                    value: json!("closed"),
                }),
                vec![refuse("closed tickets are final")],
            ),
        ))
        .unwrap();
    let error = store
        .update_where("tickets", &set_status("open"), &id_is(1))
        .unwrap_err();
    assert!(error.starts_with("TRIGGER_REFUSED: "), "{error}");
    assert_eq!(status_of(&store, 1), Cell::Text("closed".into()));
    // Before delete refuses too.
    store
        .create_trigger(&trigger(
            "keep_closed",
            TriggerTiming::Before,
            TriggerEvent::Delete,
            chart(
                "before_delete",
                Some(Guard::Eq {
                    key: "old.status".to_string(),
                    value: json!("closed"),
                }),
                vec![refuse("keep closed tickets")],
            ),
        ))
        .unwrap();
    let error = store.delete_where("tickets", &id_is(1)).unwrap_err();
    assert!(error.starts_with("TRIGGER_REFUSED: "), "{error}");
    assert_eq!(store.scan("tickets").unwrap().len(), 1);
    let _ = std::fs::remove_file(&path);
}

// spec: MEDULLA-ENGINE-PARITY-R012.1
#[test]
fn after_trigger_cannot_rewrite_but_can_refuse() {
    let path = fresh_path("after");
    let store = open(&path);
    tickets(&store);
    store
        .create_trigger(&trigger(
            "ignored_assign",
            TriggerTiming::After,
            TriggerEvent::Insert,
            chart("after_insert", None, vec![assign("new.status", json!("x"))]),
        ))
        .unwrap();
    store
        .insert_rows("tickets", &cols(), &[vec![json!(1), json!("open")]])
        .unwrap();
    assert_eq!(
        status_of(&store, 1),
        Cell::Text("open".into()),
        "an After machine's Assign to `new` is ignored"
    );
    store
        .create_trigger(&trigger(
            "after_refuse",
            TriggerTiming::After,
            TriggerEvent::Insert,
            chart(
                "after_insert",
                Some(Guard::Eq {
                    key: "new.id".to_string(),
                    value: json!(2),
                }),
                vec![refuse("no id 2")],
            ),
        ))
        .unwrap();
    let error = store
        .insert_rows("tickets", &cols(), &[vec![json!(2), json!("open")]])
        .unwrap_err();
    assert!(error.starts_with("TRIGGER_REFUSED: "), "{error}");
    assert_eq!(
        store.scan("tickets").unwrap().len(),
        1,
        "the write rolled back"
    );
    // A trigger whose machine lacks its event is refused at creation.
    let bad = TriggerSpec::new(
        "tickets",
        "bad",
        TriggerTiming::Before,
        vec![TriggerEvent::Delete],
        chart("before_insert", None, Vec::new()),
    );
    let error = store.create_trigger(&bad).unwrap_err();
    assert!(error.starts_with("TRIGGER_MACHINE_INVALID: "), "{error}");
    let _ = std::fs::remove_file(&path);
}

fn serial_table(store: &TableStore, name: &str) {
    let mut id = Column::new("id", ColumnType::BigInt, false, true);
    id.serial = true;
    store
        .create_table(
            &TableSchema::new(
                name,
                vec![id, Column::new("note", ColumnType::Text, true, false)],
            ),
            false,
        )
        .unwrap();
}

/// An after-insert machine on `table` that inserts `{note: <new.note>}` into
/// `target` (a `Custom` write effect).
fn copy_into(table: &str, target: &str, name: &str) -> TriggerSpec {
    TriggerSpec::new(
        table,
        name,
        TriggerTiming::After,
        vec![TriggerEvent::Insert],
        chart(
            "after_insert",
            None,
            vec![Action::Custom {
                name: "insert_row".to_string(),
                args: json!({"table": target, "row": {"note": {"from": "new.note"}}}),
            }],
        ),
    )
}

fn row_count(store: &TableStore, table: &str) -> usize {
    store.scan(table).unwrap().len()
}

// spec: MEDULLA-ENGINE-PARITY-R012.2
#[test]
fn depth_bound_refuses() {
    let path = fresh_path("depth");
    let store = open(&path);
    serial_table(&store, "loop_t");
    // The trigger writes a fresh row into its own table, so each level is a new
    // event and only the depth bound stops the chain.
    store
        .create_trigger(&copy_into("loop_t", "loop_t", "again"))
        .unwrap();
    let error = store
        .insert_rows("loop_t", &["note".to_string()], &[vec![json!("x")]])
        .unwrap_err();
    assert!(error.starts_with("TRIGGER_DEPTH: "), "{error}");
    // The refusal aborts the whole statement and every nested write.
    assert_eq!(row_count(&store, "loop_t"), 0);
    let _ = std::fs::remove_file(&path);
}

// spec: MEDULLA-ENGINE-PARITY-R012.2
#[test]
fn nested_triggers_run_in_creation_order_and_write_the_ledger() {
    let path = fresh_path("nested");
    let store = open(&path);
    serial_table(&store, "src");
    serial_table(&store, "mid");
    serial_table(&store, "dst");
    store
        .create_trigger(&copy_into("src", "mid", "t1"))
        .unwrap();
    store
        .create_trigger(&copy_into("mid", "dst", "t2"))
        .unwrap();
    store
        .insert_rows("src", &["note".to_string()], &[vec![json!("hello")]])
        .unwrap();
    for table in ["src", "mid", "dst"] {
        assert_eq!(row_count(&store, table), 1, "{table}");
    }
    assert_eq!(
        store.scan("dst").unwrap()[0][1],
        Cell::Text("hello".to_string())
    );
    let _ = std::fs::remove_file(&path);
}

// spec: MEDULLA-ENGINE-PARITY-R012.2
#[test]
fn second_reaction_for_one_event_refuses() {
    let path = fresh_path("duplicate");
    let store = open(&path);
    // Two plain tables: the chain a -> b -> a re-inserts the same content, so the
    // trigger on `a` would react twice to one event of one write.
    for name in ["a", "b"] {
        store
            .create_table(
                &TableSchema::new(
                    name,
                    vec![Column::new("note", ColumnType::Text, true, false)],
                ),
                false,
            )
            .unwrap();
    }
    store
        .create_trigger(&copy_into("a", "b", "a_to_b"))
        .unwrap();
    store
        .create_trigger(&copy_into("b", "a", "b_to_a"))
        .unwrap();
    let error = store
        .insert_rows("a", &["note".to_string()], &[vec![json!("x")]])
        .unwrap_err();
    assert!(error.starts_with("TRIGGER_REACTION_DUPLICATE: "), "{error}");
    assert_eq!(row_count(&store, "a"), 0);
    assert_eq!(row_count(&store, "b"), 0);
    let _ = std::fs::remove_file(&path);
}

// spec: MEDULLA-ENGINE-PARITY-R012.2
#[test]
fn before_trigger_cannot_write_and_a_refusing_nested_trigger_aborts_everything() {
    let path = fresh_path("nested_refuse");
    let store = open(&path);
    serial_table(&store, "src");
    serial_table(&store, "dst");
    store
        .create_trigger(&copy_into("src", "dst", "copy"))
        .unwrap();
    store
        .create_trigger(&TriggerSpec::new(
            "dst",
            "no_dst",
            TriggerTiming::Before,
            vec![TriggerEvent::Insert],
            chart("before_insert", None, vec![refuse("dst is closed")]),
        ))
        .unwrap();
    let error = store
        .insert_rows("src", &["note".to_string()], &[vec![json!("x")]])
        .unwrap_err();
    assert!(error.starts_with("TRIGGER_REFUSED: "), "{error}");
    assert_eq!(row_count(&store, "src"), 0);
    assert_eq!(row_count(&store, "dst"), 0);
    // A before trigger that tries to write is not a valid machine.
    store.drop_trigger("dst", "no_dst").unwrap();
    store.drop_trigger("src", "copy").unwrap();
    store
        .create_trigger(&TriggerSpec::new(
            "src",
            "bad_before",
            TriggerTiming::Before,
            vec![TriggerEvent::Insert],
            chart(
                "before_insert",
                None,
                vec![Action::Custom {
                    name: "insert_row".to_string(),
                    args: json!({"table": "dst", "row": {"note": "y"}}),
                }],
            ),
        ))
        .unwrap();
    let error = store
        .insert_rows("src", &["note".to_string()], &[vec![json!("x")]])
        .unwrap_err();
    assert!(error.starts_with("TRIGGER_MACHINE_INVALID: "), "{error}");
    let _ = std::fs::remove_file(&path);
}

/// Serves one fixed machine under one id, as the engine's statechart store does.
struct FixedMachine {
    id: String,
    machine: StatechartDef,
}

impl medulla_table_store::tables::TriggerMachineSource for FixedMachine {
    fn machine(&self, _persist_dir: &Path, machine_id: &str) -> Result<StatechartDef, String> {
        if machine_id == self.id {
            Ok(self.machine.clone())
        } else {
            Err("no stored definition".to_string())
        }
    }
}

// spec: MEDULLA-ENGINE-PARITY-R012.2
#[test]
fn create_trigger_statement_runs_the_named_machine() {
    use medulla_sql_lang::sql::classify::{classify, StatementKind};
    use medulla_table_store::tables::{resolve_trigger_spec, TableTxn, TxnOp};

    let machine = chart(
        "before_insert",
        None,
        vec![assign("new.status", json!("open"))],
    );
    let id = machine.def_id();
    medulla_table_store::tables::install_trigger_machine_source(std::sync::Arc::new(
        FixedMachine {
            id: id.clone(),
            machine,
        },
    ));
    let path = fresh_path("ddl");
    let store = open(&path);
    tickets(&store);

    let create = classify(&format!(
        "CREATE TRIGGER stamp BEFORE INSERT ON tickets EXECUTE MACHINE {id}"
    ))
    .unwrap();
    let StatementKind::CreateTrigger(plan) = create else {
        panic!("expected CreateTrigger");
    };
    let spec = resolve_trigger_spec(
        &path,
        &plan.table,
        &plan.name,
        plan.timing,
        plan.events,
        &plan.machine_id,
    )
    .unwrap();
    let mut txn = TableTxn::new();
    txn.push(TxnOp::CreateTrigger { spec });
    store.commit_txn(&txn).unwrap();
    store
        .insert_rows("tickets", &cols(), &[vec![json!(1), json!("closed")]])
        .unwrap();
    assert_eq!(status_of(&store, 1), Cell::Text("open".to_string()));

    // An id the statechart store does not hold refuses before anything is written.
    let unknown = resolve_trigger_spec(
        &path,
        "tickets",
        "other",
        TriggerTiming::Before,
        vec![TriggerEvent::Insert],
        "no-such-machine",
    )
    .unwrap_err();
    assert!(
        unknown.starts_with("TRIGGER_MACHINE_INVALID: "),
        "{unknown}"
    );

    let StatementKind::DropTrigger { name, table } =
        classify("DROP TRIGGER stamp ON tickets").unwrap()
    else {
        panic!("expected DropTrigger");
    };
    let mut txn = TableTxn::new();
    txn.push(TxnOp::DropTrigger { table, name });
    store.commit_txn(&txn).unwrap();
    store
        .insert_rows("tickets", &cols(), &[vec![json!(2), json!("closed")]])
        .unwrap();
    assert_eq!(status_of(&store, 2), Cell::Text("closed".to_string()));
    let _ = std::fs::remove_file(&path);
}
