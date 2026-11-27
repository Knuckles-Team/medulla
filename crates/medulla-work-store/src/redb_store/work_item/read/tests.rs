//! Store-level proof of the tenant-bound WorkItem reads over a real shard:
//! rows are written by the SAME native row writer the transitions use, so the
//! revision the read projects is the one the writer maintains.

use super::*;
use medulla_work_types::work_item_read::WorkItemStatus;

use super::super::test_shard::{open, with_node_rows, with_nodes, GRAPH};

pub fn work_item(
    tenant: &str,
    kind: &str,
    status: &str,
) -> serde_json::Map<String, serde_json::Value> {
    serde_json::json!({
        "status": status,
        "kind": kind,
        "tenant": tenant,
        "node_type": "WorkItem",
        "updated_at": 2.5,
        "metadata": {"step": 1},
    })
    .as_object()
    .cloned()
    .expect("fixture row is an object")
}

/// Write `rows` through the native WorkItem row writer.
fn write_rows(
    shard: &Shard,
    tag: &str,
    rows: Vec<(&str, serde_json::Map<String, serde_json::Value>)>,
) {
    with_node_rows(shard, tag, |nodes, transitions| {
        for (id, mut props) in rows {
            write_work_item_props(
                nodes,
                transitions,
                GRAPH,
                id,
                &mut props,
                DurableCrypto::none(),
            )?;
        }
        Ok(())
    });
}

fn get(shard: &Shard, tenant: &str, id: &str) -> Option<WorkItemView> {
    read_work_item(shard, GRAPH, tenant, id, DurableCrypto::none()).unwrap()
}

fn list(limit: u32, cursor: Option<String>) -> WorkItemListRequest {
    WorkItemListRequest {
        tenant: "tenant-a".to_string(),
        cursor,
        limit,
        kind: None,
        metadata_match: None,
    }
}

#[test]
fn a_point_read_sees_only_its_own_tenant_and_every_write_is_a_new_revision() {
    let temp = open("point");
    write_rows(
        &temp.shard,
        "seed",
        vec![
            ("wi-a", work_item("tenant-a", "au.task", "ready")),
            ("wi-b", work_item("tenant-b", "au.task", "ready")),
        ],
    );
    let view = get(&temp.shard, "tenant-a", "wi-a").expect("own row is visible");
    assert_eq!(view.version, 1);
    assert_eq!(view.updated_at_ms, 2_500);
    assert_eq!(view.metadata["step"], 1);
    assert_eq!(
        get(&temp.shard, "tenant-a", "wi-b"),
        None,
        "cross-tenant read"
    );
    assert_eq!(get(&temp.shard, "tenant-a", "wi-missing"), None);

    // A second native write of the same row advances its revision.
    let mut rewritten = work_item("tenant-a", "au.task", "running");
    rewritten.insert("row_revision".into(), serde_json::json!(view.version));
    write_rows(&temp.shard, "rewrite", vec![("wi-a", rewritten)]);
    let view = get(&temp.shard, "tenant-a", "wi-a").unwrap();
    assert_eq!(view.version, 2);
    assert_eq!(view.status, WorkItemStatus::Running);
}

#[test]
fn listing_pages_resume_by_seek_and_skip_other_tenants_and_node_types() {
    let temp = open("list");
    let mut rows = Vec::new();
    for (id, tenant) in [
        ("wi-1", "tenant-a"),
        ("wi-2", "tenant-b"),
        ("wi-3", "tenant-a"),
        ("wi-4", "tenant-a"),
    ] {
        rows.push((id, work_item(tenant, "au.task", "ready")));
    }
    let mut document = work_item("tenant-a", "au.task", "ready");
    document.insert("node_type".into(), serde_json::json!("Document"));
    rows.push(("wi-0-doc", document));
    write_rows(&temp.shard, "seed", rows);

    let first = list_work_items(&temp.shard, GRAPH, &list(2, None), DurableCrypto::none()).unwrap();
    let ids: Vec<&str> = first
        .items
        .iter()
        .map(|item| item.work_item_id.as_str())
        .collect();
    assert_eq!(ids, ["wi-1", "wi-3"]);
    let cursor = first.next_cursor.clone().expect("a full page resumes");

    let second = list_work_items(
        &temp.shard,
        GRAPH,
        &list(2, Some(cursor.clone())),
        DurableCrypto::none(),
    )
    .unwrap();
    let ids: Vec<&str> = second
        .items
        .iter()
        .map(|item| item.work_item_id.as_str())
        .collect();
    assert_eq!(ids, ["wi-4"]);
    assert_eq!(second.next_cursor, None);

    let mut foreign = list(2, Some(cursor));
    foreign.tenant = "tenant-b".to_string();
    let error = list_work_items(&temp.shard, GRAPH, &foreign, DurableCrypto::none()).unwrap_err();
    assert!(error.contains("not minted for this tenant"), "{error}");
}

// spec: MEDULLA-DURABLE-KERNEL-R028.2
#[test]
fn an_outcome_read_returns_only_digest_verified_provenance_of_the_tenants_item() {
    use sha2::{Digest, Sha256};
    let temp = open("outcome");
    let evaluation = rmp_serde::to_vec_named(&serde_json::json!({
        "node_type": "OutcomeEvaluation",
        "status": "succeeded",
    }))
    .unwrap();
    let mut item = work_item("tenant-a", "browser.control.call", "succeeded");
    item.insert("outcome_ref".into(), "oe-1".into());
    item.insert(
        "outcome_digest".into(),
        hex::encode(Sha256::digest(&evaluation)).into(),
    );
    item.insert("trace_ref".into(), "rt-1".into());
    item.insert("tool_call_refs".into(), serde_json::json!(["tc-1"]));
    write_rows(&temp.shard, "item", vec![("wi-o", item)]);
    let receipt = evaluation.clone();
    with_nodes(&temp.shard, "receipt", |nodes| {
        nodes.insert((GRAPH, "oe-1"), receipt.as_slice())?;
        Ok(())
    });

    let read = |tenant: &str| {
        read_work_item_outcome(&temp.shard, GRAPH, tenant, "wi-o", DurableCrypto::none())
    };
    let outcome = read("tenant-a")
        .unwrap()
        .expect("committed outcome is visible");
    assert_eq!(
        outcome.terminal_event,
        medulla_work_types::work_item_read::TerminalEventRead::default()
    );
    assert_eq!(outcome.work_item.work_item_id, "wi-o");
    assert_eq!(
        (outcome.trace_ref.as_str(), outcome.outcome_ref.as_str()),
        ("rt-1", "oe-1")
    );
    assert_eq!(outcome.tool_call_refs, ["tc-1"]);
    assert_eq!(outcome.outcome.unwrap()["status"], "succeeded");
    assert_eq!(read("tenant-b").unwrap(), None, "cross-tenant outcome read");

    let forged = rmp_serde::to_vec_named(&serde_json::json!({"status": "failed"})).unwrap();
    with_nodes(&temp.shard, "forge", |nodes| {
        nodes.insert((GRAPH, "oe-1"), forged.as_slice())?;
        Ok(())
    });
    assert!(read("tenant-a").is_err(), "an altered receipt is refused");
}

// spec: MEDULLA-DURABLE-KERNEL-R028.2
#[test]
fn event_bindings_cannot_be_forged_and_missing_bound_event_is_not_legacy() {
    use medulla_work_types::{
        work_item_read::WORK_ITEM_EVENT_BATCH, work_item_read::WORK_ITEM_EVENT_DIGEST,
    };
    let temp = open("terminal-binding-refusal");
    let mut item = work_item("tenant-a", "au.task", "succeeded");
    item.insert("outcome_ref".into(), "oe".into());
    write_rows(&temp.shard, "seed", vec![("wi", item.clone())]);
    for key in [WORK_ITEM_EVENT_BATCH, WORK_ITEM_EVENT_DIGEST] {
        let method = Method::CompareAndSetNodeFields {
            node_id: "wi".into(),
            conditions_msgpack: rmp_serde::to_vec_named(&serde_json::json!({})).unwrap(),
            updates_msgpack: rmp_serde::to_vec_named(&serde_json::json!({key: "forged"})).unwrap(),
        };
        assert!(super::super::test_shard::check_guard(&temp.shard, key, method).is_err());
    }
    assert_eq!(
        read_work_item_outcome(&temp.shard, GRAPH, "tenant-a", "wi", DurableCrypto::none())
            .unwrap()
            .unwrap()
            .terminal_event,
        medulla_work_types::work_item_read::TerminalEventRead::default()
    );
    item.insert(WORK_ITEM_EVENT_BATCH.into(), "missing-batch".into());
    write_rows(&temp.shard, "partial", vec![("wi", item.clone())]);
    assert!(
        read_work_item_outcome(&temp.shard, GRAPH, "tenant-a", "wi", DurableCrypto::none())
            .unwrap_err()
            .contains("incomplete terminal event binding")
    );
    item.insert(WORK_ITEM_EVENT_DIGEST.into(), "a".repeat(64).into());
    write_rows(&temp.shard, "missing", vec![("wi", item)]);
    assert!(
        read_work_item_outcome(&temp.shard, GRAPH, "tenant-a", "wi", DurableCrypto::none())
            .unwrap_err()
            .contains("terminal receipt is missing")
    );
}

// spec: MEDULLA-DURABLE-KERNEL-R028.2
#[test]
fn a_bound_event_read_refuses_a_committed_batch_without_its_event() {
    use medulla_work_types::{
        work_item_read::WORK_ITEM_EVENT_BATCH, work_item_read::WORK_ITEM_EVENT_DIGEST,
    };
    let temp = open("empty-terminal-event");
    let mut item = work_item("tenant-a", "au.task", "succeeded");
    item.insert("outcome_ref".into(), "oe".into());
    // This is a real committed native maintenance batch, but it emitted no
    // terminal event. A bound missing event is corruption, not legacy data.
    write_rows(&temp.shard, "seed-event", vec![("wi", item.clone())]);
    let batch_id = {
        let handle = temp.shard.graph(GRAPH).unwrap();
        let read = temp.shard.read(&handle).unwrap();
        let batches = medulla_transaction::read_batches(&read).unwrap();
        assert_eq!(batches.len(), 1);
        batches[0].batch.batch_id.clone()
    };
    item.insert(WORK_ITEM_EVENT_BATCH.into(), batch_id.into());
    item.insert(WORK_ITEM_EVENT_DIGEST.into(), "a".repeat(64).into());
    write_rows(&temp.shard, "bind-event", vec![("wi", item)]);
    let error = read_work_item_outcome(&temp.shard, GRAPH, "tenant-a", "wi", DurableCrypto::none())
        .unwrap_err();
    assert!(error.contains("terminal event is missing"), "{error}");
}

// spec: MEDULLA-DURABLE-KERNEL-R028.2
#[test]
fn present_event_payload_corruption_is_refused() {
    use medulla_mutation_types::{
        mutation_scope::MutationOutboxIntent, mutation_scope::MutationOutboxRecord,
    };
    use sha2::{Digest, Sha256};
    let temp = open("corrupt-terminal-event");
    write_rows(
        &temp.shard,
        "seed",
        vec![("wi", work_item("tenant-a", "au.task", "succeeded"))],
    );
    let handle = temp.shard.graph(GRAPH).unwrap();
    let read = temp.shard.read(&handle).unwrap();
    let mut receipt = medulla_transaction::read_batches(&read).unwrap().remove(0);
    let intent = MutationOutboxIntent {
        topic: medulla_result_types::outcome_bundle::RUN_EVENT_OUTBOX_TOPIC.into(),
        key: "wi".into(),
        payload: vec![0xc1], // Reserved MessagePack byte: cannot encode a RunEvent.
        headers: Default::default(),
    };
    receipt.batch.outbox = vec![intent.clone()];
    let row = MutationOutboxRecord {
        schema_version: 1,
        batch_id: receipt.batch.batch_id.clone(),
        ordinal: 0,
        identity: receipt.identity.clone(),
        committed_version: receipt.committed_version,
        commit_sequence: receipt.committed_version.target(),
        intent,
        created_at_ms: receipt.committed_at_ms,
    };
    let check = |digest: &str| {
        terminal_event::project(
            read.scope(),
            "tenant-a",
            "wi",
            &receipt.batch.batch_id,
            digest,
            &receipt,
            std::slice::from_ref(&row),
        )
        .unwrap_err()
    };
    assert!(check(&"0".repeat(64)).contains("digest mismatch"));
    let actual_digest = hex::encode(Sha256::digest(&row.intent.payload));
    assert!(check(&actual_digest).contains("event is malformed"));
}

// spec: MEDULLA-DURABLE-KERNEL-R028.2
#[test]
fn a_generic_write_cannot_forge_the_outcome_digest_of_a_bound_item() {
    use sha2::{Digest, Sha256};
    let temp = open("forged-outcome-digest");
    let evaluation = rmp_serde::to_vec_named(&serde_json::json!({
        "node_type": "OutcomeEvaluation",
        "status": "succeeded",
    }))
    .unwrap();
    let mut item = work_item("tenant-a", "au.task", "succeeded");
    item.insert("outcome_ref".into(), "oe".into());
    item.insert(
        "outcome_digest".into(),
        hex::encode(Sha256::digest(&evaluation)).into(),
    );
    write_rows(&temp.shard, "seed", vec![("wi", item)]);
    let receipt = evaluation.clone();
    with_nodes(&temp.shard, "receipt", |nodes| {
        nodes.insert((GRAPH, "oe"), receipt.as_slice())?;
        Ok(())
    });
    let read =
        || read_work_item_outcome(&temp.shard, GRAPH, "tenant-a", "wi", DurableCrypto::none());
    assert!(
        read().unwrap().is_some(),
        "the bound outcome reads verified"
    );

    // A generic field CAS cannot rewrite the digest the terminal commit bound.
    let method = Method::CompareAndSetNodeFields {
        node_id: "wi".into(),
        conditions_msgpack: rmp_serde::to_vec_named(&serde_json::json!({})).unwrap(),
        updates_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
            "outcome_digest": "f".repeat(64),
        }))
        .unwrap(),
    };
    let error =
        super::super::test_shard::check_guard(&temp.shard, "outcome_digest", method).unwrap_err();
    assert!(
        error.contains("native WorkItem authority"),
        "a generic CAS must refuse the kernel-owned digest field: {error}"
    );
    assert!(
        read().unwrap().is_some(),
        "the refusal leaves the committed outcome binding intact"
    );
}

/// One item driven through submit, claim, two renewals and a terminal commit.
fn drive_one_item(shard: &Shard) -> String {
    use super::super::test_shard::{apply, run_request, submit_run_rows, with_write};
    use medulla_protocol_types::protocol::{Method, ResultPayload};
    use medulla_result_types::epistemic_operations::{
        ClaimWorkItemRequest, ClaimWorkItemRequestSchemaVersion, ClaimWorkItemResult,
    };

    let mut request = run_request();
    request.idempotency_key = "ledger-key".to_string();
    request.run_id = None;
    let id = with_write(shard, "submit/ledger", 1_000, |admitted| {
        submit_run_rows(admitted, &request, "owner")
    })
    .expect("the work item submits")
    .work_item_id;
    let claim = Method::ClaimWorkItem {
        request: ClaimWorkItemRequest {
            schema_version: ClaimWorkItemRequestSchemaVersion::V1,
            tenant_ref: "tenant".into(),
            work_item_id: Some(id.clone()),
            queue_ref: None,
            resource_class: None,
            fairness_group: None,
            worker_ref: "worker-1".into(),
            now_ms: 2_000,
            lease_ms: 60_000,
            max_tenant_in_flight: 64,
        },
    };
    let held: ClaimWorkItemResult = match apply(shard, "claim/ledger", claim, 2_000).unwrap() {
        ResultPayload::Raw(bytes) => decode_durable(&bytes).unwrap(),
        other => panic!("ClaimWorkItem answers a bin-encoded result, got {other:?}"),
    };
    let (epoch, token) = (held.lease_epoch.unwrap(), held.fencing_token.unwrap());
    // The first renewal starts the work; the second keeps `running` and appends nothing.
    for (index, now_ms) in [2_100_u64, 2_200].into_iter().enumerate() {
        let renew = Method::RenewWorkItemLease {
            tenant: "tenant".into(),
            work_item_id: id.clone(),
            worker_id: "worker-1".into(),
            lease_epoch: epoch,
            fencing_token: token,
            now_ms,
            lease_ms: 60_000,
        };
        apply(shard, &format!("renew/ledger-{index}"), renew, now_ms).unwrap();
    }
    let commit = Method::CommitWorkItemResult {
        tenant: "tenant".into(),
        work_item_id: id.clone(),
        worker_id: "worker-1".into(),
        lease_epoch: epoch,
        fencing_token: token,
        idempotency_key: "commit-ledger".into(),
        outcome: "succeeded".into(),
        result_ref: Some("cas:result:ledger".into()),
        error_ref: None,
        retryable: false,
        now_ms: 3_000,
        outcome_extension: None,
        run_completion: None,
    };
    apply(shard, "commit/ledger", commit, 3_000).unwrap();
    id
}

// spec: MEDULLA-DURABLE-KERNEL-R028.2.1
#[test]
fn transition_ledger_appends_on_status_change_only() {
    let temp = open("transition-ledger");
    let id = drive_one_item(&temp.shard);
    let page = |tenant: &str, cursor: Option<&str>, limit: u32| {
        list_work_item_transitions(
            &temp.shard,
            GRAPH,
            tenant,
            &id,
            cursor,
            limit,
            DurableCrypto::none(),
        )
    };

    // Exactly ready, leased, running, succeeded: the second renewal added none.
    let whole = page("tenant", None, 100)
        .unwrap()
        .expect("the item has a ledger");
    assert_eq!(whole.next_cursor, None);
    let states: Vec<_> = whole.entries.iter().map(|entry| entry.state).collect();
    assert_eq!(
        states,
        [
            WorkItemStatus::Ready,
            WorkItemStatus::Leased,
            WorkItemStatus::Running,
            WorkItemStatus::Succeeded,
        ]
    );
    let revisions: Vec<_> = whole.entries.iter().map(|entry| entry.revision).collect();
    assert!(
        revisions.windows(2).all(|pair| pair[0] < pair[1]),
        "revisions ascend in commit order: {revisions:?}"
    );

    // `limit 2` pages with a cursor and rejoins to the whole ledger.
    let first = page("tenant", None, 2).unwrap().unwrap();
    assert_eq!(first.entries, whole.entries[..2]);
    let cursor = first.next_cursor.expect("more entries remain");
    let second = page("tenant", Some(&cursor), 2).unwrap().unwrap();
    assert_eq!(second.entries, whole.entries[2..]);
    assert_eq!(second.next_cursor, None);

    // Another tenant, and an absent item, read `None`; a bound outside [1, 200] is refused.
    assert_eq!(page("tenant-b", None, 100).unwrap(), None);
    assert_eq!(
        list_work_item_transitions(
            &temp.shard,
            GRAPH,
            "tenant",
            "absent",
            None,
            100,
            DurableCrypto::none()
        )
        .unwrap(),
        None
    );
    assert!(page("tenant", None, 0)
        .unwrap_err()
        .starts_with("INVALID_PARAMS"));
    assert!(page("tenant", None, 201)
        .unwrap_err()
        .starts_with("INVALID_PARAMS"));
}
