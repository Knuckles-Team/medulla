//! Moved from the in-crate test tree of medulla-persistence (MEDULLA-CONTRACT-R056.11).

#[allow(unused_imports)]
use std::collections::BTreeMap;
#[allow(unused_imports)]
use std::io::{Read as _, Write as _};
#[allow(unused_imports)]
use std::path::{Path, PathBuf};
#[allow(unused_imports)]
use medulla_storage::RecoveryStoreCounts;
#[allow(unused_imports)]
use redb::ReadableTable;
#[allow(unused_imports)]
use sha2::{Digest, Sha256};
#[allow(unused_imports)]
use medulla_persistence::shard_migrate;
#[allow(unused_imports)]
use medulla_store::redb_store::shard::Shard;
#[allow(unused_imports)]
use medulla_store::redb_store::XSHARD_DECISION;
#[allow(unused_imports)]
use medulla_store::redb_store::XSHARD_PREPARE;
#[allow(unused_imports)]
use medulla_persistence::backup::*;

use crate::redb_backend_tests::seed_raw_row;
use medulla_persistence::redb_backend::RedbBackend;
#[cfg(feature = "security")]
use medulla_persistence::redb_backend::ENCRYPTION_CANARY;
use crate::shard_migrate_tests::assert_node_a_carries_graph_tag;
use crate::shard_migrate_tests::props;
use crate::shard_migrate_tests::seed_at_k as seed;
use medulla_protocol_types::protocol::GraphType;
use medulla_protocol_types::protocol::Method;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_storage::{
    GraphShardOwner, OwnerLayout, PhysicalStoreIdentity, StorageKernel, StrictRecoveryEvidence,
};
use medulla_store::redb_store::shard::SHARD_PHYSICAL_STORE;
#[cfg(feature = "security")]
use redb::TableHandle;
use redb::{Database, ReadableDatabase};

/// Reopen ONE shard file as the kernel owner it is, for offline inspection.
/// Only valid once nothing holds the file (redb's exclusive lock).
fn shard_kernel(path: &Path) -> StorageKernel {
    StorageKernel::open_owner::<GraphShardOwner>(
        path,
        PhysicalStoreIdentity::new(SHARD_PHYSICAL_STORE).expect("shard physical identity"),
        None,
    )
    .expect("open shard as a kernel owner file")
}

/// Row count of ONE table in one census, or 0 when the census does not declare it.
/// Presence is asserted separately, against the declared table list.
fn census_rows(evidence: &StrictRecoveryEvidence, table: &str) -> u64 {
    evidence
        .tables
        .iter()
        .find(|entry| entry.table_id == table)
        .map(|entry| entry.rows)
        .unwrap_or_default()
}

/// Row count of ONE table in a shard file opened fresh (offline inspection —
/// mirrors `shard_migrate.rs`'s own test helper of the same name).
fn table_row_count<K, V>(path: &Path, def: redb::TableDefinition<K, V>) -> usize
where
    K: redb::Key + 'static,
    V: redb::Value + 'static,
{
    let db = Database::open(path).expect("open shard for inspection");
    let rtx = db.begin_read().expect("begin read");
    match rtx.open_table(def) {
        Ok(t) => t.iter().expect("iterate table").count(),
        Err(_) => 0,
    }
}

/// CONCEPT:MEDULLA-KG.sharding.reshard-on-restore — the DR round trip: populate a durable dir → ONLINE backup (live,
/// no quiesce) → restore into a FRESH dir → reopen and assert every graph's
/// nodes/edges/ledger survive identically.
#[tokio::test(flavor = "multi_thread")]
async fn backup_restore_roundtrip_preserves_everything() {
    // Held for the whole test: it opens a `RedbBackend` (`src`), backs it up, then
    // opens a SECOND `RedbBackend` (`restored`) and requires the restored data to
    // read back identically — which only holds if `MEDULLA_ENCRYPTION_KEY`
    // resolves the SAME way at both opens. See
    // `medulla_kernel::crypto::acquire_test_env_lock`'s doc for the full mechanism.
    #[cfg(feature = "security")]
    let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
    // Exercise the ENCRYPTED restore path deliberately, instead of whatever the
    // ambient environment happens to configure.
    //
    // This test used to pass with encryption OFF (no key in env => no canary rows
    // => the cross-shard key check short-circuits), and fail the moment a key was
    // present. That is backwards: the K>1 restore is exactly where the key-binding
    // comparison runs, so the interesting path was the one never covered. Pinning
    // a key here makes the encrypted multi-shard round trip the default assertion.
    #[cfg(feature = "security")]
    let _key_guard = EnvVarGuard::set(
        medulla_kernel::crypto::ENCRYPTION_KEY_ENV,
        "backup-roundtrip-key-material",
    );
    let root = std::env::temp_dir().join(format!("medulla-backup-rt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src = root.join("live");
    let bundle = root.join("bundle");
    let restored = root.join("restored");
    std::fs::create_dir_all(&src).unwrap();
    let src_s = src.to_string_lossy().to_string();

    let graphs = ["alpha", "beta", "gamma", "delta", "epsilon"];
    seed(&src_s, 3, &graphs).await;

    // ── ONLINE backup: reopen the SAME dir (K=3) and back it up while it is live ──
    let backend = RedbBackend::open_with_shards(src_s.clone(), 256, 3).expect("reopen");
    assert_eq!(backend.shard_count(), 3);
    let report = backend
        .backup(&bundle, "test-engine", 1_700_000_000, "nightly", &[])
        .expect("backup");
    assert_eq!(report.shards, 3);
    assert_eq!(report.shard_counts.len(), 3, "one census per bundle shard");
    // Every graph is one bound serving scope; the control scope of each file is
    // the `- K`. This is the census's own answer to "how many graphs did we
    // capture", replacing a `GRAPH_META` row counter the copy loop kept about
    // itself.
    assert_eq!(report.graph_scopes(), graphs.len() as u64);
    // Capture the LIVE per-graph shape so restore can be proven identical.
    let mut source: std::collections::HashMap<String, (usize, usize, usize)> =
        std::collections::HashMap::new();
    for g in &graphs {
        let d = backend.read_graph_dump_blocking(g).unwrap().unwrap();
        source.insert(
            (*g).to_string(),
            (d.nodes.len(), d.edges.len(), d.ledger.len()),
        );
    }
    backend.shutdown();

    // Bundle holds K=3 shard files + a manifest.
    for i in 0..3 {
        assert!(bundle.join(format!("graph-{i}.redb")).exists(), "shard {i}");
    }
    assert!(
        bundle.join(ADMIN_MUTATIONS_FILE).exists(),
        "admin coordinator store"
    );
    // `read_manifest` re-derives every bundle shard's census from the FILE, so a
    // manifest that survives this call is one the bundle actually backs up.
    let manifest = read_manifest(&bundle).expect("manifest");
    assert_eq!(manifest.shard_count, 3);
    assert_eq!(manifest.engine_version, "test-engine");
    assert_eq!(manifest.timestamp, 1_700_000_000);
    assert_eq!(manifest.label_ref, opaque_text_ref("nightly"));
    assert_eq!(manifest.shard_counts, report.shard_counts);
    assert_eq!(manifest.graph_scopes(), graphs.len() as u64);

    // ── restore into a FRESH dir at the same K ──
    let rr = restore_bundle(&bundle, &restored, 3).expect("restore");
    assert_eq!(rr.restored_shards, 3);
    assert_eq!(rr.restored_counts.len(), 3);
    assert_eq!(graph_scopes(&rr.restored_counts), graphs.len() as u64);

    // ── reopen the restored dir and verify every graph is intact ──
    let restored_s = restored.to_string_lossy().to_string();
    let rb = RedbBackend::open_with_shards(restored_s, 256, 3).expect("reopen");
    assert_eq!(rb.shard_count(), 3);
    for g in &graphs {
        let dump = rb
            .read_graph_dump_blocking(g)
            .expect("read")
            .unwrap_or_else(|| panic!("graph {g} missing after restore"));
        assert_eq!(dump.name, *g);
        assert_eq!(dump.nodes.len(), 2, "graph {g} nodes");
        assert_eq!(dump.edges.len(), 1, "graph {g} edges");
        // Restored shape is IDENTICAL to the live source (nodes, edges, ledger).
        // This is where the report's old `nodes`/`edges`/`ledger` counters are
        // re-pinned: they claimed the copy was complete, this proves it is.
        let src = source.get(*g).copied().expect("source shape");
        assert_eq!(
            (dump.nodes.len(), dump.edges.len(), dump.ledger.len()),
            src,
            "graph {g} shape identical after restore"
        );
        assert_node_a_carries_graph_tag(&dump, g);
    }
    rb.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

/// CONCEPT:MEDULLA-KG.sharding.reshard-on-restore — RE-SHARD ON RESTORE: a K=1 bundle restores into a K=4 persist-dir,
/// every graph re-routed by MEDULLA-026 and still fully readable.
#[tokio::test(flavor = "multi_thread")]
async fn restore_can_reshard() {
    // See `backup_restore_roundtrip_preserves_everything` above: held for the
    // whole test (this one also opens a source backend and a re-sharded restored
    // backend that must resolve the same cipher).
    #[cfg(feature = "security")]
    let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
    let root =
        std::env::temp_dir().join(format!("medulla-backup-reshard-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src = root.join("live");
    let bundle = root.join("bundle");
    let restored = root.join("restored");
    std::fs::create_dir_all(&src).unwrap();
    let src_s = src.to_string_lossy().to_string();

    let graphs = ["one", "two", "three", "four", "five", "six", "seven"];
    seed(&src_s, 1, &graphs).await;

    let backend = RedbBackend::open_with_shards(src_s.clone(), 256, 1).expect("reopen");
    let report = backend
        .backup(&bundle, "test-engine", 42, "", &[])
        .expect("backup");
    assert_eq!(report.shards, 1);
    assert!(bundle.join("graph-0.redb").exists(), "K=1 bundle file");
    backend.shutdown();

    // Restore at K=4 (re-shard on restore). A scope cannot be created or
    // destroyed by re-routing it, so the graph-scope census is invariant across
    // the K change and `restore_bundle` refuses a restore that loses one.
    let rr = restore_bundle(&bundle, &restored, 4).expect("restore reshard");
    assert_eq!(rr.restored_shards, 4);
    assert_eq!(rr.restored_counts.len(), 4);
    assert_eq!(graph_scopes(&rr.restored_counts), graphs.len() as u64);

    let restored_s = restored.to_string_lossy().to_string();
    let rb = RedbBackend::open_with_shards(restored_s, 256, 4).expect("reopen");
    assert_eq!(rb.shard_count(), 4, "restored at K=4");
    for g in &graphs {
        assert!(
            rb.read_graph_dump_blocking(g).unwrap().is_some(),
            "graph {g} after reshard-restore"
        );
    }
    rb.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

/// BUG-PE-054 — the DR round trip must carry IDENTITY state.
///
/// The bundle used to hold graph shards + `admin-mutations.redb` only, so a restore
/// came up with no RBAC/identity, KV, cluster-topology or placement-catalog state —
/// silently, from a manifest that said nothing about its own scope. This proves the
/// non-shard durable stores survive the round trip, and that the manifest declares
/// both what it captured and what it deliberately did not.
#[cfg(feature = "kv")]
#[tokio::test(flavor = "multi_thread")]
async fn backup_restore_carries_non_shard_durable_stores() {
    #[cfg(feature = "security")]
    let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
    let root = std::env::temp_dir().join(format!("medulla-backup-sib-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src = root.join("live");
    let bundle = root.join("bundle");
    let restored = root.join("restored");
    std::fs::create_dir_all(&src).unwrap();
    let src_s = src.to_string_lossy().to_string();

    seed(&src_s, 1, &["alpha"]).await;

    // Durable RBAC/identity state beside the shards — the omission that mattered.
    let identity = medulla_types::acl::AgentIdentity {
        agent_id: "restore-probe".to_string(),
        role: medulla_types::acl::AgentRole::Agent,
        teams: Vec::new(),
        roles: vec!["reader".to_string()],
    };
    let authority = medulla_kernel::store_authority::process_authority();
    let rbac = medulla_isolation::rbac_persist::RbacStore::open(
        &src,
        authority.as_ref(),
        authority.principal(),
        &authority.proof(),
    )
    .expect("open rbac store");
    let snapshot = rbac.authority_snapshot().expect("read identity preimage");
    let mut identities = snapshot.identities().clone();
    identities.insert(identity.agent_id.clone(), identity);
    rbac.compare_and_save(
        snapshot.revision(),
        snapshot.policy(),
        &identities,
        medulla_isolation::rbac_persist::IdentityBootstrapState::Consumed,
    )
    .expect("persist identity");

    let kv = medulla_server_blob::kv::KvStore::open(Some(&src_s)).expect("open kv");
    kv.put("probe", "key", b"value".to_vec()).expect("kv put");

    let backend = RedbBackend::open_with_shards(src_s.clone(), 256, 1).expect("reopen");
    // Exactly the adapter the production admin handler hands in.
    let rbac_source = medulla_persistence::durable_stores::RbacBundledStore(std::sync::Arc::new(rbac));
    let extra: Vec<&dyn medulla_server_api::api::persistence::BundledStoreSource> =
        vec![&rbac_source, &kv];
    let report = backend
        .backup(&bundle, "test-engine", 7, "sibling", &extra)
        .expect("backup");
    backend.shutdown();
    drop(rbac_source);
    drop(kv);

    assert!(
        report.bundled_stores.contains_key("rbac.redb"),
        "rbac.redb bundled, got {:?}",
        report.bundled_stores
    );
    assert!(
        report.bundled_stores.contains_key("kv.redb"),
        "kv.redb bundled, got {:?}",
        report.bundled_stores
    );
    assert!(
        report.bundled_stores.contains_key("node_info.redb"),
        "node_info.redb bundled, got {:?}",
        report.bundled_stores
    );

    let manifest = read_manifest(&bundle).expect("manifest validates");
    assert_eq!(manifest.bundled_stores, report.bundled_stores);
    // Self-describing: every deliberately-excluded store is named WITH its reason.
    for (name, reason) in &manifest.excluded_stores {
        assert!(!reason.is_empty(), "{name} excluded with no reason");
    }
    assert!(
        manifest.excluded_stores.contains_key("blob.redb"),
        "the manifest must state that blob bytes are not captured"
    );

    let restore = restore_bundle(&bundle, &restored, 1).expect("restore");
    for expected in ["rbac.redb", "kv.redb", "node_info.redb"] {
        assert!(
            restore.restored_stores.contains(&expected.to_string()),
            "{expected} restored, got {:?}",
            restore.restored_stores
        );
    }
    // The restored engine knows the identity again — the whole point of BUG-PE-054.
    let restored_rbac = medulla_isolation::rbac_persist::RbacStore::open(
        &restored,
        authority.as_ref(),
        authority.principal(),
        &authority.proof(),
    )
    .expect("reopen restored rbac");
    let (_, restored_identities, bootstrap) = restored_rbac.load().expect("load identities");
    assert!(
        restored_identities.contains_key("restore-probe"),
        "registered identity survived the round trip, got {:?}",
        restored_identities.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        bootstrap,
        medulla_isolation::rbac_persist::IdentityBootstrapState::Consumed,
        "identity bootstrap lifecycle survived the round trip"
    );
    drop(restored_rbac);
    // The restored KV surface answers the write taken before the backup.
    let restored_s = restored.to_string_lossy().to_string();
    let restored_kv =
        medulla_server_blob::kv::KvStore::open(Some(&restored_s)).expect("open kv");
    assert_eq!(
        restored_kv.get("probe", "key").expect("kv get").as_deref(),
        Some(&b"value"[..]),
        "KV row survived the round trip"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// BUG-PE-054 — an unregistered durable store cannot be smuggled into a bundle.
/// The registry is the single list; anything else must fail loudly at backup time.
#[tokio::test(flavor = "multi_thread")]
async fn backup_refuses_an_unregistered_bundled_store() {
    #[cfg(feature = "security")]
    let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
    struct Bogus;
    impl medulla_server_api::api::persistence::BundledStoreSource for Bogus {
        fn file_name(&self) -> &'static str {
            "not-in-the-registry.redb"
        }
        fn copy_into(&self, _destination: &Path) -> Result<u64, String> {
            panic!("must never be copied");
        }
    }
    let root =
        std::env::temp_dir().join(format!("medulla-backup-bogus-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src = root.join("live");
    std::fs::create_dir_all(&src).unwrap();
    let src_s = src.to_string_lossy().to_string();
    seed(&src_s, 1, &["alpha"]).await;
    let backend = RedbBackend::open_with_shards(src_s.clone(), 256, 1).expect("reopen");
    let bogus = Bogus;
    let extra: Vec<&dyn medulla_server_api::api::persistence::BundledStoreSource> =
        vec![&bogus];
    let error = backend
        .backup(&root.join("bundle"), "test-engine", 7, "", &extra)
        .expect_err("unregistered store must be refused");
    assert!(
        error.contains("not a registered bundled durable store"),
        "got: {error}"
    );
    backend.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

/// A restore refuses a manifest with a future format version.
#[test]
fn rejects_future_format() {
    let dir = std::env::temp_dir().join(format!("medulla-backup-fmt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // The manifest schema has grown and shed fields since this fixture was first
    // written; a manifest missing any current field fails
    // `serde(deny_unknown_fields)` DESERIALIZATION before `read_manifest` ever
    // reaches its `format_version` check, so a stale shape asserts the wrong
    // error message. This fixture is kept in sync with the CURRENT
    // `BackupManifest` shape (every field present, `format_version` deliberately
    // one past what this build understands) so the test exercises exactly the
    // format-version guard it names, not a parse failure one step earlier.
    let m = serde_json::json!({
        "format_version": BUNDLE_FORMAT_VERSION + 1,
        "engine_version": "x",
        "shard_count": 1,
        "timestamp": 0,
        "label_ref": opaque_text_ref(""),
        "shard_counts": [RecoveryStoreCounts::default()],
        "xshard_prepares": 0,
        "xshard_decisions": 0,
        "admin_mutations": RecoveryStoreCounts::default(),
        "bundled_stores": {},
        "excluded_stores": {},
        "file_digests": {},
    });
    std::fs::write(dir.join(MANIFEST_FILE), serde_json::to_vec(&m).unwrap()).unwrap();
    let err = read_manifest(&dir).unwrap_err();
    assert!(
        err.contains("not the portable coordinator-aware format"),
        "got: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// WD5-BUG-04, re-pinned onto the DECLARED CENSUS.
///
/// The defect was a hand-maintained copy list that silently omitted 30 tables, and
/// the test that caught it enumerated the same list back — so it could only ever
/// catch the tables someone had already thought of. `backup_recovery_store` copies
/// the declared `OwnerLayout::GraphShard` census instead
/// (`copy_declared_owner_tables` + `visit_ledger_tables!`), so the property to
/// assert is no longer "these 30 tables came across" but "the census IS what came
/// across". This test asserts exactly that, three ways:
///
/// 1. the bundle's table inventory equals `declared_table_names(GraphShard)` —
///    a table added to the census joins the backup with no edit here, and one
///    dropped from the copy cannot pass;
/// 2. every declared OWNER table is byte-identical to the source's, fingerprint
///    included — which covers `encryption_canary` (a FileWide owner table) and so
///    proves the restore keeps the original key identity/version boundary and
///    cannot establish a new canary, without this module copying a canary row;
/// 3. the WD5-BUG-04 tables named in the original defect still carry their seeded
///    row after a full backup + restore round trip.
#[tokio::test(flavor = "multi_thread")]
async fn the_declared_census_is_what_a_backup_copies() {
    #[cfg(feature = "security")]
    let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
    // A key is pinned so `encryption_canary` actually holds its sealed canary and
    // key-binding rows: an empty table would make assertion (2) vacuous.
    #[cfg(feature = "security")]
    let _key_guard = EnvVarGuard::set(
        medulla_kernel::crypto::ENCRYPTION_KEY_ENV,
        "backup-census-key-material",
    );
    use medulla_store::redb_layout::shard_filename;
    use medulla_resource_store::redb_store::capacity_lease;
    use medulla_lane_store::redb_store::development_lane;
    use medulla_store::redb_store::MATVIEW_OPERATOR_STATE;
    use medulla_store::redb_store::PLAN_MATVIEWS;
    use medulla_store::redb_store::PROVENANCE_ANCHOR_MEMBERS;
    use medulla_store::redb_store::RESOURCE_RESERVATIONS;
    use medulla_store::redb_store::WORK_ITEM_COMMAND_SEQUENCE;

    let root =
        std::env::temp_dir().join(format!("medulla-backup-census-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src = root.join("live");
    let bundle = root.join("bundle");
    let restored = root.join("restored");
    std::fs::create_dir_all(&src).unwrap();
    let src_s = src.to_string_lossy().to_string();

    let backend = RedbBackend::open_with_shards(src_s.clone(), 256, 1).expect("open backend");
    backend
        .register_graph("g", "g", GraphType::Global)
        .await
        .expect("register");
    // One real row through the graph's OWN scope, exactly as this module's
    // `seed()` helper does for every other test here.
    //
    // It is not decoration. `register_graph` writes the catalog entry under
    // the shard's CONTROL scope, and a graph's own scope is bound LAZILY on
    // first use (CONCEPT:MEDULLA-KG.sharding.lazy-graph-catalog), so a graph that
    // is registered and never read or written has NO scope binding at all.
    // The MEDULLA-030 migration engine that backs both `restore_bundle` and online
    // resharding then refuses to move it: `Shard::graft_graph_from_with_payload`
    // fails with "graft source scope is not bound and destination has no
    // recovery binding", because it cannot tell "this scope never existed"
    // from "this scope was retired by a completed Phase C and its destination
    // marker is missing". Without this write, that refusal -- not the census
    // property named above -- is what this test measured.
    //
    // KNOWN GAP, reported rather than guessed at (medulla-f3 burndown,
    // 2026-09-11): "registered but never written" is a reachable production
    // state, so a store containing such a graph cannot currently be
    // backed up and restored. Closing it means teaching the graft preflight
    // to distinguish the two cases from durable evidence, which is a change
    // to graft semantics and belongs with the MEDULLA-030 owner -- the same
    // caution `shard_migrate.rs` records about its own snapshot rebinding.
    backend
        .record_durable(
            "g",
            &Method::AddNode {
                node_id: "census-a".into(),
                properties_msgpack: props(serde_json::json!({"type": "Task"})),
            },
        )
        .await
        .expect("seed one row through the graph's own scope");
    backend.shutdown();
    // `shutdown()` only stops the shard writer thread (which releases the
    // graph-N.redb file lock); it does NOT drop `admin_mutations`/`node_info`/
    // `cluster_hierarchy` on this `RedbBackend`. Rust does not drop a
    // shadowed `let` binding until the END of the enclosing scope, so
    // WITHOUT this explicit drop the reopen below (`let backend = ...`,
    // same `src_s` directory) still holds this backend's admin-mutations.redb
    // handle open and fails with "Database already open. Cannot acquire
    // lock." (WD5-BUG-05 — this was a test defect, not a fix defect: every
    // other reopen in this module targets a DIFFERENT directory via a
    // differently-named variable, so this is the only test that shadows
    // `backend` on the SAME directory).
    drop(backend);

    let shard0 = src.join(shard_filename(0));
    seed_raw_row(&shard0, RESOURCE_RESERVATIONS, ("g", "r1"), b"reservation");
    seed_raw_row(&shard0, development_lane::HOLDS, ("g", "h1"), b"hold");
    seed_raw_row(&shard0, capacity_lease::CELLS, ("g", "c1"), b"cell");
    seed_raw_row(&shard0, WORK_ITEM_COMMAND_SEQUENCE, "g", 7u64);
    seed_raw_row(
        &shard0,
        PROVENANCE_ANCHOR_MEMBERS,
        ("g", 1u64),
        b"anchor-member",
    );
    seed_raw_row(&shard0, PLAN_MATVIEWS, "mv-1", b"plan-def");
    seed_raw_row(&shard0, MATVIEW_OPERATOR_STATE, "mv-1", b"operator-state");

    // ── ONLINE backup (live, reopened — matches every other test in this module) ──
    let backend = RedbBackend::open_with_shards(src_s.clone(), 256, 1).expect("reopen");
    backend
        .backup(&bundle, "test-engine", 1, "census", &[])
        .expect("backup");
    backend.shutdown();
    drop(backend);

    // (1) + (2): the bundle file's own census, compared table by table against
    // the source's -- taken HERE, BEFORE the restore.
    //
    // It used to be taken after, on the reasoning that "nothing here can
    // perturb the bytes the manifest's digests were computed over". The
    // reader does not, but `restore_bundle` itself does: it runs
    // `shard_migrate::migrate_shards` DIRECTLY against the bundle directory,
    // and a migration MOVES each graph -- "the graft copies every ledger row
    // verbatim and retires the source scope", "the move consumes the
    // migration snapshot, not the live source" (`shard_migrate.rs`'s module
    // doc). So by the time the old code read the bundle's census, every
    // migrated graph's owner rows had been retired OUT of the bundle file,
    // and the comparison reported `nodes` as 1 row in the source and 0 in
    // the bundle -- a difference the restore created, not one `backup()`
    // left. `src` is untouched by the restore, so only the bundle read had
    // to move.
    let source_evidence = {
        let kernel = shard_kernel(&shard0);
        medulla_storage::strict_recovery_evidence(&kernel).expect("source census")
    };
    // The bundle's census is read from a REBOUND COPY, never from the bundle
    // itself, because BOTH ways of touching the bundle directly are
    // destructive to this assertion:
    //
    //  * reading it in place: `shard_kernel` opens the file as a kernel
    //    owner, and an open re-materializes and re-validates the declared
    //    table census -- a write. The bundle's own portable-file digests then
    //    no longer match its manifest and `restore_bundle` refuses.
    //  * reading it after the restore: the restore MOVES each graph out of
    //    the bundle (`shard_migrate.rs`: "the graft copies every ledger row
    //    verbatim and retires the source scope", "the move consumes the
    //    migration snapshot"), so the census then describes an evacuated
    //    file rather than what `backup()` produced.
    //
    // A copy is unopenable at its new path until its `(dev, ino)`-derived
    // physical root is rebound, which is exactly what `rebind_copied_store`
    // is for. Rebinding writes only the physical-identity tables, which are
    // in neither `owner_table_names` nor `declared_table_names` (see
    // `medulla_storage::owner_table_names`' doc: "the ledger and the three
    // physical-identity tables are never in this set"), so the census this
    // reads is byte-for-byte the one the bundle carries.
    let census_copy = root.join("bundle-census");
    std::fs::create_dir_all(&census_copy).unwrap();
    let bundle_shard0 = bundle.join(shard_filename(0));
    let census_shard0 = census_copy.join(shard_filename(0));
    std::fs::copy(&bundle_shard0, &census_shard0).unwrap();
    medulla_storage::rebind_copied_store(&census_shard0, &bundle_shard0)
        .expect("rebind the bundle census copy");
    let bundle_evidence = {
        let kernel = shard_kernel(&census_shard0);
        medulla_storage::strict_recovery_evidence(&kernel).expect("bundle census")
    };
    let mut inventory: Vec<&str> = bundle_evidence
        .tables
        .iter()
        .map(|entry| entry.table_id.as_str())
        .collect();
    inventory.sort_unstable();
    assert_eq!(
        inventory,
        medulla_storage::declared_table_names(OwnerLayout::GraphShard),
        "the bundle's tables ARE the declared graph-shard census"
    );
    for owner_table in medulla_storage::owner_table_names(OwnerLayout::GraphShard) {
        let expected = source_evidence
            .tables
            .iter()
            .find(|entry| entry.table_id == *owner_table)
            .unwrap_or_else(|| panic!("source census omits {owner_table}"));
        let actual = bundle_evidence
            .tables
            .iter()
            .find(|entry| entry.table_id == *owner_table)
            .unwrap_or_else(|| panic!("bundle census omits {owner_table}"));
        assert_eq!(
            (expected.rows, expected.fingerprint),
            (actual.rows, actual.fingerprint),
            "{owner_table} is not byte-identical in the bundle"
        );
    }
    // The canary is carried BY THE CENSUS, not by a hand-written copy: a restore
    // therefore retains the original key identity/version boundary and cannot
    // silently establish a new canary under a different key.
    #[cfg(feature = "security")]
    assert!(
        census_rows(&bundle_evidence, ENCRYPTION_CANARY.name()) > 0,
        "the sealed canary + key-binding rows must be in the bundle"
    );

    // ── restore into a FRESH dir, which also validates the bundle's manifest ──
    // Deliberately AFTER the bundle census above: the restore's MEDULLA-030 graft
    // consumes the bundle (see that block's comment).
    restore_bundle(&bundle, &restored, 1).expect("restore");

    // (3) the tables WD5-BUG-04 named, still present after backup AND restore.
    let restored_shard0 = restored.join(shard_filename(0));
    for (table, rows) in [
        ("resource_reservations", 1u64),
        ("development_lane_holds", 1),
        ("capacity_cells", 1),
        ("work_item_command_sequence", 1),
        ("provenance_anchor_members", 1),
        ("plan_matviews", 1),
        ("matview_operator_state", 1),
    ] {
        assert_eq!(
            census_rows(&bundle_evidence, table),
            rows,
            "{table} survives the backup (WD5-BUG-04)"
        );
    }
    assert_eq!(
        table_row_count(&restored_shard0, RESOURCE_RESERVATIONS),
        1,
        "resource_reservations survives backup/restore (WD5-BUG-04)"
    );
    assert_eq!(
        table_row_count(&restored_shard0, development_lane::HOLDS),
        1,
        "development_lane_holds survives backup/restore (WD5-BUG-04)"
    );
    assert_eq!(
        table_row_count(&restored_shard0, capacity_lease::CELLS),
        1,
        "capacity_cells survives backup/restore (WD5-BUG-04)"
    );
    assert_eq!(
        table_row_count(&restored_shard0, WORK_ITEM_COMMAND_SEQUENCE),
        1,
        "work_item_command_sequence survives backup/restore (WD5-BUG-04)"
    );
    assert_eq!(
        table_row_count(&restored_shard0, PROVENANCE_ANCHOR_MEMBERS),
        1,
        "provenance_anchor_members survives backup/restore (WD5-BUG-04)"
    );
    assert_eq!(
        table_row_count(&restored_shard0, PLAN_MATVIEWS),
        1,
        "plan_matviews survives backup/restore (WD5-BUG-04)"
    );
    assert_eq!(
        table_row_count(&restored_shard0, MATVIEW_OPERATOR_STATE),
        1,
        "matview_operator_state survives backup/restore (WD5-BUG-04)"
    );

    let _ = std::fs::remove_dir_all(&root);
}
