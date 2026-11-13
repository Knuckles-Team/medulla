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
