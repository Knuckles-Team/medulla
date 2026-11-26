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
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
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

use super::*;
use crate::redb_store::work_item::test_shard::{
    apply, open, run_request, submit_run_rows, with_write, GRAPH,
};
use medulla_protocol_types::protocol::{Method, ResultPayload};
use medulla_protocol_types::result_contract::coordination::{
    WorkItemCancelOutcome, WorkItemCommitOutcome, WorkItemTransition,
};
use medulla_result_types::epistemic_operations::{
    ClaimWorkItemRequest, ClaimWorkItemRequestSchemaVersion, ClaimWorkItemResult,
};

fn payload<T: serde::de::DeserializeOwned>(result: ResultPayload) -> T {
    match result {
        ResultPayload::Raw(bytes) => medulla_store::redb_store::decode_durable(&bytes)
            .expect("the work item row dispatch answers a durable result"),
        other => panic!("expected a raw durable result, got {other:?}"),
    }
}

fn submit(shard: &Shard, key: &str) -> String {
    let mut request = run_request();
    request.idempotency_key = key.to_string();
    request.run_id = None;
    with_write(shard, &format!("submit/{key}"), 1_000, |admitted| {
        submit_run_rows(admitted, &request, "owner")
    })
    .expect("the work item submits")
    .work_item_id
}

fn claim(shard: &Shard, id: &str) -> ClaimWorkItemResult {
    let method = Method::ClaimWorkItem {
        request: ClaimWorkItemRequest {
            schema_version: ClaimWorkItemRequestSchemaVersion::V1,
            tenant_ref: "tenant".into(),
            work_item_id: Some(id.to_string()),
            queue_ref: None,
            resource_class: None,
            fairness_group: None,
            worker_ref: "worker-1".into(),
            now_ms: 2_000,
            lease_ms: 60_000,
            max_tenant_in_flight: 64,
        },
    };
    payload(apply(shard, &format!("claim/{id}"), method, 2_000).expect("the claim applies"))
}

fn commit(
    id: &str,
    lease_epoch: u64,
    fencing_token: u64,
    key: &str,
) -> Method {
    Method::CommitWorkItemResult {
        tenant: "tenant".into(),
        work_item_id: id.to_string(),
        worker_id: "worker-1".into(),
        lease_epoch,
        fencing_token,
        idempotency_key: key.to_string(),
        outcome: "succeeded".into(),
        result_ref: Some("cas:result:one".into()),
        error_ref: None,
        retryable: false,
        now_ms: 3_000,
        outcome_extension: None,
        run_completion: None,
    }
}

fn cancel(id: &str, key: &str) -> Method {
    Method::CancelWorkItem {
        tenant: "tenant".into(),
        work_item_id: id.to_string(),
        idempotency_key: key.to_string(),
        reason_ref: None,
        now_ms: 4_000,
    }
}

fn status(shard: &Shard, id: &str) -> serde_json::Value {
    let view = read_work_item(shard, GRAPH, "tenant", id, DurableCrypto::none())
        .expect("the work item reads")
        .expect("the work item exists");
    serde_json::to_value(view.status).expect("the status serializes")
}

// spec: MEDULLA-ORCH-VOCAB-R812.4
#[test]
fn illegal_work_item_transitions_answer_typed_outcomes_through_the_row_dispatch() {
    let temp = open("r812-4-illegal-transitions");
    let shard = &temp.shard;

    // A commit on a Cancelled item answers Noop and leaves the row unchanged.
    let cancelled = submit(shard, "r812-4-cancelled");
    let outcome: WorkItemTransition<WorkItemCancelOutcome> =
        payload(apply(shard, "cancel-first", cancel(&cancelled, "cancel-1"), 4_000).unwrap());
    assert_eq!(outcome.outcome, WorkItemCancelOutcome::Cancelled);
    assert_eq!(status(shard, &cancelled), serde_json::json!("cancelled"));
    let after: WorkItemTransition<WorkItemCommitOutcome> = payload(
        apply(shard, "commit-after-cancel", commit(&cancelled, 1, 1, "commit-1"), 4_500)
            .unwrap(),
    );
    assert_eq!(after.outcome, WorkItemCommitOutcome::Noop);
    assert_eq!(status(shard, &cancelled), serde_json::json!("cancelled"));

    // A cancel on a Succeeded item answers Noop and leaves the row unchanged.
    let succeeded = submit(shard, "r812-4-succeeded");
    let hold = claim(shard, &succeeded);
    let done: WorkItemTransition<WorkItemCommitOutcome> = payload(
        apply(
            shard,
            "commit-ok",
            commit(
                &succeeded,
                hold.lease_epoch.unwrap(),
                hold.fencing_token.unwrap(),
                "commit-2",
            ),
            3_000,
        )
        .unwrap(),
    );
    assert_eq!(done.outcome, WorkItemCommitOutcome::Succeeded);
    let late: WorkItemTransition<WorkItemCancelOutcome> =
        payload(apply(shard, "cancel-late", cancel(&succeeded, "cancel-2"), 4_000).unwrap());
    assert_eq!(late.outcome, WorkItemCancelOutcome::Noop);
    assert_eq!(status(shard, &succeeded), serde_json::json!("succeeded"));

    // A commit with a stale fencing token answers Fenced and leaves the row unchanged.
    let leased = submit(shard, "r812-4-fenced");
    let hold = claim(shard, &leased);
    let before = status(shard, &leased);
    let fenced: WorkItemTransition<WorkItemCommitOutcome> = payload(
        apply(
            shard,
            "commit-stale",
            commit(
                &leased,
                hold.lease_epoch.unwrap(),
                hold.fencing_token.unwrap() + 7,
                "commit-3",
            ),
            3_000,
        )
        .unwrap(),
    );
    assert_eq!(fenced.outcome, WorkItemCommitOutcome::Fenced);
    assert_eq!(status(shard, &leased), before);
}

// spec: MEDULLA-DURABLE-KERNEL-R028.7
#[test]
fn a_claim_answers_the_identity_the_item_was_submitted_with() {
    let temp = open("r028-7-claim-identity");
    let shard = &temp.shard;
    let mut request = run_request();
    request.idempotency_key = "identity-key".to_string();
    request.run_id = None;
    request.metadata = serde_json::from_value(serde_json::json!({
        "capability_digest": "sha256:cc",
        "delegation_id": "delegation-1",
        "agent_id": "selected-agent",
    }))
    .unwrap();
    let id = with_write(shard, "submit/identity", 1_000, |admitted| {
        submit_run_rows(admitted, &request, "owner")
    })
    .expect("the work item submits")
    .work_item_id;

    let claimed = claim(shard, &id);
    assert!(claimed.claimed);
    assert_eq!(claimed.capability_digest.as_deref(), Some("sha256:cc"));
    assert_eq!(claimed.delegation_id.as_deref(), Some("delegation-1"));
    assert_eq!(claimed.selected_agent_id.as_deref(), Some("selected-agent"));
    assert_eq!(claimed.delegator_id.as_deref(), Some("worker"));
    assert_eq!(claimed.catalog_digest.as_deref(), Some("catalog"));
    assert_eq!(claimed.model_digest.as_deref(), Some("model"));
    assert_eq!(claimed.policy_digest.as_deref(), Some("policy"));

    // A refused claim carries no identity.
    let empty = claim_nothing(shard);
    assert!(!empty.claimed);
    assert!(empty.capability_digest.is_none() && empty.delegator_id.is_none());
}

fn claim_nothing(shard: &Shard) -> ClaimWorkItemResult {
    let method = Method::ClaimWorkItem {
        request: ClaimWorkItemRequest {
            schema_version: ClaimWorkItemRequestSchemaVersion::V1,
            tenant_ref: "tenant".into(),
            work_item_id: Some("no-such-item".into()),
            queue_ref: None,
            resource_class: None,
            fairness_group: None,
            worker_ref: "worker-2".into(),
            now_ms: 2_000,
            lease_ms: 60_000,
            max_tenant_in_flight: 64,
        },
    };
    payload(apply(shard, "claim/none", method, 2_000).expect("the claim applies"))
}
