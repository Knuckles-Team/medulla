//! The WorkItem store cutover onto the machine (MEDULLA-ORCH-VOCAB-R812.2): every status
//! write goes through `apply`, no hand-copied terminal list remains, and the retired
//! `completed` literal is refused.

use std::path::{Path, PathBuf};

/// The production part (before the first test module) of every `.rs` file under `dirs`.
fn production_sources(dirs: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = dirs.to_vec();
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            for entry in std::fs::read_dir(&path).unwrap().flatten() {
                stack.push(entry.path());
            }
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        // The cutover tests themselves name the patterns they scan for.
        // A test module is a `tests` directory or a `tests.rs` / `*_tests.rs` file, which a
        // `#[cfg(test)] mod` declaration in its parent compiles only under test.
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let test_file = stem == "tests" || stem.ends_with("_tests");
        if test_file || path.components().any(|part| part.as_os_str() == "tests") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let production = text
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_default()
            .to_string();
        out.push((path, production));
    }
    out
}

fn writer_dirs() -> Vec<PathBuf> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    vec![
        crates.join("medulla-work-store/src/redb_store/work_item"),
        crates.join("medulla-lane-store/src/redb_store/development_lane"),
    ]
}

fn compact(text: &str) -> String {
    text.split_whitespace().collect()
}

// spec: MEDULLA-ORCH-VOCAB-R812.2
#[test]
fn no_writer_sets_status_outside_apply() {
    let mut offenders = Vec::new();
    for (path, text) in production_sources(&writer_dirs()) {
        let flat = compact(&text);
        if flat.contains("insert(\"status\".into(),serde_json::Value::String(\"") {
            offenders.push(path.display().to_string());
        }
    }
    assert!(
        offenders.is_empty(),
        "raw status literal writes: {offenders:?}"
    );
    // The one writer is `machine::fire`, which stores what `apply` returned.
    let machine = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/redb_store/work_item/machine.rs"),
    )
    .unwrap();
    assert!(compact(&machine).contains("from.apply(event,facts)"));
}

// spec: MEDULLA-ORCH-VOCAB-R812.2
#[test]
fn no_hand_copied_terminal_set_remains() {
    let mut offenders = Vec::new();
    for (path, text) in production_sources(&writer_dirs()) {
        let flat = compact(&text);
        for list in [
            "\"succeeded\"|\"failed\"|\"cancelled\"|\"dead_letter\"",
            "\"leased\"|\"running\"",
            "\"completed\"",
        ] {
            if flat.contains(list) {
                offenders.push(format!("{}: {list}", path.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "hand-copied state lists: {offenders:?}"
    );
}

// spec: MEDULLA-ORCH-VOCAB-R812.2
#[test]
fn completed_literal_is_refused() {
    let row: serde_json::Map<String, serde_json::Value> =
        serde_json::from_value(serde_json::json!({
            "node_type": "WorkItem",
            "tenant": "tenant-a",
            "status": "completed",
        }))
        .unwrap();
    let error = super::stored_status(&row).unwrap_err();
    assert!(error.starts_with("WORK_ITEM_STATUS_UNKNOWN"), "{error}");
    assert!(!super::is_terminal_row(&row));
    let error = super::super::is_submit_work_item_inflight_row(&row, "tenant-a").unwrap_err();
    assert!(error.contains("completed"), "{error}");
    // A firing on the row is refused and leaves it unchanged.
    let mut fired = row.clone();
    let facts = super::row_facts(&row, 0.0, false);
    assert!(super::fire(
        &mut fired,
        medulla_work_types::work_item_machine::WorkItemEvent::Cancel,
        &facts
    )
    .is_err());
    assert_eq!(fired, row);
}

// spec: MEDULLA-ORCH-VOCAB-R812.2
#[test]
fn fire_stores_the_state_apply_returned_and_refuses_illegal_events() {
    let mut row: serde_json::Map<String, serde_json::Value> =
        serde_json::from_value(serde_json::json!({"status": "succeeded"})).unwrap();
    let facts = super::row_facts(&row, 0.0, false);
    let error = super::fire(
        &mut row,
        medulla_work_types::work_item_machine::WorkItemEvent::Cancel,
        &facts,
    )
    .unwrap_err();
    assert!(error.starts_with("TRANSITION_REFUSED"), "{error}");
    assert_eq!(row["status"], "succeeded");
    let mut row: serde_json::Map<String, serde_json::Value> =
        serde_json::from_value(serde_json::json!({"status": "ready"})).unwrap();
    super::fire(
        &mut row,
        medulla_work_types::work_item_machine::WorkItemEvent::Cancel,
        &facts,
    )
    .unwrap();
    assert_eq!(row["status"], "cancelled");
}
