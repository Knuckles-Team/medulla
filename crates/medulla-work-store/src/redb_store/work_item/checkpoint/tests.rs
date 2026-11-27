use super::super::test_shard::run_fixture::{admission, seed_claim, with_rows};
use super::super::test_shard::{open, with_node_rows, with_nodes, with_write, GRAPH};
use super::*;

const STALE_FENCE: &str =
    "CHECKPOINT_FENCE_STALE: checkpoint read requires the live lease epoch and fencing token";

fn request(expected_revision: Option<u64>) -> PublishRunCheckpointRequest {
    let content = serde_json::json!({"phase":"completed","next_step":expected_revision.unwrap_or(0),"history":["private prompt"],"usage":{}}).to_string();
    PublishRunCheckpointRequest {
        tenant: "tenant".into(),
        work_item_id: run_owner::work_item_id(GRAPH, "tenant", "run").unwrap(),
        run_id: "run".into(),
        context_id: "context".into(),
        worker_id: "worker".into(),
        lease_epoch: 1,
        fencing_token: 2,
        expected_revision,
        idempotency_key: format!("checkpoint-{expected_revision:?}"),
        snapshot: CheckpointContent {
            payload_digest: hex::encode(Sha256::digest(content.as_bytes())),
            content,
            short_term: vec![],
            long_term: vec![],
        },
    }
}
fn seed(shard: &Shard) {
    seed_admission(
        shard,
        admission(serde_json::json!({"a2a_context_id":"context"})),
    );
}

fn seed_admission(
    shard: &Shard,
    admitted: medulla_work_types::native_control::SubmitWorkItemRequest,
) {
    seed_claim(shard, |row| {
        let metadata = serde_json::to_value(&admitted.metadata).unwrap();
        row.insert("kind".into(), "orchestrator_task".into());
        row.insert("metadata".into(), metadata.clone());
        row.insert(
            medulla_work_types::work_item_read::RUN_ADMISSION_REQUEST.into(),
            serde_json::to_value(admitted).unwrap(),
        );
    });
}

pub fn apply(
    shard: &Shard,
    tag: &str,
    req: &PublishRunCheckpointRequest,
    actor: &str,
    now: u64,
) -> Result<RunCheckpointPublished, String> {
    with_rows(shard, tag, now, |rows| {
        publish(GRAPH, req, Some(actor), rows, DurableCrypto::none(), now)
    })
}
fn get(
    shard: &Shard,
    actor: &str,
    claim: bool,
    now: u64,
) -> Result<Option<RunCheckpointRead>, String> {
    let req = request(None);
    read_current(
        shard,
        GRAPH,
        &GetRunCheckpointRequest {
            tenant: req.tenant,
            work_item_id: req.work_item_id,
            run_id: req.run_id,
            context_id: req.context_id,
            current_claim: claim.then_some(CheckpointClaim {
                worker_id: "worker".into(),
                lease_epoch: 1,
                fencing_token: 2,
            }),
        },
        actor,
        DurableCrypto::none(),
        now,
    )
}
fn count(shard: &Shard) -> usize {
    let handle = shard.graph(GRAPH).unwrap();
    let read = shard.read(&handle).unwrap();
    read.scoped_owner_table(RUN_CHECKPOINTS)
        .unwrap()
        .scope_rows()
        .unwrap()
        .count()
}
// spec: MEDULLA-DURABLE-KERNEL-R028.1.1
#[test]
fn checkpoint_cas_is_atomic_private_and_owner_bound() {
    let tmp = open("checkpoint-roundtrip");
    let shard = &tmp.shard;
    seed(shard);
    assert!(get(shard, "owner", true, 2000).unwrap().is_none());
    let first = apply(shard, "first", &request(None), "owner", 2000).unwrap();
    assert_eq!(first.revision, 1);
    assert!(!first.replayed);
    let view = get(shard, "owner", true, 2000).unwrap().unwrap();
    assert_eq!(view.snapshot.content, request(None).snapshot.content);
    assert_eq!(view.work_item.metadata["a2a_context_id"], "context");
    assert!(get(shard, "foreign", false, 2000).unwrap().is_none());
    let before = view.clone();
    for (index, mut bad) in [
        request(None),
        request(Some(1)),
        request(Some(1)),
        request(Some(1)),
    ]
    .into_iter()
    .enumerate()
    {
        match index {
            1 => bad.worker_id = "other".into(),
            2 => bad.lease_epoch = 2,
            3 => bad.fencing_token = 3,
            _ => (),
        }
        let refusal = apply(shard, &format!("bad-{index}"), &bad, "owner", 2000).unwrap_err();
        assert!(refusal.starts_with("CONFLICT: "), "{refusal}");
        assert_eq!(
            get(shard, "owner", true, 2000).unwrap(),
            Some(before.clone())
        );
        assert_eq!(count(shard), 1);
    }
    assert!(apply(shard, "foreign", &request(Some(1)), "other", 2000).is_err());
    assert!(apply(shard, "expired", &request(Some(1)), "owner", 10000).is_err());
    apply(shard, "second", &request(Some(1)), "owner", 2000).unwrap();
    assert_eq!(count(shard), 2); // immutable prior revision remains retained
    let (_, row) = super::super::read::snapshot_row(
        shard,
        GRAPH,
        &request(None).work_item_id,
        DurableCrypto::none(),
    )
    .unwrap();
    assert!(!serde_json::to_string(&row)
        .unwrap()
        .contains("private prompt"));
    assert_eq!(
        get(shard, "owner", true, 2000)
            .unwrap()
            .unwrap()
            .snapshot
            .revision,
        2
    );
}
// spec: MEDULLA-DURABLE-KERNEL-R028.1.2.1
#[test]
fn sixty_fifth_revision_prunes_the_oldest_and_keeps_the_head() {
    let tmp = open("checkpoint-prune");
    let shard = &tmp.shard;
    seed(shard);
    let mut expected = None;
    for revision in 1..=(SHORT_TERM_MAX_REVISIONS as u64 + 1) {
        let published = apply(
            shard,
            &format!("prune-{revision}"),
            &request(expected),
            "owner",
            2000,
        )
        .unwrap();
        assert_eq!(published.revision, revision);
        expected = Some(revision);
        let retained = count(shard);
        assert_eq!(
            retained as u64,
            revision.min(SHORT_TERM_MAX_REVISIONS as u64)
        );
    }
    // 65 revisions published: revision 1 is gone, the head reads back.
    assert_eq!(count(shard), SHORT_TERM_MAX_REVISIONS);
    let head = get(shard, "owner", true, 2000).unwrap().unwrap();
    assert_eq!(head.snapshot.revision, SHORT_TERM_MAX_REVISIONS as u64 + 1);
    let (_, row) = super::super::read::snapshot_row(
        shard,
        GRAPH,
        &request(None).work_item_id,
        DurableCrypto::none(),
    )
    .unwrap();
    let row = row.expect("the WorkItem row");
    let retained: Vec<String> =
        serde_json::from_value(row.get(WORK_ITEM_CHECKPOINT_HISTORY).unwrap().clone()).unwrap();
    assert_eq!(retained.len(), SHORT_TERM_MAX_REVISIONS);
    assert_eq!(
        retained.last().map(String::as_str),
        row.get(WORK_ITEM_CHECKPOINT_REF)
            .and_then(|value| value.as_str()),
        "the head is the last retained reference"
    );
    // Completion deletes no revision.
    with_node_rows(shard, "terminal-prune", |nodes, transitions| {
        let id = request(None).work_item_id;
        let bytes = nodes.get((GRAPH, id.as_str()))?.unwrap();
        let mut row: Row = decode_durable(bytes.value())?;
        drop(bytes);
        row.insert("status".into(), "succeeded".into());
        write_work_item_props(
            nodes,
            transitions,
            GRAPH,
            &id,
            &mut row,
            DurableCrypto::none(),
        )
    });
    assert_eq!(count(shard), SHORT_TERM_MAX_REVISIONS);
    assert!(get(shard, "owner", false, 2000).unwrap().is_some());
}

// spec: MEDULLA-DURABLE-KERNEL-R028.1.1
#[test]
fn completed_history_survives_claim_expiry_but_resume_requires_live_fence() {
    let tmp = open("checkpoint-retention");
    let shard = &tmp.shard;
    seed(shard);
    apply(shard, "first", &request(None), "owner", 2000).unwrap();
    assert_eq!(get(shard, "owner", true, 10000).unwrap_err(), STALE_FENCE);
    assert!(get(shard, "owner", false, 10000).unwrap().is_some());
    with_node_rows(shard, "terminal", |nodes, transitions| {
        let id = request(None).work_item_id;
        let bytes = nodes.get((GRAPH, id.as_str()))?.unwrap();
        let mut row: Row = decode_durable(bytes.value())?;
        drop(bytes);
        row.insert("status".into(), "succeeded".into());
        write_work_item_props(
            nodes,
            transitions,
            GRAPH,
            &id,
            &mut row,
            DurableCrypto::none(),
        )
    });
    assert_eq!(get(shard, "owner", true, 2000).unwrap_err(), STALE_FENCE);
    assert!(get(shard, "owner", false, 2000).unwrap().is_some());
    assert_eq!(count(shard), 1);
}

// spec: MEDULLA-DURABLE-KERNEL-R028.1.1
#[test]
fn aborted_transaction_retains_neither_snapshot_nor_pointer() {
    let tmp = open("checkpoint-rollback");
    let shard = &tmp.shard;
    seed(shard);
    let failure: Result<(), String> = with_write(shard, "abort", 2000, |admitted| {
        let member = admitted.graph(GRAPH)?;
        let mut rows = crate::redb_store::graph_row_tables::GraphRowTables::open(member)?;
        publish(
            GRAPH,
            &request(None),
            Some("owner"),
            &mut rows,
            DurableCrypto::none(),
            2000,
        )?;
        Err("injected failure before atomic transaction commit".into())
    });
    assert!(failure.is_err());
    assert_eq!(count(shard), 0);
    assert!(get(shard, "owner", true, 2000).unwrap().is_none());
    apply(shard, "retry", &request(None), "owner", 2000).unwrap();
    assert_eq!(count(shard), 1);
}

// spec: MEDULLA-DURABLE-KERNEL-R028.1.1
#[test]
fn mutable_metadata_cannot_rebind_checkpoint_context() {
    let tmp = open("checkpoint-context-rebinding");
    let shard = &tmp.shard;
    seed(shard);
    apply(shard, "first", &request(None), "owner", 2000).unwrap();
    let id = request(None).work_item_id;
    with_node_rows(shard, "replace-metadata", |nodes, transitions| {
        let bytes = nodes.get((GRAPH, id.as_str()))?.unwrap();
        let mut row: Row = decode_durable(bytes.value())?;
        drop(bytes);
        row.insert(
            "metadata".into(),
            serde_json::json!({"a2a_context_id":"foreign"}),
        );
        write_work_item_props(
            nodes,
            transitions,
            GRAPH,
            &id,
            &mut row,
            DurableCrypto::none(),
        )
    });
    let valid = get(shard, "owner", true, 2000).unwrap().unwrap();
    assert_eq!(valid.context_id, "context");
    assert_eq!(valid.work_item.metadata["a2a_context_id"], "foreign");
    let mut foreign = request(Some(1));
    foreign.context_id = "foreign".into();
    assert!(apply(shard, "foreign-context", &foreign, "owner", 2000).is_err());
    let read = GetRunCheckpointRequest {
        tenant: foreign.tenant,
        work_item_id: foreign.work_item_id,
        run_id: foreign.run_id,
        context_id: foreign.context_id,
        current_claim: None,
    };
    assert!(!is_owner(shard, GRAPH, &read, "owner", DurableCrypto::none()).unwrap());
    assert!(
        read_current(shard, GRAPH, &read, "owner", DurableCrypto::none(), 2000)
            .unwrap()
            .is_none()
    );
    assert_eq!(get(shard, "owner", true, 2000).unwrap(), Some(valid));
    assert_eq!(count(shard), 1);
}

// spec: MEDULLA-DURABLE-KERNEL-R028.1.1
#[test]
fn checkpoint_rejects_missing_admission_and_non_work_item_owner_rows() {
    for remove_admission in [false, true] {
        let tmp = open("checkpoint-original-admission");
        let shard = &tmp.shard;
        seed(shard);
        let id = request(None).work_item_id;
        with_node_rows(shard, "invalidate-admission", |nodes, transitions| {
            let bytes = nodes.get((GRAPH, id.as_str()))?.unwrap();
            let mut row: Row = decode_durable(bytes.value())?;
            drop(bytes);
            if remove_admission {
                row.remove(medulla_work_types::work_item_read::RUN_ADMISSION_REQUEST);
            } else {
                row.insert("node_type".into(), "Other".into());
            }
            write_work_item_props(
                nodes,
                transitions,
                GRAPH,
                &id,
                &mut row,
                DurableCrypto::none(),
            )
        });
        assert!(apply(shard, "invalid", &request(None), "owner", 2000).is_err());
        assert!(get(shard, "owner", false, 2000).unwrap().is_none());
        assert_eq!(count(shard), 0);
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R028.1.1
#[test]
fn checkpoint_read_never_crosses_native_graph_tenant_or_run() {
    let tmp = open("checkpoint-scope");
    let shard = &tmp.shard;
    seed(shard);
    apply(shard, "first", &request(None), "owner", 2000).unwrap();
    for field in ["graph", "tenant", "run"] {
        let source = request(None);
        let mut read = GetRunCheckpointRequest {
            tenant: source.tenant,
            work_item_id: source.work_item_id,
            run_id: source.run_id,
            context_id: source.context_id,
            current_claim: None,
        };
        let graph = if field == "graph" {
            "foreign-graph"
        } else {
            GRAPH
        };
        if field == "tenant" {
            read.tenant = "foreign".into();
        }
        if field == "run" {
            read.run_id = "foreign".into();
        }
        assert!(
            read_current(shard, graph, &read, "owner", DurableCrypto::none(), 2000)
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(count(shard), 1);
}

fn typed_admission(
    metadata: serde_json::Value,
) -> medulla_work_types::native_control::SubmitWorkItemRequest {
    let mut request = admission(metadata);
    request.conversation = Some(Box::new(
        medulla_work_types::native_control::ConversationTurnAdmission {
            schema_version: medulla_work_types::native_control::NativeControlSchemaVersion::V1,
            context_id: "context".into(),
            expected_revision: 0,
            predecessor_run_id: None,
        },
    ));
    request
}

// spec: MEDULLA-DURABLE-KERNEL-R028.1.1
#[test]
fn checkpoint_typed_admission_is_owner_bound_with_optional_matching_legacy_context() {
    for metadata in [
        serde_json::json!({}),
        serde_json::json!({"a2a_context_id":"context"}),
    ] {
        let tmp = open("checkpoint-typed-context");
        let shard = &tmp.shard;
        seed_admission(shard, typed_admission(metadata));
        apply(shard, "first", &request(None), "owner", 2000).unwrap();
        let before = get(shard, "owner", true, 2000).unwrap().unwrap();
        assert_eq!(before.context_id, "context");
        assert!(get(shard, "foreign", false, 2000).unwrap().is_none());
        assert!(apply(shard, "foreign", &request(Some(1)), "foreign", 2000).is_err());
        assert_eq!(get(shard, "owner", true, 2000).unwrap(), Some(before));
        assert_eq!(count(shard), 1);
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R028.1.1
#[test]
fn conflicting_original_context_bindings_refuse_without_mutation() {
    for value in [
        serde_json::json!("foreign"),
        serde_json::json!(7),
        serde_json::Value::Null,
    ] {
        let tmp = open("checkpoint-conflicting-context");
        let shard = &tmp.shard;
        seed_admission(
            shard,
            typed_admission(serde_json::json!({"a2a_context_id":value})),
        );
        let id = request(None).work_item_id;
        let (_, before) =
            super::super::read::snapshot_row(shard, GRAPH, &id, DurableCrypto::none()).unwrap();
        assert!(get(shard, "owner", false, 2000).unwrap().is_none());
        for context in ["context", "foreign"] {
            let mut req = request(None);
            req.context_id = context.into();
            assert!(apply(shard, context, &req, "owner", 2000).is_err());
        }
        assert_eq!(count(shard), 0);
        assert_eq!(
            super::super::read::snapshot_row(shard, GRAPH, &id, DurableCrypto::none())
                .unwrap()
                .1,
            before
        );
    }
}

fn seed_memory_revision(shard: &Shard, graph_tag: &str, doc: &str, revision: u64, digest: &str) {
    let id = format!("memrev:{doc}@{revision}");
    let row = serde_json::json!({
        "label": "MemoryRevision",
        "head": {"doc_id": doc, "revision": revision, "content_digest": digest},
        "content": "memory",
    });
    let bytes = rmp_serde::to_vec_named(&row).unwrap();
    with_nodes(shard, graph_tag, |nodes| {
        nodes.insert((GRAPH, id.as_str()), bytes.as_slice())?;
        Ok(())
    });
}

fn with_short_term(
    refs: Vec<medulla_work_types::run_checkpoint::CheckpointMemoryRef>,
) -> PublishRunCheckpointRequest {
    let mut req = request(None);
    req.snapshot.short_term = refs;
    req
}

fn memory_ref(
    doc: &str,
    revision: u64,
    digest: &str,
) -> medulla_work_types::run_checkpoint::CheckpointMemoryRef {
    medulla_work_types::run_checkpoint::CheckpointMemoryRef {
        document_id: doc.into(),
        revision,
        digest: digest.into(),
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R028.1.2.2
#[test]
fn memory_refs_must_name_an_existing_revision_with_the_same_digest() {
    let tmp = open("checkpoint-memory-refs");
    let shard = &tmp.shard;
    seed(shard);
    let good = "a".repeat(64);
    seed_memory_revision(shard, "seed-memory", "notes/a.md", 1, &good);
    let absent = apply(
        shard,
        "absent",
        &with_short_term(vec![memory_ref("notes/absent.md", 1, &good)]),
        "owner",
        2000,
    )
    .unwrap_err();
    assert!(
        absent.starts_with("CHECKPOINT_MEMORY_REF_MISSING: "),
        "{absent}"
    );
    let wrong_revision = apply(
        shard,
        "wrong-revision",
        &with_short_term(vec![memory_ref("notes/a.md", 2, &good)]),
        "owner",
        2000,
    )
    .unwrap_err();
    assert!(
        wrong_revision.starts_with("CHECKPOINT_MEMORY_REF_MISSING: "),
        "{wrong_revision}"
    );
    let differing = apply(
        shard,
        "digest",
        &with_short_term(vec![memory_ref("notes/a.md", 1, &"b".repeat(64))]),
        "owner",
        2000,
    )
    .unwrap_err();
    assert!(
        differing.starts_with("CHECKPOINT_DIGEST_MISMATCH: "),
        "{differing}"
    );
    assert_eq!(count(shard), 0, "no row changes on refusal");
    assert!(get(shard, "owner", false, 2000).unwrap().is_none());
    apply(
        shard,
        "ok",
        &with_short_term(vec![memory_ref("notes/a.md", 1, &good)]),
        "owner",
        2000,
    )
    .unwrap();
    assert_eq!(count(shard), 1);
}
