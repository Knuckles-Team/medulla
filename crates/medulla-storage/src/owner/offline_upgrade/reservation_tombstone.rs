//! MEDULLA-PLACEMENT-R781.4 — the offline upgrade of stored reservation rows that still carry the
//! retired `tombstone` field.
//!
//! The reservation record no longer has a `tombstone` field (a finished reservation is recognised
//! from its state). A row in table `resource_reservations` of a shard written before that is a
//! sealed-or-plain MessagePack map `{ record: { ..., state, tombstone, ... }, held_* ... }` and
//! the current record refuses unknown fields. This step rewrites every such row in the current
//! shape in the same commit that replaces the shard manifest. No pass runs at store open and no
//! compatibility reader exists.
//!
//! A row whose `tombstone` disagrees with its `state` (a `reserved` row with `tombstone` true, or
//! a terminal row with it false) fails the upgrade of that file with
//! `RESERVATION_TOMBSTONE_DISAGREES: <graph>/<reservation_id>`; the commit is not made, so the
//! file stays in its predecessor layout and the operator decides. A row that cannot be read as a
//! reservation fails with `RESERVATION_ROW_UNREADABLE: <graph>/<reservation_id>`. A shard written
//! with at-rest encryption holds sealed rows: `store-upgrade` carries the same at-rest key source
//! the engine uses (a `StoredRowCipher` over `DurableCrypto`), the step opens each row through it
//! and seals the converted row again, so an encrypted shard converts like a plain one.
//!
//! Caller chain: operator runs `store-upgrade` (`crates/medulla-persistence-ops/src/store_upgrade.rs`)
//! → `offline_upgrade` for the predecessor → `commit_upgrade` → `reservation_tombstone::convert`.

use crate::owner::graph_shard::RESOURCE_RESERVATIONS;
use crate::physical::integrity::StoredRowCipher;
use redb::{ReadableTable, WriteTransaction};
use serde::Deserialize;
use serde_json::Value;

/// The two fields of the old record shape this step reads: its `state` and the `tombstone` flag
/// it checks against it. Kept only in this file.
#[derive(Deserialize)]
struct OldRecordFacts {
    state: String,
    tombstone: bool,
}

/// The states of `ResourceReservationMachine` that leave no edge (`released`, `reclaimed`,
/// `expired`, `superseded`); a `reserved` or `absent` row is not terminal.
const TERMINAL_STATES: [&str; 4] = ["released", "reclaimed", "expired", "superseded"];

fn disagrees(graph: &str, reservation_id: &str) -> String {
    format!("RESERVATION_TOMBSTONE_DISAGREES: {graph}/{reservation_id}")
}

fn unreadable(graph: &str, reservation_id: &str) -> String {
    format!("RESERVATION_ROW_UNREADABLE: {graph}/{reservation_id}")
}

/// Convert one stored row. `Ok(None)` is a row already in the current shape (no `tombstone`
/// field), which is left untouched. With a `cipher` the stored value is a sealed blob: it is
/// opened through the cipher and the converted row is sealed again before it is written.
pub(crate) fn convert_row(
    graph: &str,
    reservation_id: &str,
    stored: &[u8],
    cipher: Option<&dyn StoredRowCipher>,
) -> Result<Option<Vec<u8>>, String> {
    let opened;
    let plain: &[u8] = match cipher {
        Some(cipher) => {
            opened = cipher
                .unseal(stored)
                .map_err(|_| unreadable(graph, reservation_id))?;
            &opened
        }
        None => stored,
    };
    let mut row: Value =
        rmp_serde::from_slice(plain).map_err(|_| unreadable(graph, reservation_id))?;
    let record = row
        .get_mut("record")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| unreadable(graph, reservation_id))?;
    if !record.contains_key("tombstone") {
        return Ok(None);
    }
    let facts: OldRecordFacts = serde_json::from_value(Value::Object(record.clone()))
        .map_err(|_| unreadable(graph, reservation_id))?;
    if facts.tombstone != TERMINAL_STATES.contains(&facts.state.as_str()) {
        return Err(disagrees(graph, reservation_id));
    }
    record.remove("tombstone");
    let rewritten = rmp_serde::to_vec_named(&row).map_err(|_| unreadable(graph, reservation_id))?;
    Ok(Some(match cipher {
        Some(cipher) => cipher.seal(&rewritten),
        None => rewritten,
    }))
}

/// Rewrite every row of `resource_reservations` in the current shape. Any row that disagrees or
/// cannot be read fails the call, and the caller's transaction is not committed.
pub(crate) fn convert(
    write: &WriteTransaction,
    cipher: Option<&dyn StoredRowCipher>,
) -> Result<(), String> {
    let mut table = write
        .open_table(RESOURCE_RESERVATIONS)
        .map_err(|error| error.to_string())?;
    let mut converted: Vec<(String, String, Vec<u8>)> = Vec::new();
    for row in table.iter().map_err(|error| error.to_string())? {
        let (key, value) = row.map_err(|error| error.to_string())?;
        let (graph, reservation_id) = key.value();
        if let Some(bytes) = convert_row(graph, reservation_id, value.value(), cipher)? {
            converted.push((graph.to_string(), reservation_id.to_string(), bytes));
        }
    }
    for (graph, reservation_id, bytes) in &converted {
        table
            .insert((graph.as_str(), reservation_id.as_str()), bytes.as_slice())
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::owner::identity::PhysicalStoreIdentity;
    use crate::owner::lineage::GRAPH_SHARD_BEFORE_RESERVATION_TOMBSTONE_REMOVAL;
    use crate::owner::offline_upgrade::{
        registered_offline_upgrade, OfflineUpgradeInspectionOptions,
    };
    use crate::owner::persisted_layout::create_predecessor_owner_file;
    use redb::{ReadableDatabase, ReadableTable};
    use serde_json::json;
    use std::path::Path;

    fn identity() -> PhysicalStoreIdentity {
        PhysicalStoreIdentity::new("physical:test:reservation-tombstone").unwrap()
    }

    fn options(path: &Path) -> OfflineUpgradeInspectionOptions {
        OfflineUpgradeInspectionOptions::new(
            path.parent()
                .unwrap()
                .join("reservation-tombstone-inspection"),
            256 * 1024 * 1024,
        )
        .unwrap()
    }

    fn old_row(state: &str, tombstone: bool) -> Vec<u8> {
        rmp_serde::to_vec_named(&json!({
            "record": { "reservation_id": "r", "state": state, "tombstone": tombstone, "revision": 3 },
            "held_cpu_weight": 0,
            "fairness_debt": 0,
        }))
        .unwrap()
    }

    fn seed(path: &Path, rows: &[(&str, &str, Vec<u8>)]) {
        let database = redb::Database::open(path).unwrap();
        let write = database.begin_write().unwrap();
        {
            let mut table = write.open_table(RESOURCE_RESERVATIONS).unwrap();
            for (graph, id, bytes) in rows {
                table.insert((*graph, *id), bytes.as_slice()).unwrap();
            }
        }
        write.commit().unwrap();
    }

    fn upgrade(path: &Path) -> Result<(), String> {
        upgrade_with(path, options(path))
    }

    fn upgrade_with(path: &Path, options: OfflineUpgradeInspectionOptions) -> Result<(), String> {
        registered_offline_upgrade(&GRAPH_SHARD_BEFORE_RESERVATION_TOMBSTONE_REMOVAL)
            .expect("the predecessor has a registered upgrade")
            .inspect(path, identity(), None, options)?
            .apply()
            .map(drop)
    }

    /// A stand-in for the engine's at-rest cipher: a sealed blob is the plaintext XOR a key byte
    /// behind a marker, and a blob without the marker fails closed as a real cipher's does.
    struct MarkerCipher;

    impl StoredRowCipher for MarkerCipher {
        fn unseal(&self, stored: &[u8]) -> Result<Vec<u8>, String> {
            let body = stored.strip_prefix(b"SEALED:").ok_or("not sealed")?;
            Ok(body.iter().map(|byte| byte ^ 0x5a).collect())
        }

        fn seal(&self, plaintext: &[u8]) -> Vec<u8> {
            let mut sealed = b"SEALED:".to_vec();
            sealed.extend(plaintext.iter().map(|byte| byte ^ 0x5a));
            sealed
        }
    }

    // spec: MEDULLA-PLACEMENT-R781.4
    #[test]
    fn consistent_rows_are_rewritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("graph-0.redb");
        create_predecessor_owner_file(
            &path,
            identity(),
            &GRAPH_SHARD_BEFORE_RESERVATION_TOMBSTONE_REMOVAL,
        )
        .unwrap();
        seed(
            &path,
            &[
                ("graph-a", "live", old_row("reserved", false)),
                ("graph-a", "done", old_row("released", true)),
                ("graph-b", "gone", old_row("expired", true)),
            ],
        );
        upgrade(&path).unwrap();
        let database = redb::Database::open(&path).unwrap();
        let read = database.begin_read().unwrap();
        let table = read.open_table(RESOURCE_RESERVATIONS).unwrap();
        let mut seen = 0;
        for row in table.iter().unwrap() {
            let (_, value) = row.unwrap();
            let decoded: Value = rmp_serde::from_slice(value.value()).unwrap();
            assert!(decoded["record"].get("tombstone").is_none());
            assert_eq!(decoded["record"]["revision"], 3);
            seen += 1;
        }
        assert_eq!(seen, 3);
    }

    // spec: MEDULLA-PLACEMENT-R781.4
    #[test]
    fn a_disagreeing_row_fails_the_upgrade_with_its_id() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("graph-0.redb");
        create_predecessor_owner_file(
            &path,
            identity(),
            &GRAPH_SHARD_BEFORE_RESERVATION_TOMBSTONE_REMOVAL,
        )
        .unwrap();
        seed(
            &path,
            &[
                ("graph-a", "fine", old_row("released", true)),
                ("graph-a", "bad", old_row("reserved", true)),
            ],
        );
        let error = upgrade(&path).unwrap_err();
        assert_eq!(error, "RESERVATION_TOMBSTONE_DISAGREES: graph-a/bad");
        // The file is untouched: it still refuses an ordinary open as the predecessor.
        let refused =
            crate::StorageKernel::open_owner::<crate::GraphShardOwner>(&path, identity(), None)
                .err()
                .expect("a failed upgrade leaves the predecessor layout");
        assert!(
            refused.starts_with("GRAPH_SHARD_FORMAT_UPGRADE_REQUIRED:"),
            "{refused}"
        );
    }

    // spec: MEDULLA-PLACEMENT-R781.4
    #[test]
    fn an_encrypted_shard_converts_through_the_at_rest_cipher() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("graph-0.redb");
        create_predecessor_owner_file(
            &path,
            identity(),
            &GRAPH_SHARD_BEFORE_RESERVATION_TOMBSTONE_REMOVAL,
        )
        .unwrap();
        seed(
            &path,
            &[
                (
                    "graph-a",
                    "live",
                    MarkerCipher.seal(&old_row("reserved", false)),
                ),
                (
                    "graph-a",
                    "done",
                    MarkerCipher.seal(&old_row("released", true)),
                ),
            ],
        );
        upgrade_with(
            &path,
            options(&path).with_row_cipher(std::sync::Arc::new(MarkerCipher)),
        )
        .unwrap();
        let database = redb::Database::open(&path).unwrap();
        let read = database.begin_read().unwrap();
        let table = read.open_table(RESOURCE_RESERVATIONS).unwrap();
        let mut seen = 0;
        for row in table.iter().unwrap() {
            let (_, value) = row.unwrap();
            let opened = MarkerCipher
                .unseal(value.value())
                .expect("the converted row is sealed again");
            let decoded: Value = rmp_serde::from_slice(&opened).unwrap();
            assert!(decoded["record"].get("tombstone").is_none());
            assert_eq!(decoded["record"]["revision"], 3);
            seen += 1;
        }
        assert_eq!(seen, 2);
    }

    // spec: MEDULLA-PLACEMENT-R781.4
    #[test]
    fn an_encrypted_shard_without_the_key_fails_naming_its_row() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("graph-0.redb");
        create_predecessor_owner_file(
            &path,
            identity(),
            &GRAPH_SHARD_BEFORE_RESERVATION_TOMBSTONE_REMOVAL,
        )
        .unwrap();
        seed(
            &path,
            &[(
                "graph-a",
                "live",
                MarkerCipher.seal(&old_row("reserved", false)),
            )],
        );
        assert_eq!(
            upgrade(&path).unwrap_err(),
            "RESERVATION_ROW_UNREADABLE: graph-a/live"
        );
    }
}
