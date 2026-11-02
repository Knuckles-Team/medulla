use medulla_store::{redb_store::decode_durable, redb_store::property_string, redb_store::DurableCrypto, redb_store::NODES};
use medulla_result_types::epistemic_operations_ext::WorkItemClaimCapabilityDecision;
use medulla_result_types::epistemic_operations_ext::WorkItemClaimCapabilityMintRequest;
use medulla_result_types::epistemic_operations_ext::WorkItemClaimCapabilityRequestSchemaVersion;
use medulla_result_types::epistemic_operations_ext::WorkItemClaimCapabilityResult;
use medulla_result_types::epistemic_operations_ext::WorkItemClaimCapabilityResultSchemaVersion;
use medulla_result_types::epistemic_operations_ext::WorkItemClaimCapabilityVerifyRequest;
use medulla_storage::{GraphShardOwner, PhysicalWriteCapability, ScopedOwnerTableMut};
use medulla_transaction::{AdmittedOwnerWrite, OwnerPayloadWrite};
use rand::RngCore;
use redb::TableDefinition;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use medulla_store::redb_store::shard::ShardWrite;
use medulla_work_store::redb_store::work_item_capability::lease::{read_live_lease, record_matches_live};
use medulla_work_store::redb_store::work_item_capability::authority::{bounded_text, validate_authority};
use medulla_store::redb_store::store_prelude::*;
use medulla_store::redb_store::*;
use medulla_work_store::redb_store::work_item_capability::*;
use super::*;
use medulla_store::redb_store::shard::Shard;
use medulla_mutation_types::mutation_scope::IncarnationId;
use medulla_mutation_types::mutation_scope::LogicalName;
use medulla_mutation_types::mutation_scope::MutationScopeIdentity;
use medulla_mutation_types::mutation_scope::ScopeTenantId;
use medulla_protocol_types::mutation_batch::MutationBatch;
use medulla_protocol_types::protocol::Method;
use std::path::{Path, PathBuf};

/// One process-unique admission id per ATTEMPT, exactly as the production
/// write paths require: a shard batch id is durable, so two admissions in
/// one run -- or one run and its restart -- may never share one.
fn attempt(tag: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{tag}-{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn temp_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "medulla-work-item-capability-{tag}-{}-{}.redb",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    ))
}

/// `create_owner` materializes the whole declared shard census, so there is
/// no hand-written table bootstrap to run here any more.
pub fn open(path: &Path) -> Shard {
    Shard::open(path).expect("open capability test shard")
}

/// The graph's own authoritative version, read from the kernel ledger
/// rather than from a private version row.
fn graph_version(shard: &Shard) -> u64 {
    let handle = shard.graph("graph-a").expect("bind graph-a");
    let read = shard.read(&handle).expect("read graph-a");
    medulla_transaction::version(&read).expect("graph-a authoritative version")
}

fn try_commit_method(shard: &Shard, method: Method) -> Result<(), String> {
    let mut ops = vec![("graph-a".to_string(), method)];
    let mut raft_log_ops = Vec::new();
    #[cfg(feature = "security")]
    let mut audit_tail = medulla_store::redb_store::AuditTailCache::new();
    medulla_store_apply::redb_store::store_control::commit_ops(
        shard,
        &mut ops,
        &mut raft_log_ops,
        &attempt("native-method"),
        0,
        DurableCrypto::none(),
        #[cfg(feature = "security")]
        &mut audit_tail,
    )
}

fn commit_method(shard: &Shard, method: Method) {
    try_commit_method(shard, method).expect("public native graph mutation");
}

fn sealed_work_item_batch(
    method: Method,
    prefix: &str,
    key: &str,
    now_ms: u64,
    expected_graph_version: u64,
) -> MutationBatch {
    // Use the caller scope; the shard's reserved tenant is bound only at
    // admission, never supplied by this fixture.
    let identity = MutationScopeIdentity::graph(
        ScopeTenantId::new("tenant-a").expect("valid caller tenant"),
        LogicalName::new("graph-a").expect("valid graph name"),
        IncarnationId::new("incarnation:test:work-item-capability").expect("valid incarnation"),
    );
    medulla_store::redb_store::control_fixture_batch(medulla_store::redb_store::ControlFixtureBatch {
        identity,
        batch_id: format!("{prefix}-{key}"),
        actor: format!("principal:sha256:{}", "a".repeat(64)),
        request_id: now_ms,
        idempotency_key: format!("{prefix}-key-{key}"),
        expected_version: expected_graph_version,
        method,
        created_at_ms: now_ms,
    })
}

fn commit_work_item_batch(shard: &Shard, batch: &MutationBatch, now_ms: u64) -> bool {
    #[cfg(feature = "security")]
    let mut audit_tail = medulla_store::redb_store::AuditTailCache::new();
    medulla_store_apply::redb_store::store_mutation::commit_mutation_batch_inner(
        shard,
        medulla_store_apply::redb_store::store_mutation::BatchCommitInput::compact("graph-a", batch, None, now_ms),
        DurableCrypto::none(),
        #[cfg(feature = "security")]
        &mut audit_tail,
    )
    .expect("native WorkItem fixture mutation")
    .replayed
}

fn claim_native(shard: &Shard, id: &str, worker: &str, now_ms: u64, key: &str) {
    let expected_graph_version = medulla_store::redb_store::store_read::read_mutation_graph_version(shard, "graph-a")
        .expect("read mutation graph version");
    let batch = sealed_work_item_batch(
        Method::ClaimWorkItem {
            request: medulla_result_types::epistemic_operations::ClaimWorkItemRequest {
                schema_version:
                    medulla_result_types::epistemic_operations::ClaimWorkItemRequestSchemaVersion::V1,
                tenant_ref: "tenant-a".to_string(),
                work_item_id: Some(id.to_string()),
                queue_ref: None,
                resource_class: None,
                fairness_group: None,
                worker_ref: worker.to_string(),
                now_ms,
                lease_ms: 100_000,
                max_tenant_in_flight: 64,
            },
        },
        "claim",
        key,
        now_ms,
        expected_graph_version,
    );
    assert!(
        !commit_work_item_batch(shard, &batch, now_ms),
        "claim fixture must be a fresh mutation"
    );
}

fn seed_work_item(shard: &Shard, id: &str) {
    commit_method(
        shard,
        Method::AddNode {
            node_id: id.to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                "node_type": "WorkItem",
                "tenant": "tenant-a",
                "kind": "generic",
                "status": "ready",
            }))
            .expect("encode WorkItem"),
        },
    );
    claim_native(shard, id, "worker-a", 10_000, &format!("seed-{id}"));
}

fn authority(now_ms: u64) -> AuthenticatedAuthority {
    AuthenticatedAuthority {
        tenant: "tenant-a".to_string(),
        audience: "graph-os".to_string(),
        principal: format!("principal:sha256:{}", "a".repeat(64)),
        agent_id: "worker-a".to_string(),
        session: "session-a".to_string(),
        authority_epoch: 7,
        incarnation_id: "incarnation-a".to_string(),
        now_ms,
    }
}

fn mint(
    shard: &Shard,
    item: &str,
    authority: &AuthenticatedAuthority,
) -> WorkItemClaimCapabilityResult {
    let op_id = attempt("capability-mint");
    let members = shard.graph_members(&["graph-a"]).expect("bind graph-a");
    let (group, batches) = shard
        .admit_maintenance(&members, &op_id)
        .expect("admit capability mint");
    let write =
        ShardWrite::open(shard, &group, &members, &batches).expect("open capability mint");
    let result = mint_claim_capability(
        &write,
        "graph-a",
        &WorkItemClaimCapabilityMintRequest {
            schema_version: WorkItemClaimCapabilityRequestSchemaVersion::V1,
            work_item_id: item.to_string(),
        },
        authority,
        DurableCrypto::none(),
    )
    .expect("native mint result");
    write.finish().expect("finish capability mint");
    shard
        .commit_drain(group, &batches, authority.now_ms)
        .expect("commit capability mint");
    result
}

fn verify(
    shard: &Shard,
    item: &str,
    capability: Vec<u8>,
    authority: &AuthenticatedAuthority,
) -> WorkItemClaimCapabilityResult {
    let op_id = attempt("capability-verify");
    let members = shard.graph_members(&["graph-a"]).expect("bind graph-a");
    let (group, batches) = shard
        .admit_maintenance(&members, &op_id)
        .expect("admit capability verify");
    let write =
        ShardWrite::open(shard, &group, &members, &batches).expect("open capability verify");
    let result = verify_claim_capability(
        &write,
        "graph-a",
        &WorkItemClaimCapabilityVerifyRequest {
            schema_version: WorkItemClaimCapabilityRequestSchemaVersion::V1,
            work_item_id: item.to_string(),
            capability,
        },
        authority,
        DurableCrypto::none(),
    )
    .expect("native verify result");
    write.finish().expect("finish capability verify");
    shard
        .commit_drain(group, &batches, authority.now_ms)
        .expect("commit capability verify");
    result
}

// Test-only helper: every parameter is an independent field of the
// committed result being asserted on; no natural grouping applies.
#[allow(clippy::too_many_arguments)]
fn commit_native_result(
    shard: &Shard,
    item: &str,
    lease_epoch: u64,
    fencing_token: u64,
    now_ms: u64,
    key: &str,
    outcome: &str,
    retryable: bool,
) {
    let expected_graph_version = medulla_store::redb_store::store_read::read_mutation_graph_version(shard, "graph-a")
        .expect("read mutation graph version");
    let batch = sealed_work_item_batch(
        Method::CommitWorkItemResult {
            tenant: "tenant-a".to_string(),
            work_item_id: item.to_string(),
            worker_id: "worker-a".to_string(),
            lease_epoch,
            fencing_token,
            idempotency_key: format!("work-result-{key}"),
            outcome: outcome.to_string(),
            result_ref: None,
            outcome_extension: None,
            run_completion: None,
            error_ref: None,
            retryable,
            now_ms,
        },
        "result",
        key,
        now_ms,
        expected_graph_version,
    );
    commit_work_item_batch(shard, &batch, now_ms);
}

/// A scoped table has no `len()` -- it would have to answer for every graph
/// in the file -- so the count is this graph's own rows, walked.
fn private_row_counts(shard: &Shard) -> (usize, usize, usize) {
    let handle = shard.graph("graph-a").expect("bind graph-a");
    let read = shard.read(&handle).expect("read graph-a");
    let count = |table: TableDefinition<'static, (&str, &str), &[u8]>| -> usize {
        read.scoped_owner_table(table)
            .expect("open private table")
            .scope_rows()
            .expect("scan private table")
            .fold(0, |acc, row| {
                row.expect("private row");
                acc + 1
            })
    };
    (
        count(CAPABILITIES),
        count(INVOCATIONS),
        count(NATIVE_WORK_ITEMS),
    )
}

#[test]
fn generic_work_item_authority_forgery_is_rejected_before_private_state() {
    let path = temp_path("generic-forgery");
    let shard = open(&path);
    let forged = serde_json::json!({
        "node_type": "WorkItem",
        "kind": "generic",
        "status": "leased",
        "tenant": "tenant-a",
        "lease_owner": "worker-a",
        "last_lease_owner": "worker-a",
        "worker_ref": "worker-a",
        "session": "session-a",
        "idempotency_key": "forged-key",
        "lease_epoch": 4,
        "fencing_token": 5,
        "work_item_fence": "forged-fence",
        "lease_expires_at": 99_999.0,
        "authority_epoch": 7,
        "incarnation_id": "forged-incarnation",
        "attempt": 2,
        "metadata": {"private": "forged"}
    });
    let error = try_commit_method(
        &shard,
        Method::AddNode {
            node_id: "forged".to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&forged).expect("encode forged row"),
        },
    )
    .expect_err("generic active WorkItem AddNode must refuse");
    assert!(error.contains("native WorkItem authority"));
    let batch_forgery = serde_json::json!([{
        "op": "add_node",
        "id": "forged-batch",
        "properties": forged,
    }]);
    let error = try_commit_method(
        &shard,
        Method::BatchUpdate {
            operations_msgpack: rmp_serde::to_vec_named(&batch_forgery)
                .expect("encode forged batch"),
        },
    )
    .expect_err("generic active WorkItem BatchUpdate must refuse");
    assert!(error.contains("native WorkItem authority"));
    assert!(medulla_store::redb_store::store_read::read_one_node(
        &shard,
        "graph-a",
        "forged-batch",
        DurableCrypto::none()
    )
    .expect("read forged batch row")
    .is_none());
    assert!(
        medulla_store::redb_store::store_read::read_one_node(&shard, "graph-a", "forged", DurableCrypto::none())
            .expect("read forged row")
            .is_none()
    );
    assert_eq!(private_row_counts(&shard), (0, 0, 0));

    seed_work_item(&shard, "wi-guard");
    let before =
        medulla_store::redb_store::store_read::read_one_node(&shard, "graph-a", "wi-guard", DurableCrypto::none())
            .expect("read native WorkItem")
            .expect("native WorkItem exists");
    let error = try_commit_method(
        &shard,
        Method::CompareAndSetNodeFields {
            node_id: "wi-guard".to_string(),
            conditions_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                "status": "leased"
            }))
            .expect("encode CAS conditions"),
            updates_msgpack: rmp_serde::to_vec_named(&forged).expect("encode forged update"),
        },
    )
    .expect_err("generic authority CAS must refuse");
    assert!(error.contains("native WorkItem authority"));
    let after =
        medulla_store::redb_store::store_read::read_one_node(&shard, "graph-a", "wi-guard", DurableCrypto::none())
            .expect("read native WorkItem after refusal")
            .expect("native WorkItem remains");
    assert_eq!(
        before, after,
        "refused CAS must not partially mutate the row"
    );

    assert!(try_commit_method(
        &shard,
        Method::RemoveNode {
            node_id: "wi-guard".to_string()
        }
    )
    .is_err());
    assert!(try_commit_method(
        &shard,
        Method::SetPose {
            node_id: "wi-guard".to_string(),
            pose_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                "translation": [1.0, 2.0, 3.0]
            }))
            .expect("encode pose")
        }
    )
    .is_err());
    assert_eq!(private_row_counts(&shard).0, 0);
    drop(shard);
    let _ = std::fs::remove_file(path);
}

#[test]
fn graph_clear_purges_capability_invocation_and_native_provenance_rows_atomically() {
    let path = temp_path("graph-clear");
    let shard = open(&path);
    commit_method(
        &shard,
        Method::AddNode {
            node_id: "wi-clear".to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                "node_type": "WorkItem",
                "tenant": "tenant-a",
                "kind": "generic",
                "status": "ready",
            }))
            .expect("encode replacement WorkItem"),
        },
    );
    claim_native(&shard, "wi-clear", "worker-a", 10_000, "replacement");
    let first = mint(&shard, "wi-clear", &authority(10_000));
    assert_eq!(first.decision, WorkItemClaimCapabilityDecision::Minted);
    assert_eq!(private_row_counts(&shard), (1, 1, 1));

    commit_method(&shard, Method::ClearGraph);
    assert_eq!(private_row_counts(&shard), (0, 0, 0));
    assert!(
        medulla_store::redb_store::store_read::read_one_node(&shard, "graph-a", "wi-clear", DurableCrypto::none())
            .expect("read cleared WorkItem")
            .is_none()
    );

    // Reusing the graph/item id after a clear requires a new native claim;
    // the old opaque bytes cannot be replayed against the replacement row.
    commit_method(
        &shard,
        Method::AddNode {
            node_id: "wi-clear".to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                "node_type": "WorkItem",
                "tenant": "tenant-a",
                "kind": "generic",
                "status": "ready",
            }))
            .expect("encode restored WorkItem"),
        },
    );
    claim_native(&shard, "wi-clear", "worker-a", 10_000, "restored");
    let second = mint(&shard, "wi-clear", &authority(10_000));
    assert_eq!(second.decision, WorkItemClaimCapabilityDecision::Minted);
    assert_ne!(second.capability, first.capability);
    drop(shard);
    let _ = std::fs::remove_file(path);
}

#[test]
fn checkpoint_replacement_purges_private_capability_state_and_requires_new_claim() {
    let path = temp_path("checkpoint-replacement");
    let shard = open(&path);
    seed_work_item(&shard, "wi-checkpoint");
    let owner = authority(10_000);
    let first = mint(&shard, "wi-checkpoint", &owner);
    assert_eq!(first.decision, WorkItemClaimCapabilityDecision::Minted);
    let old_capability = first.capability.clone().expect("checkpoint capability");
    assert_eq!(private_row_counts(&shard), (1, 1, 1));

    // A checkpoint is an untrusted public graph image.  Convert the
    // leased image to a fresh ready submission before restoring it; the
    // restore path must never preserve an old lease or its private rows.
    let dump = medulla_store_apply::redb_store::dump::read_graph_dump(&shard, "graph-a", DurableCrypto::none())
        .expect("read graph checkpoint")
        .expect("graph checkpoint identity");
    let invalid = medulla_store_apply::redb_store::store_control::GraphDump::in_place_core_checkpoint(
        medulla_store_apply::redb_store::store_control::InPlaceCoreCheckpoint {
            graph: dump.graph.clone(),
            name: dump.name.clone(),
            graph_type: dump.graph_type,
            incarnation_id: "incarnation:checkpoint-invalid".to_string(),
            source_snapshot_version: dump.source_snapshot_version.saturating_add(1),
            schema_sources: std::sync::Arc::clone(&dump.schema_sources),
            nodes: dump.nodes.clone(),
            edges: dump.edges.clone(),
            ledger: dump.ledger.clone(),
            semantic: dump.semantic.clone(),
        },
    );
    let error = medulla_store_apply::redb_store::checkpoint::apply_checkpoint(
        &shard,
        &mut Vec::new(),
        vec![invalid],
        DurableCrypto::none(),
    )
    .expect_err("checkpoint cannot manufacture an active WorkItem image");
    assert!(error.contains("native WorkItem authority"));
    assert_eq!(
        private_row_counts(&shard),
        (1, 1, 1),
        "rejected checkpoint must not purge private state"
    );
    let nodes = dump
        .nodes
        .into_iter()
        .map(|(id, bytes)| {
            let mut props: serde_json::Map<String, serde_json::Value> =
                decode_durable(&bytes).expect("decode checkpoint WorkItem");
            props.insert("status".into(), serde_json::Value::String("ready".into()));
            for key in [
                "lease_owner",
                "last_lease_owner",
                "worker_ref",
                "session",
                "idempotency_key",
                "lease_epoch",
                "fencing_token",
                "work_item_fence",
                "lease_expires_at",
                "authority_epoch",
                "incarnation_id",
                "attempt",
            ] {
                props.remove(key);
            }
            (
                id,
                rmp_serde::to_vec_named(&props).expect("encode restored WorkItem"),
            )
        })
        .collect();
    let restored_incarnation = "incarnation:checkpoint-replacement";
    let restored = medulla_store_apply::redb_store::store_control::GraphDump::in_place_core_checkpoint(
        medulla_store_apply::redb_store::store_control::InPlaceCoreCheckpoint {
            graph: dump.graph,
            name: dump.name,
            graph_type: dump.graph_type,
            incarnation_id: restored_incarnation.to_string(),
            source_snapshot_version: dump.source_snapshot_version.saturating_add(1),
            schema_sources: dump.schema_sources,
            nodes,
            edges: dump.edges,
            ledger: dump.ledger,
            semantic: dump.semantic,
        },
    );
    medulla_store_apply::redb_store::checkpoint::apply_checkpoint(
        &shard,
        &mut Vec::new(),
        vec![restored],
        DurableCrypto::none(),
    )
    .expect("checkpoint replacement commits atomically");
    assert_eq!(private_row_counts(&shard), (0, 0, 0));
    assert_eq!(
        verify(&shard, "wi-checkpoint", old_capability, &owner).decision,
        WorkItemClaimCapabilityDecision::Unauthorized,
        "a capability cannot survive graph replacement"
    );

    let mut replacement_owner = authority(30_000);
    replacement_owner.incarnation_id = restored_incarnation.to_string();
    claim_native(
        &shard,
        "wi-checkpoint",
        "worker-a",
        30_000,
        "checkpoint-reclaim",
    );
    let second = mint(&shard, "wi-checkpoint", &replacement_owner);
    assert_eq!(second.decision, WorkItemClaimCapabilityDecision::Minted);
    assert_ne!(second.capability, first.capability);
    assert_eq!(private_row_counts(&shard), (1, 1, 1));

    drop(shard);
    let _ = std::fs::remove_file(path);
}

#[test]
fn crossmodal_clear_and_delete_purge_private_capability_state_atomically() {
    let path = temp_path("crossmodal-lifecycle");
    let shard = open(&path);
    commit_method(
        &shard,
        Method::AddNode {
            node_id: "wi-crossmodal".to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                "node_type": "WorkItem",
                "tenant": "tenant-a",
                "kind": "generic",
                "status": "ready",
            }))
            .expect("encode replacement WorkItem"),
        },
    );
    claim_native(
        &shard,
        "wi-crossmodal",
        "worker-a",
        10_000,
        "crossmodal-delete",
    );
    assert_eq!(
        mint(&shard, "wi-crossmodal", &authority(10_000)).decision,
        WorkItemClaimCapabilityDecision::Minted
    );
    let before_blob_ref = private_row_counts(&shard);
    let blob_refs = [("wi-crossmodal".to_string(), "sha256:forged".to_string())];
    let error = {
        #[cfg(feature = "security")]
        let mut audit_tail = medulla_store::redb_store::AuditTailCache::new();
        medulla_store_apply::redb_store::crossmodal::commit_crossmodal(
            &shard,
            "graph-a",
            medulla_store_apply::redb_store::crossmodal::CrossModalStaged {
                blob_refs: &blob_refs,
                ..Default::default()
            },
            &attempt("crossmodal-refuse"),
            10_000,
            DurableCrypto::none(),
            #[cfg(feature = "security")]
            &mut audit_tail,
        )
    }
    .expect_err("generic crossmodal blob mutation cannot touch a WorkItem");
    assert!(error.contains("native WorkItem authority"));
    assert_eq!(private_row_counts(&shard), before_blob_ref);
    let props =
        medulla_store::redb_store::store_read::read_one_node(&shard, "graph-a", "wi-crossmodal", DurableCrypto::none())
            .expect("read WorkItem after refused blob mutation")
            .expect("WorkItem remains after refused blob mutation");
    let props: serde_json::Map<String, serde_json::Value> =
        decode_durable(&props).expect("decode WorkItem after refused blob mutation");
    assert!(!props.contains_key("__blob__"));
    #[cfg(feature = "security")]
    let mut audit_tail = medulla_store::redb_store::AuditTailCache::new();
    medulla_store_apply::redb_store::crossmodal::commit_crossmodal(
        &shard,
        "graph-a",
        medulla_store_apply::redb_store::crossmodal::CrossModalStaged {
            methods: &[Method::ClearGraph],
            ..Default::default()
        },
        &attempt("crossmodal-clear"),
        10_000,
        DurableCrypto::none(),
        #[cfg(feature = "security")]
        &mut audit_tail,
    )
    .expect("crossmodal ClearGraph commits");
    assert_eq!(private_row_counts(&shard), (0, 0, 0));

    commit_method(
        &shard,
        Method::AddNode {
            node_id: "wi-crossmodal".to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                "node_type": "WorkItem",
                "tenant": "tenant-a",
                "kind": "generic",
                "status": "ready",
            }))
            .expect("encode replacement WorkItem"),
        },
    );
    claim_native(
        &shard,
        "wi-crossmodal",
        "worker-a",
        10_000,
        "crossmodal-delete-recreated",
    );
    assert_eq!(
        mint(&shard, "wi-crossmodal", &authority(10_000)).decision,
        WorkItemClaimCapabilityDecision::Minted
    );
    #[cfg(feature = "security")]
    let mut delete_audit_tail = medulla_store::redb_store::AuditTailCache::new();
    medulla_store_apply::redb_store::crossmodal::commit_crossmodal(
        &shard,
        "graph-a",
        medulla_store_apply::redb_store::crossmodal::CrossModalStaged {
            methods: &[Method::DeleteGraph {
                graph_name: "graph-a".to_string(),
            }],
            ..Default::default()
        },
        &attempt("crossmodal-delete"),
        10_000,
        DurableCrypto::none(),
        #[cfg(feature = "security")]
        &mut delete_audit_tail,
    )
    .expect("crossmodal DeleteGraph commits");
    assert_eq!(private_row_counts(&shard), (0, 0, 0));

    drop(shard);
    let _ = std::fs::remove_file(path);
}

#[test]
fn expired_private_rows_are_pruned_with_a_bounded_scan_before_new_mint() {
    let path = temp_path("cleanup");
    let shard = open(&path);
    seed_work_item(&shard, "wi-expired");
    let owner = authority(10_000);
    let first = mint(&shard, "wi-expired", &owner);
    let old_capability = first.capability.expect("initial capability");
    assert_eq!(private_row_counts(&shard), (1, 1, 1));

    commit_method(
        &shard,
        Method::AddNode {
            node_id: "wi-fresh".to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                "node_type": "WorkItem",
                "tenant": "tenant-a",
                "kind": "generic",
                "status": "ready",
            }))
            .expect("encode fresh WorkItem"),
        },
    );
    claim_native(&shard, "wi-fresh", "worker-a", 200_000, "cleanup-fresh");
    let mut fresh_owner = authority(200_000);
    fresh_owner.session = "session-b".to_string();
    let fresh = mint(&shard, "wi-fresh", &fresh_owner);
    assert_eq!(fresh.decision, WorkItemClaimCapabilityDecision::Minted);
    assert_eq!(
        private_row_counts(&shard),
        (1, 1, 2),
        "expired capability and invocation rows are pruned before mint"
    );
    assert_eq!(
        verify(&shard, "wi-expired", old_capability, &owner).decision,
        WorkItemClaimCapabilityDecision::Unauthorized
    );

    drop(shard);
    let _ = std::fs::remove_file(path);
}

#[test]
fn capability_is_opaque_context_bound_restart_persistent_and_fail_closed() {
    let path = temp_path("lifecycle");
    let capability = {
        let shard = open(&path);
        seed_work_item(&shard, "wi-1");
        seed_work_item(&shard, "wi-2");
        let owner = authority(10_000);
        let minted = mint(&shard, "wi-1", &owner);
        assert_eq!(minted.decision, WorkItemClaimCapabilityDecision::Minted);
        assert!(minted.valid);
        let capability = minted.capability.expect("opaque capability bytes");
        assert_eq!(capability.len(), CAPABILITY_BYTES);
        assert_eq!(&capability[..CAPABILITY_MAGIC.len()], CAPABILITY_MAGIC);
        assert_ne!(
            &capability[CAPABILITY_MAGIC.len()..],
            &[0_u8; CAPABILITY_NONCE_BYTES]
        );

        let verified = verify(&shard, "wi-1", capability.clone(), &owner);
        assert_eq!(verified.decision, WorkItemClaimCapabilityDecision::Verified);
        assert!(verified.valid);
        assert!(
            verified.capability.is_none(),
            "verify never re-exports the nonce"
        );

        // Lost-ack retry: same authenticated session and live lease returns
        // exactly the persisted opaque bytes, not a second nonce.
        let replayed = mint(&shard, "wi-1", &authority(10_001));
        assert_eq!(replayed.decision, WorkItemClaimCapabilityDecision::Replayed);
        assert_eq!(replayed.capability, Some(capability.clone()));

        // A changed input on the same idempotency session is rejected, even
        // though the second WorkItem is a valid live lease.
        let conflict = mint(&shard, "wi-2", &owner);
        assert_eq!(
            conflict.decision,
            WorkItemClaimCapabilityDecision::InputConflict
        );

        // Scope/principal/session/owner/graph mismatches never verify the
        // copied opaque bytes or disclose private-row state.
        let assert_denied = |result: WorkItemClaimCapabilityResult| {
            assert_eq!(
                result.decision,
                WorkItemClaimCapabilityDecision::Unauthorized,
                "all foreign/invalid verification outcomes are indistinguishable"
            );
            assert!(!result.valid);
            assert!(result.capability.is_none());
        };

        // A caller who does not currently own the live public lease for
        // this WorkItem (wrong tenant/owner, an unclaimed/unknown item,
        // or capability bytes that fail the cheap bounds/magic check
        // before any table lookup) is refused by `read_live_lease` alone.
        // The private CAPABILITIES/INVOCATIONS tables must never be
        // opened for these — this is the ordering RMDD-29 requires and
        // the counter below fails the test if that ordering regresses.
        reset_private_work_item_body_reads();
        let malformed = verify(&shard, "wi-1", vec![0_u8; MAX_CAPABILITY_BYTES + 1], &owner);
        assert_denied(malformed);
        assert_denied(verify(&shard, "wi-1", vec![1, 2, 3], &owner));
        assert_denied(verify(
            &shard,
            "unknown-work-item",
            capability.clone(),
            &owner,
        ));
        let mut wrong_tenant = owner.clone();
        wrong_tenant.tenant = "tenant-b".to_string();
        assert_denied(verify(&shard, "wi-1", capability.clone(), &wrong_tenant));
        let mut wrong_owner = owner.clone();
        wrong_owner.agent_id = "worker-b".to_string();
        assert_denied(verify(&shard, "wi-1", capability.clone(), &wrong_owner));
        assert_eq!(
            private_work_item_body_reads(),
            0,
            "a caller who does not own the live public lease must be refused \
             before the private capability row is ever opened"
        );

        // A caller who DOES own the live public lease (tenant + lease
        // owner match) but presents a capability minted for a different
        // audience/principal/session/authority-epoch/incarnation still
        // cannot verify — but this final check is inherently a private
        // record comparison (those fields are not stored on the public
        // WorkItem row at all), so it legitimately touches the private
        // row once base lease ownership is already established. The
        // outcome remains the same normalized denial either way.
        let mut wrong_audience = owner.clone();
        wrong_audience.audience = "other-audience".to_string();
        assert_denied(verify(&shard, "wi-1", capability.clone(), &wrong_audience));
        let mut wrong_principal = owner.clone();
        wrong_principal.principal = format!("principal:sha256:{}", "b".repeat(64));
        assert_denied(verify(&shard, "wi-1", capability.clone(), &wrong_principal));
        let mut wrong_session = owner.clone();
        wrong_session.session = "session-b".to_string();
        assert_denied(verify(&shard, "wi-1", capability.clone(), &wrong_session));
        let mut wrong_epoch = owner.clone();
        wrong_epoch.authority_epoch = 8;
        assert_denied(verify(&shard, "wi-1", capability.clone(), &wrong_epoch));
        let mut wrong_incarnation = owner.clone();
        wrong_incarnation.incarnation_id = "incarnation-b".to_string();
        assert_denied(verify(
            &shard,
            "wi-1",
            capability.clone(),
            &wrong_incarnation,
        ));

        // Authoritative expiry, terminal state, and a reclaimed lease all
        // invalidate the previously minted capability.
        let expired = verify(&shard, "wi-1", capability.clone(), &authority(110_000));
        assert_denied(expired);
        // A native retryable failure returns the row to ready, then the
        // native claim authority advances both lease epoch and fence.
        commit_native_result(&shard, "wi-1", 1, 1, 10_001, "retry", "failed", true);
        claim_native(&shard, "wi-1", "worker-a", 20_000, "reclaim");
        assert_denied(verify(&shard, "wi-1", capability.clone(), &owner));
        // The new lease can be completed only through the native result
        // operation; generic CAS is no longer an authority path.
        commit_native_result(&shard, "wi-1", 2, 2, 20_001, "terminal", "succeeded", false);
        assert_denied(verify(&shard, "wi-1", capability.clone(), &owner));

        capability
    };

    // Capability and invocation rows are native durable state, not public
    // WorkItem metadata; reopening the same database still returns the
    // exact replay bytes and remains fail-closed after the lease changed.
    let shard = open(&path);
    let replay_after_restart = mint(&shard, "wi-1", &authority(10_001));
    assert_ne!(
        replay_after_restart.decision,
        WorkItemClaimCapabilityDecision::Minted
    );
    assert_ne!(
        replay_after_restart.capability,
        Some(capability.clone()),
        "the changed lease must not replay a stale capability"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn mint_refusal_names_the_failed_live_lease_check() {
    let path = temp_path("lease-refusals");
    let shard = open(&path);
    seed_work_item(&shard, "wi-1");
    let owner = authority(10_000);
    let decision =
        |item: &str, caller: &AuthenticatedAuthority| mint(&shard, item, caller).decision;
    assert_eq!(
        decision("unknown-work-item", &owner),
        WorkItemClaimCapabilityDecision::NotFound
    );
    let mut wrong_tenant = owner.clone();
    wrong_tenant.tenant = "tenant-b".to_string();
    assert_eq!(
        decision("wi-1", &wrong_tenant),
        WorkItemClaimCapabilityDecision::Unauthorized
    );
    let mut wrong_owner = owner.clone();
    wrong_owner.agent_id = "worker-b".to_string();
    assert_eq!(
        decision("wi-1", &wrong_owner),
        WorkItemClaimCapabilityDecision::Unauthorized
    );
    assert_eq!(
        decision("wi-1", &authority(110_000)),
        WorkItemClaimCapabilityDecision::Expired
    );
    commit_native_result(&shard, "wi-1", 1, 1, 10_001, "terminal", "succeeded", false);
    assert_eq!(
        decision("wi-1", &owner),
        WorkItemClaimCapabilityDecision::Stale
    );
    let _ = std::fs::remove_file(path);
}
