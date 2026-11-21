//! MEDULLA-CQ-ORCH-R811.4 — the offline upgrade of a jobs store to the analytics job machine.
//!
//! A stored analytics-job record (msgpack, `rmp_serde::to_vec_named`) in the retired table
//! `analytics_jobs` carries `cancel_requested`, `lease`, `not_before_ms` and a six-variant `state`.
//! The machine shape keeps one state, with the lease, the cancel request and the backoff time as
//! part of it, in table `analytics_job_records`. A value shape is invisible to the owner layout
//! digest, so the cutover renames the table: this upgrade converts every record in the same
//! commit that replaces the manifest, and a record it cannot map refuses the whole upgrade.
//!
//! Caller chain: engine open, or the operator `apply` command → `migrate_on_open` →
//! `upgrade_jobs_job_machine` → `convert_job_rows` → `convert_job_record`.

use super::agent_library_upgrade::{
    inspect_layout_upgrade, upgrade_layout, OwnerLayoutUpgradeReport, UpgradeTarget,
    ValidatedOwnerLayoutUpgrade,
};
use super::identity::PhysicalStoreIdentity;
use super::registry::{ANALYTICS_JOBS_BEFORE_JOB_MACHINE, ANALYTICS_JOB_RECORDS};
use super::sql_checkpoint_upgrade::SqlSourceCheckpointInspectionOptions;
use crate::physical::integrity::PrivatePayloadIntegrity;
use crate::StorageKernel;
use redb::{ReadableTable, WriteTransaction};
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::Arc;

/// Inspect a jobs owner file from before the analytics job machine, tied to the source
/// descriptor and bytes.
pub fn inspect_jobs_job_machine_upgrade(
    path: &Path,
    expected_physical_identity: PhysicalStoreIdentity,
    private_integrity: Option<Arc<dyn PrivatePayloadIntegrity>>,
    options: SqlSourceCheckpointInspectionOptions,
) -> Result<ValidatedOwnerLayoutUpgrade, String> {
    inspect_layout_upgrade(
        path,
        expected_physical_identity,
        private_integrity,
        options,
        UpgradeTarget::Jobs,
    )
}

/// Bring an inspected jobs file to the current layout in one durable commit: every record of
/// `analytics_jobs` is converted to the machine shape into `analytics_job_records` and the retired
/// table is deleted.
pub fn upgrade_jobs_job_machine(
    token: ValidatedOwnerLayoutUpgrade,
) -> Result<(StorageKernel, OwnerLayoutUpgradeReport), String> {
    upgrade_layout(token, UpgradeTarget::Jobs)
}

/// The failure of a record the conversion cannot map: `JOBS_RECORD_UNMAPPABLE: <job id>`.
fn unmappable(job_id: &str) -> String {
    format!("JOBS_RECORD_UNMAPPABLE: {job_id}")
}

/// Convert every record of the retired table into the new table, then delete the retired table.
/// The conversion reads no clock. Any unmappable record fails the call with
/// `JOBS_RECORD_UNMAPPABLE: <job id>` and the caller's transaction is not committed, so the file
/// stays in its previous layout.
pub(crate) fn convert_job_rows(write: &WriteTransaction) -> Result<(), String> {
    let converted: Vec<(String, Vec<u8>)> = {
        let old = write
            .open_table(ANALYTICS_JOBS_BEFORE_JOB_MACHINE)
            .map_err(|error| error.to_string())?;
        let mut converted = Vec::new();
        for row in old.iter().map_err(|error| error.to_string())? {
            let (key, value) = row.map_err(|error| error.to_string())?;
            let job_id = key.value().to_string();
            let decoded = rmp_serde::from_slice::<Value>(value.value())
                .map_err(|_| unmappable(&job_id))?;
            let record = convert_job_record(&decoded)?;
            let bytes = rmp_serde::to_vec_named(&record).map_err(|_| unmappable(&job_id))?;
            converted.push((job_id, bytes));
        }
        converted
    };
    {
        let mut records = write
            .open_table(ANALYTICS_JOB_RECORDS)
            .map_err(|error| error.to_string())?;
        for (job_id, bytes) in &converted {
            records
                .insert(job_id.as_str(), bytes.as_slice())
                .map_err(|error| error.to_string())?;
        }
    }
    write
        .delete_table(ANALYTICS_JOBS_BEFORE_JOB_MACHINE)
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Convert one stored record of the old shape to the machine shape.
///
/// `Running` and `Publishing` with `cancel_requested` and a lease become
/// `CancelRequested { lease, checkpoint }`; with `cancel_requested` and no lease they become
/// `Cancelled { checkpoint }`; without `cancel_requested` they keep their variant and move the
/// stored lease into the payload (no lease refuses the record). `Submitted` with
/// `not_before_ms > 0` and at least one attempt made becomes
/// `Backoff { not_before_ms, checkpoint: None }`, else `Submitted`. `Succeeded`, `Failed` and
/// `Cancelled` keep their payload. The fields `cancel_requested`, `lease` and `not_before_ms` are
/// dropped. Any other shape fails with `JOBS_RECORD_UNMAPPABLE: <job id>`.
pub fn convert_job_record(old: &Value) -> Result<Value, String> {
    let job_id = old
        .get("job_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let refuse = || unmappable(&job_id);
    let record = old.as_object().ok_or_else(refuse)?;
    let cancel_requested = record
        .get("cancel_requested")
        .and_then(Value::as_bool)
        .ok_or_else(refuse)?;
    let lease = match record.get("lease") {
        None | Some(Value::Null) => None,
        Some(lease) if lease.is_object() => Some(lease.clone()),
        Some(_) => return Err(refuse()),
    };
    let not_before_ms = match record.get("not_before_ms") {
        None | Some(Value::Null) => 0,
        Some(value) => value.as_i64().ok_or_else(refuse)?,
    };
    let attempts_made = record
        .get("retry")
        .and_then(|retry| retry.get("attempts_made"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let state = record.get("state").ok_or_else(refuse)?;
    let state = convert_state(
        state,
        cancel_requested,
        lease,
        (not_before_ms, attempts_made),
    )
    .ok_or_else(refuse)?;
    let mut converted: Map<String, Value> = record.clone();
    for dropped in ["cancel_requested", "lease", "not_before_ms"] {
        converted.remove(dropped);
    }
    converted.insert("state".to_string(), state);
    Ok(Value::Object(converted))
}

/// The machine-shape state of an old `state`, or `None` when the old shape maps to nothing.
fn convert_state(
    state: &Value,
    cancel_requested: bool,
    lease: Option<Value>,
    (not_before_ms, attempts_made): (i64, u64),
) -> Option<Value> {
    if let Value::String(name) = state {
        if name != "Submitted" {
            return None;
        }
        return Some(if not_before_ms > 0 && attempts_made >= 1 {
            json!({ "Backoff": { "not_before_ms": not_before_ms, "checkpoint": null } })
        } else {
            json!("Submitted")
        });
    }
    let map = state.as_object().filter(|map| map.len() == 1)?;
    let (variant, body) = map.iter().next()?;
    let body = body.as_object()?;
    match variant.as_str() {
        "Running" | "Publishing" => {
            let checkpoint = body.get("checkpoint").filter(|value| value.is_object())?;
            match (cancel_requested, lease) {
                (true, Some(lease)) => Some(
                    json!({ "CancelRequested": { "lease": lease, "checkpoint": checkpoint } }),
                ),
                (true, None) => Some(json!({ "Cancelled": { "checkpoint": checkpoint } })),
                (false, Some(lease)) => {
                    let mut body = body.clone();
                    body.insert("lease".to_string(), lease);
                    let mut wrapped = Map::new();
                    wrapped.insert(variant.clone(), Value::Object(body));
                    Some(Value::Object(wrapped))
                }
                (false, None) => None,
            }
        }
        "Succeeded" | "Failed" | "Cancelled" => Some(state.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease() -> Value {
        json!({ "worker_ref": "w", "epoch": 2, "acquired_at_ms": 10, "expires_at_ms": 20 })
    }

    fn checkpoint() -> Value {
        json!({ "progress": 0.5, "stage": "half", "state_blob": null, "updated_at_ms": 15 })
    }

    fn record(state: Value, cancel_requested: bool, lease: Option<Value>, not_before: i64, attempts: u64) -> Value {
        json!({
            "job_id": "job-0000000000000001",
            "retry": { "max_attempts": 3, "backoff_ms": 10, "attempts_made": attempts },
            "state": state,
            "cancel_requested": cancel_requested,
            "lease_epoch": 2,
            "lease": lease,
            "last_worker_ref": "w",
            "not_before_ms": not_before,
            "output": null,
            "created_at_ms": 1,
            "updated_at_ms": 2,
        })
    }

    fn converted_state(old: Value) -> Value {
        let converted = convert_job_record(&old).unwrap();
        let object = converted.as_object().unwrap();
        for dropped in ["cancel_requested", "lease", "not_before_ms"] {
            assert!(!object.contains_key(dropped), "{dropped} is dropped");
        }
        assert_eq!(object["job_id"], "job-0000000000000001");
        assert_eq!(object["lease_epoch"], 2);
        object["state"].clone()
    }

    // spec: MEDULLA-CQ-ORCH-R811.4

    #[test]
    fn maps_every_stored_combination_to_one_state() {
        let running = json!({ "Running": { "checkpoint": checkpoint() } });
        let publishing = json!({ "Publishing": { "result_ref": "r", "checkpoint": checkpoint() } });
        // A live job keeps its variant and carries its lease in the payload.
        assert_eq!(
            converted_state(record(running.clone(), false, Some(lease()), 0, 1)),
            json!({ "Running": { "lease": lease(), "checkpoint": checkpoint() } })
        );
        assert_eq!(
            converted_state(record(publishing.clone(), false, Some(lease()), 0, 1)),
            json!({ "Publishing": { "lease": lease(), "result_ref": "r", "checkpoint": checkpoint() } })
        );
        // A cancel request with a lease waits for its holder; without one it is cancelled.
        for state in [running.clone(), publishing.clone()] {
            assert_eq!(
                converted_state(record(state.clone(), true, Some(lease()), 0, 1)),
                json!({ "CancelRequested": { "lease": lease(), "checkpoint": checkpoint() } })
            );
            assert_eq!(
                converted_state(record(state, true, None, 0, 1)),
                json!({ "Cancelled": { "checkpoint": checkpoint() } })
            );
        }
        // A submitted job waits in Backoff only after an attempt and with a future start.
        assert_eq!(
            converted_state(record(json!("Submitted"), false, None, 500, 1)),
            json!({ "Backoff": { "not_before_ms": 500, "checkpoint": null } })
        );
        assert_eq!(
            converted_state(record(json!("Submitted"), false, None, 500, 0)),
            json!("Submitted")
        );
        assert_eq!(
            converted_state(record(json!("Submitted"), false, None, 0, 2)),
            json!("Submitted")
        );
        // Terminal states keep their payload.
        for state in [
            json!({ "Succeeded": { "result_ref": "r", "checkpoint": checkpoint() } }),
            json!({ "Failed": { "reason": "boom", "checkpoint": null } }),
            json!({ "Cancelled": { "checkpoint": checkpoint() } }),
        ] {
            assert_eq!(
                converted_state(record(state.clone(), true, None, 0, 1)),
                state
            );
        }
    }

    // spec: MEDULLA-CQ-ORCH-R811.4

    #[test]
    fn unmappable_record_fails_the_upgrade() {
        let expected = "JOBS_RECORD_UNMAPPABLE: job-0000000000000001";
        let running = json!({ "Running": { "checkpoint": checkpoint() } });
        // Running without a lease and without a cancel request has no owner to carry.
        assert_eq!(
            convert_job_record(&record(running, false, None, 0, 1)).unwrap_err(),
            expected
        );
        // A state outside the old shape.
        for state in [
            json!("Running"),
            json!({ "Backoff": { "not_before_ms": 1, "checkpoint": null } }),
            json!({ "Running": { "checkpoint": checkpoint() }, "Failed": {} }),
            json!({ "Running": "not a body" }),
        ] {
            assert_eq!(
                convert_job_record(&record(state, false, Some(lease()), 0, 1)).unwrap_err(),
                expected
            );
        }
        // A record without the cancel flag is not the old shape.
        let mut missing = record(json!("Submitted"), false, None, 0, 0);
        missing.as_object_mut().unwrap().remove("cancel_requested");
        assert_eq!(convert_job_record(&missing).unwrap_err(), expected);
        // The whole table refuses: nothing is committed and the old table stays.
        let dir = tempfile::tempdir().unwrap();
        let database = redb::Database::create(dir.path().join("jobs.redb")).unwrap();
        let write = database.begin_write().unwrap();
        {
            let mut table = write.open_table(ANALYTICS_JOBS_BEFORE_JOB_MACHINE).unwrap();
            let good = rmp_serde::to_vec_named(&record(json!("Submitted"), false, None, 0, 0)).unwrap();
            table.insert("job-0000000000000001", good.as_slice()).unwrap();
            let mut bad = record(json!({ "Running": { "checkpoint": checkpoint() } }), false, None, 0, 1);
            bad["job_id"] = json!("job-0000000000000002");
            let bad = rmp_serde::to_vec_named(&bad).unwrap();
            table.insert("job-0000000000000002", bad.as_slice()).unwrap();
        }
        assert_eq!(
            convert_job_rows(&write).unwrap_err(),
            "JOBS_RECORD_UNMAPPABLE: job-0000000000000002"
        );
        drop(write);
    }

    // spec: MEDULLA-CQ-ORCH-R811.4

    #[test]
    fn rows_move_to_the_new_table_and_the_old_table_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let database = redb::Database::create(dir.path().join("jobs.redb")).unwrap();
        let write = database.begin_write().unwrap();
        {
            let mut table = write.open_table(ANALYTICS_JOBS_BEFORE_JOB_MACHINE).unwrap();
            let live = record(
                json!({ "Running": { "checkpoint": checkpoint() } }),
                false,
                Some(lease()),
                0,
                1,
            );
            table
                .insert("job-0000000000000001", rmp_serde::to_vec_named(&live).unwrap().as_slice())
                .unwrap();
        }
        convert_job_rows(&write).unwrap();
        {
            let records = write.open_table(ANALYTICS_JOB_RECORDS).unwrap();
            let stored = records.get("job-0000000000000001").unwrap().unwrap();
            let decoded: Value = rmp_serde::from_slice(stored.value()).unwrap();
            assert_eq!(
                decoded["state"],
                json!({ "Running": { "lease": lease(), "checkpoint": checkpoint() } })
            );
        }
        let remaining: Vec<String> = write
            .list_tables()
            .unwrap()
            .map(|table| redb::TableHandle::name(&table).to_string())
            .collect();
        assert!(!remaining.iter().any(|name| name == "analytics_jobs"), "{remaining:?}");
        assert!(remaining.iter().any(|name| name == "analytics_job_records"));
    }
}
