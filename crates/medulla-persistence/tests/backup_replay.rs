//! Point-in-time restore from archive segments (`MEDULLA-ENGINE-PARITY-R016.2`
//! and `MEDULLA-ENGINE-PARITY-R016.3`).
#![cfg(all(
    feature = "redb",
    feature = "object-store",
    feature = "test-support"
))]

use std::path::Path;
use std::sync::Arc;

use medulla_mutation_types::mutation_scope::{
    DurabilityDomain, IncarnationId, LogicalName, MutationOutboxIntent, MutationScopeIdentity,
    MutationSurface, ScopeTenantId, MUTATION_BATCH_VERSION,
};
use medulla_persistence::backup::restore_bundle;
use medulla_persistence::backup_replay::{archive_bounds, replay_archive};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::mutation_batch::{
    MutationBatch, MutationOperation, VersionExpectation, MUTATION_ACTOR_HEADER,
};
use medulla_protocol_types::protocol::Method;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_storage::object::archive::ObjectArchive;
use medulla_storage::{ledger_scope_key, replay_to, ArchivePolicy, ArchiveRefusal, ReplayTarget};

const TENANT: &str = "engine";
const GRAPH: &str = "replay-graph";

fn identity() -> MutationScopeIdentity {
    MutationScopeIdentity::graph(
        ScopeTenantId::new("replay-tenant").unwrap(),
        LogicalName::new(GRAPH).unwrap(),
        IncarnationId::new("incarnation:test:backup-replay").unwrap(),
    )
}

/// Batch number `index` of the scope: it commits at sequence `index + 1`.
fn batch(index: u64) -> MutationBatch {
    let identity = identity();
    let batch_id = format!("b{index}");
    let actor = format!("principal:sha256:{}", "b".repeat(64));
    let mut batch = MutationBatch {
        schema_version: MUTATION_BATCH_VERSION,
        batch_id: batch_id.clone(),
        envelope: medulla_store::redb_store::store_graph_names::fixture_operation_envelope(
            &identity, &actor, 17, &batch_id,
        ),
        identity,
        placement_epoch: 1,
        version_expectation: VersionExpectation::Graph(index),
        fencing_token: Some(1),
        authoritative_state: None,
        operations: vec![MutationOperation {
            ordinal: 0,
            surface: MutationSurface::Transaction,
            domain: DurabilityDomain::GraphRows,
            method: Method::AddNode {
                node_id: format!("node-{index}"),
                properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({"n": index}))
                    .unwrap(),
            },
        }],
        outbox: vec![MutationOutboxIntent {
            topic: "projection.replay".to_string(),
            key: batch_id,
            payload: rmp_serde::to_vec_named(&serde_json::json!({"event": index})).unwrap(),
            headers: std::collections::BTreeMap::from([(MUTATION_ACTOR_HEADER.to_string(), actor)]),
        }],
        created_at_ms: 10,
    };
    let schema_digest = batch
        .envelope
        .operation()
        .expect("fixture carries an operation envelope")
        .method_schema_digest;
    batch.reseal_envelope(schema_digest).unwrap();
    batch
}

async fn commit_range(backend: &RedbBackend, range: std::ops::Range<u64>) {
    for index in range {
        PersistenceBackend::commit_mutation_batch(
            backend,
            GRAPH,
            &batch(index),
            None,
            1_000 + index,
        )
        .await
        .expect("batch commits");
    }
}

fn archive() -> ObjectArchive {
    ObjectArchive::new(Arc::new(object_store::memory::InMemory::new()), "root")
}

fn bundle_of(backend: &RedbBackend, dir: &Path) {
    backend
        .backup(dir, "test", 1_700_000_000, "replay", &[])
        .expect("bundle is written");
}

// spec: MEDULLA-ENGINE-PARITY-R016.2
#[tokio::test(flavor = "multi_thread")]
async fn replay_applies_segments_to_the_target_and_refuses_past_the_end() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let root = tempfile::tempdir().unwrap();
    let source_dir = root.path().join("source");
    let bundle = root.path().join("bundle");
    let archive = archive();
    let scope = ledger_scope_key(&identity());

    // 10 batches inside the bundle, 20 further batches only in the archive.
    let source = RedbBackend::open(source_dir.to_string_lossy().into_owned(), 64).unwrap();
    commit_range(&source, 0..10).await;
    bundle_of(&source, &bundle);
    commit_range(&source, 10..30).await;
    let policy = ArchivePolicy {
        segment_bytes: 4 * 1024,
        retain_s: 3_600,
        interval_s: 10,
    };
    let pass = source.archive_pass(&archive, TENANT, &policy).await.unwrap();
    assert_eq!(pass.records_archived, 30);
    drop(source);

    assert_eq!(archive.list_scopes(TENANT).await.unwrap(), vec![scope.clone()]);

    // A target past the archive end refuses and the destination stays untouched.
    let bounds = archive_bounds(&archive, TENANT).await.unwrap();
    assert_eq!(bounds.end_sequence, 30);
    let past = ReplayTarget::Sequence(31);
    assert!(matches!(
        replay_to(&past, &bounds),
        Err(ArchiveRefusal::TargetPastArchiveEnd { .. })
    ));
    let untouched = root.path().join("untouched");
    assert!(!untouched.exists());

    // A target inside the archive replays exactly the records between.
    let restored = root.path().join("restored");
    restore_bundle(&bundle, &restored, 1, None).unwrap();
    let report = replay_archive(&restored, 1, &archive, TENANT, &ReplayTarget::Sequence(15))
        .await
        .unwrap();
    assert_eq!(report.records_applied, 5);
    assert_eq!(report.final_sequence.get(&scope), Some(&15));
    assert!(report.segments_replayed >= 1);

    let reopened = RedbBackend::open_with_shards(restored.to_string_lossy().into_owned(), 64, 1)
        .unwrap();
    for index in 0..15 {
        let record = PersistenceBackend::read_mutation_batch(&reopened, GRAPH, &format!("b{index}"))
            .await
            .unwrap();
        assert!(record.is_some(), "batch b{index} is in the restored ledger");
    }
    for index in 15..30 {
        let record = PersistenceBackend::read_mutation_batch(&reopened, GRAPH, &format!("b{index}"))
            .await
            .unwrap();
        assert!(record.is_none(), "batch b{index} is past the target");
    }
    assert_eq!(reopened.last_committed_sequences().unwrap().get(&scope), Some(&15));
}

// spec: MEDULLA-ENGINE-PARITY-R016.3
#[ignore = "nightly restore drill"]
#[tokio::test(flavor = "multi_thread")]
async fn restore_drill_has_no_lost_sequences() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let root = tempfile::tempdir().unwrap();
    let source_dir = root.path().join("source");
    let bundle = root.path().join("bundle");
    let archive = archive();
    let scope = ledger_scope_key(&identity());

    let source = RedbBackend::open(source_dir.to_string_lossy().into_owned(), 1024).unwrap();
    bundle_of(&source, &bundle);
    commit_range(&source, 0..100_000).await;
    let policy = ArchivePolicy {
        segment_bytes: 64 * 1024 * 1024,
        retain_s: 3_600,
        interval_s: 10,
    };
    let pass = source.archive_pass(&archive, TENANT, &policy).await.unwrap();
    assert_eq!(pass.records_archived, 100_000);
    drop(source);

    let restored = root.path().join("restored");
    restore_bundle(&bundle, &restored, 1, None).unwrap();
    let target = ReplayTarget::Sequence(50_000);
    let report = replay_archive(&restored, 1, &archive, TENANT, &target)
        .await
        .unwrap();
    assert_eq!(report.final_sequence.get(&scope), Some(&50_000));
    assert_eq!(report.records_applied, 50_000);

    let reopened = RedbBackend::open_with_shards(restored.to_string_lossy().into_owned(), 1024, 1)
        .unwrap();
    for index in 0..50_000u64 {
        let record = PersistenceBackend::read_mutation_batch(&reopened, GRAPH, &format!("b{index}"))
            .await
            .unwrap();
        assert!(record.is_some(), "sequence {} is missing", index + 1);
    }
    let after = PersistenceBackend::read_mutation_batch(&reopened, GRAPH, "b50000")
        .await
        .unwrap();
    assert!(after.is_none(), "no sequence past the target is restored");
    drop(reopened);

    // The same restore again is a no-op.
    let again = replay_archive(&restored, 1, &archive, TENANT, &target)
        .await
        .unwrap();
    assert_eq!(again.records_applied, 0);
}
