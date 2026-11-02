//! Graft/import regressions that exercise the store's shard through the
//! persistence-level reshard entry points (`graft_graph_from`,
//! `reserve_graft_destination`, `export_graph_raw`, `import_graph_raw`).

use medulla_persistence::online_reshard::*;
use medulla_store::redb_store::shard::{
    graph_scope_identity, Shard, ShardWrite, GRAFT_BEFORE_PROTOCOL_LOCK,
};
use medulla_store::redb_store::{GRAPH_META, NODES};
use medulla_transaction::GraftDestination;
use medulla_kernel::test_rendezvous::{join_bounded, meet};
use medulla_store::redb_store::shard::{
    ShardHandle, FAIL_AFTER_GRAFT_CATALOG_CLEANUP, IMPORT_AFTER_RESERVED_STAGE,
};
use std::sync::{Arc, Barrier, Mutex};

fn temp_path(tag: &str) -> std::path::PathBuf {
    medulla_store::redb_store::temp_path("medulla-shard-seam", tag)
}

fn members(shard: &Shard, graphs: &[&str]) -> Vec<(String, ShardHandle)> {
    let names: Vec<String> = graphs.iter().map(|g| (*g).to_string()).collect();
    let handles = shard.graph_handles(&names).unwrap();
    names.into_iter().zip(handles).collect()
}

/// A source marker for one destination must be authenticated before a
/// different destination is bound. Refusing the wrong target therefore
/// leaves both the source and the attempted destination untouched.
#[cfg(feature = "server")]
#[test]
fn graft_marker_for_another_destination_refuses_before_binding() {
    let source_path = temp_path("graft-marker-source");
    let first_destination_path = temp_path("graft-marker-first-destination");
    let second_destination_path = temp_path("graft-marker-second-destination");
    let source = Shard::open(&source_path).unwrap();
    let first_destination = Shard::open(&first_destination_path).unwrap();
    let second_destination = Shard::open(&second_destination_path).unwrap();
    let identity = graph_scope_identity("graph-a").unwrap();
    let source_handle = source.graph("graph-a").unwrap();
    let first_handle = first_destination.graph("graph-a").unwrap();
    let first = GraftDestination::new(
        first_destination.mutations(),
        first_destination.kernel(),
        first_handle.as_ref(),
    );
    source
        .mutations()
        .graft_begin(source_handle.as_ref(), &first)
        .unwrap();

    let error = medulla_persistence::online_reshard::graft_graph_from(&second_destination, &source, "graph-a")
        .unwrap_err();
    assert!(
        error.contains("different authority or destination"),
        "got: {error}"
    );
    assert!(
        !second_destination
            .kernel()
            .scope_binding_exists(&identity)
            .unwrap(),
        "wrong-target refusal bound the destination"
    );
    assert!(
        source.kernel().scope_binding_exists(&identity).unwrap(),
        "wrong-target refusal altered the source binding"
    );

    let _ = std::fs::remove_file(&source_path);
    let _ = std::fs::remove_file(&first_destination_path);
    let _ = std::fs::remove_file(&second_destination_path);
}

/// A destination may already be reserved for a different source while a
/// source marker chooses another winner.  The broad reservation scan is
/// enough to select the owner-only path, but it is not authorization to
/// delete this graph's catalog: only the exact `(source, destination)`
/// reservation may authorize wrong-target cleanup.
#[cfg(feature = "server")]
#[test]
fn unrelated_reservation_preserves_catalog_payload_and_fence() {
    let source_path = temp_path("graft-unrelated-source");
    let other_source_path = temp_path("graft-unrelated-other-source");
    let winner_path = temp_path("graft-unrelated-winner");
    let destination_path = temp_path("graft-unrelated-destination");
    let source = Shard::open(&source_path).unwrap();
    let other_source = Shard::open(&other_source_path).unwrap();
    let winner = Shard::open(&winner_path).unwrap();
    let destination = Shard::open(&destination_path).unwrap();
    let graph = "graph-unrelated-reservation";
    let identity = graph_scope_identity(graph).unwrap();
    let source_handle = source.graph(graph).unwrap();
    let _other_source_handle = other_source.graph(graph).unwrap();
    let winner_handle = winner.graph(graph).unwrap();
    let destination_handle = destination.graph(graph).unwrap();
    let winner_target =
        GraftDestination::new(winner.mutations(), winner.kernel(), winner_handle.as_ref());
    let destination_target = GraftDestination::new(
        destination.mutations(),
        destination.kernel(),
        destination_handle.as_ref(),
    );

    // D is reserved by S2, and has owner rows staged under that exact
    // reservation.  The catalog row is deliberately committed in the
    // separate control transaction used by the importer.
    medulla_persistence::online_reshard::reserve_graft_destination(&destination, &other_source, graph).unwrap();
    destination
        .stage_reserved_graft_owner_payload(graph, |write| {
            write
                .open_scoped_table(NODES)?
                .insert((graph, "s2-staged"), b"payload-s2".as_slice())
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .unwrap();
    medulla_store::redb_store::store_read::write_graph_meta(
        &destination,
        graph,
        "catalog-s2",
        medulla_protocol_types::protocol::GraphType::Global,
    )
    .unwrap();
    let catalog_before = {
        let read = destination.control_read().unwrap();
        let table = read.open_owner_table(GRAPH_META).unwrap();
        table
            .get(graph)
            .unwrap()
            .map(|value| value.value().to_vec())
    };
    let payload_before = {
        let read = destination.read(&destination_handle).unwrap();
        read.scoped_owner_table(NODES)
            .unwrap()
            .get((graph, "s2-staged"))
            .unwrap()
            .map(|value| value.value().to_vec())
    };
    let fence_before = {
        let read = destination.read(&destination_handle).unwrap();
        medulla_transaction::read_fences(&read)
            .unwrap()
            .expect("the S2 reservation fences its destination")
    };

    // S1 has a durable marker for W.  There is no S1->D reservation, so D
    // must refuse without deleting S2's catalog, payload, or recovery
    // fence.
    medulla_persistence::online_reshard::reserve_graft_destination(&winner, &source, graph).unwrap();
    source
        .mutations()
        .graft_begin(source_handle.as_ref(), &winner_target)
        .unwrap();
    let error = medulla_persistence::online_reshard::graft_graph_from(&destination, &source, graph).unwrap_err();
    assert!(
        error.contains("different authority or destination"),
        "got: {error}"
    );

    assert!(destination.graft_destination_reserved(graph).unwrap());
    let source2_digest = hex::encode(other_source.kernel().owner_authority_digest().unwrap());
    assert!(destination
        .mutations()
        .graft_reservation_matches(&destination_target, &identity, &source2_digest,)
        .unwrap());
    let catalog_after = {
        let read = destination.control_read().unwrap();
        let table = read.open_owner_table(GRAPH_META).unwrap();
        table
            .get(graph)
            .unwrap()
            .map(|value| value.value().to_vec())
    };
    assert_eq!(catalog_after, catalog_before);
    let payload_after = {
        let read = destination.read(&destination_handle).unwrap();
        read.scoped_owner_table(NODES)
            .unwrap()
            .get((graph, "s2-staged"))
            .unwrap()
            .map(|value| value.value().to_vec())
    };
    assert_eq!(payload_after, payload_before);
    let fence_after = {
        let read = destination.read(&destination_handle).unwrap();
        medulla_transaction::read_fences(&read)
            .unwrap()
            .expect("the unrelated reservation remains fenced")
    };
    assert_eq!(
        (fence_after.placement_epoch, fence_after.fencing_token),
        (fence_before.placement_epoch, fence_before.fencing_token)
    );
    assert_eq!(
        (fence_after.placement_epoch, fence_after.fencing_token),
        (u64::MAX, u64::MAX)
    );

    let _ = std::fs::remove_file(&source_path);
    let _ = std::fs::remove_file(&other_source_path);
    let _ = std::fs::remove_file(&winner_path);
    let _ = std::fs::remove_file(&destination_path);
}

/// A graph owner may have staged rows while two destination reservations
/// race. A different authenticated source marker can cancel the losing
/// reservation only after the GraphShard retirement capability purges that
/// payload in the same destination transaction; reopening the graph must
/// then start from the empty v0 baseline.
#[cfg(feature = "server")]
#[test]
fn losing_graph_reservation_retires_staged_owner_rows_before_reopening() {
    let source_path = temp_path("graft-owner-source");
    let winner_path = temp_path("graft-owner-winner");
    let loser_path = temp_path("graft-owner-loser");
    let source = Shard::open(&source_path).unwrap();
    let winner = Shard::open(&winner_path).unwrap();
    let loser = Shard::open(&loser_path).unwrap();
    let source_handle = source.graph("graph-a").unwrap();
    let winner_handle = winner.graph("graph-a").unwrap();
    let loser_handle = loser.graph("graph-a").unwrap();
    let winner_target =
        GraftDestination::new(winner.mutations(), winner.kernel(), winner_handle.as_ref());
    // Reserve and stage through the authenticated destination path before
    // the source marker chooses the winner. This is the case a
    // LedgerOnly-only cancellation test cannot distinguish.
    medulla_persistence::online_reshard::reserve_graft_destination(&loser, &source, "graph-a").unwrap();
    loser
        .stage_reserved_graft_owner_payload("graph-a", |write| {
            write
                .open_scoped_table(NODES)?
                .insert(("graph-a", "loser-staged"), b"staged".as_slice())
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .unwrap();
    let staged = loser.read(&loser_handle).unwrap();
    assert!(staged
        .scoped_owner_table(NODES)
        .unwrap()
        .get(("graph-a", "loser-staged"))
        .unwrap()
        .is_some());
    drop(staged);
    // `graph_meta` is file-wide, so this deliberately models the separate
    // control transaction used by the reshard importer.  The target graph
    // had no catalog at reservation time; an unrelated graph's catalog is
    // the preservation control against an over-broad cleanup.
    medulla_store::redb_store::store_read::write_graph_meta(
        &loser,
        "graph-b",
        "stable-graph-b",
        medulla_protocol_types::protocol::GraphType::Global,
    )
    .unwrap();
    medulla_store::redb_store::store_read::write_graph_meta(
        &loser,
        "graph-a",
        "staged-graph-a",
        medulla_protocol_types::protocol::GraphType::Global,
    )
    .unwrap();

    medulla_persistence::online_reshard::reserve_graft_destination(&winner, &source, "graph-a").unwrap();
    source
        .mutations()
        .graft_begin(source_handle.as_ref(), &winner_target)
        .unwrap();
    *FAIL_AFTER_GRAFT_CATALOG_CLEANUP.lock().unwrap() = Some("graph-a".to_string());
    let retry_error =
        medulla_persistence::online_reshard::graft_graph_from(&loser, &source, "graph-a").unwrap_err();
    assert!(
        retry_error.contains("injected failure after durable graft catalog cleanup"),
        "{retry_error}"
    );
    assert!(loser.graft_destination_reserved("graph-a").unwrap());
    assert!(
        medulla_store_apply::redb_store::dump::read_catalog_record(&loser, "graph-a")
            .unwrap()
            .is_none()
    );
    assert!(
        medulla_store_apply::redb_store::dump::read_catalog_record(&loser, "graph-b")
            .unwrap()
            .is_some()
    );
    let retained = loser.read(&loser_handle).unwrap();
    assert!(retained
        .scoped_owner_table(NODES)
        .unwrap()
        .get(("graph-a", "loser-staged"))
        .unwrap()
        .is_some());
    let fence = medulla_transaction::read_fences(&retained)
        .unwrap()
        .expect("the retry handle remains fenced");
    assert_eq!(fence.placement_epoch, u64::MAX);
    assert_eq!(fence.fencing_token, u64::MAX);
    drop(retained);

    let error = medulla_persistence::online_reshard::graft_graph_from(&loser, &source, "graph-a").unwrap_err();
    assert!(
        error.contains("different authority or destination"),
        "{error}"
    );

    assert!(!loser.graft_destination_reserved("graph-a").unwrap());
    let after_cancel = loser.read(&loser_handle).unwrap();
    assert!(after_cancel
        .scoped_owner_table(NODES)
        .unwrap()
        .get(("graph-a", "loser-staged"))
        .unwrap()
        .is_none());
    drop(after_cancel);
    assert!(
        medulla_store_apply::redb_store::dump::read_catalog_record(&loser, "graph-a")
            .unwrap()
            .is_none()
    );
    assert!(
        medulla_store_apply::redb_store::dump::read_catalog_record(&loser, "graph-b")
            .unwrap()
            .is_some()
    );
    let _ = winner_target;
    drop(loser_handle);
    drop(loser);
    let loser = Shard::open(&loser_path).unwrap();
    let loser_handle = loser.graph("graph-a").unwrap();
    assert!(!loser.graft_destination_reserved("graph-a").unwrap());

    // The canceled loser is a genuine empty v0 scope again, so the next
    // owner write is admitted and visible without inheriting staged rows.
    let members = members(&loser, &["graph-a"]);
    let (group, batches) = loser.admit_drain(&members, "loser-after-cancel").unwrap();
    let write = ShardWrite::open(&loser, &group, &members, &batches).unwrap();
    write
        .graph("graph-a")
        .unwrap()
        .open_scoped_table(NODES)
        .unwrap()
        .insert(("graph-a", "loser-live"), b"live".as_slice())
        .unwrap();
    write.finish().unwrap();
    loser.commit_drain(group, &batches, 2).unwrap();
    let reopened = loser.read(&loser_handle).unwrap();
    let nodes = reopened.scoped_owner_table(NODES).unwrap();
    assert!(nodes.get(("graph-a", "loser-staged")).unwrap().is_none());
    assert!(nodes.get(("graph-a", "loser-live")).unwrap().is_some());
    drop(reopened);

    let _ = std::fs::remove_file(&source_path);
    let _ = std::fs::remove_file(&winner_path);
    let _ = std::fs::remove_file(&loser_path);
}

/// A reserved import's owner stage and catalog commit must not be
/// interleaved with wrong-target cleanup.  The stage deliberately pauses
/// before its second transaction; cleanup enters the same graph protocol
/// and therefore cannot delete the catalog and cancel the reservation while
/// the importer is still able to rewrite it.
#[cfg(feature = "server")]
#[test]
fn reserved_import_and_loser_cleanup_are_one_graph_protocol() {
    let source_path = temp_path("graft-protocol-source");
    let winner_path = temp_path("graft-protocol-winner");
    let loser_path = temp_path("graft-protocol-loser");
    let source = Arc::new(Shard::open(&source_path).unwrap());
    let winner = Arc::new(Shard::open(&winner_path).unwrap());
    let loser = Arc::new(Shard::open(&loser_path).unwrap());
    let source_handle = source.graph("graph-protocol").unwrap();
    let winner_handle = winner.graph("graph-protocol").unwrap();
    let loser_handle = loser.graph("graph-protocol").unwrap();
    let winner_target =
        GraftDestination::new(winner.mutations(), winner.kernel(), winner_handle.as_ref());

    medulla_persistence::online_reshard::reserve_graft_destination(&loser, source.as_ref(), "graph-protocol")
        .unwrap();
    loser
        .stage_reserved_graft_owner_payload("graph-protocol", |write| {
            write
                .open_scoped_table(NODES)?
                .insert(
                    ("graph-protocol", "staged-before-import"),
                    b"staged".as_slice(),
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .unwrap();
    medulla_store::redb_store::store_read::write_graph_meta(
        loser.as_ref(),
        "graph-b",
        "stable-graph-b",
        medulla_protocol_types::protocol::GraphType::Global,
    )
    .unwrap();
    // The catalog NAME must sanitize back to the physical key: an exported
    // raw image is refused by `RawGraphRows::durable_identity` ("raw graph
    // rows durable identity does not match its key") when it does not, so
    // the old "staged-graph-protocol" spelling made `import_graph_raw`
    // fail on its FIRST statement -- before the staged-payload hook that
    // this test's rendezvous waits on, which then timed out after 120s and
    // reported a fixture error as a concurrency hang. The staging this test
    // is about is the reserved owner payload written above, not the display
    // name. Corrected 2026-09-11 by the redb_store absolute-green lane (F1).
    medulla_store::redb_store::store_read::write_graph_meta(
        loser.as_ref(),
        "graph-protocol",
        "graph-protocol",
        medulla_protocol_types::protocol::GraphType::Global,
    )
    .unwrap();
    let mut rows =
        medulla_persistence::online_reshard::export_graph_raw(loser.as_ref(), "graph-protocol").unwrap();
    rows.requires_graft_reservation = true;
    let stale_rows = rows.clone();

    medulla_persistence::online_reshard::reserve_graft_destination(&winner, source.as_ref(), "graph-protocol")
        .unwrap();
    source
        .mutations()
        .graft_begin(source_handle.as_ref(), &winner_target)
        .unwrap();

    let stage_barrier = Arc::new(Barrier::new(2));
    assert!(IMPORT_AFTER_RESERVED_STAGE
        .set(Mutex::new(Some((
            "graph-protocol".to_string(),
            Arc::clone(&stage_barrier),
        ))))
        .is_ok());
    let graft_start_barrier = Arc::new(Barrier::new(2));
    assert!(GRAFT_BEFORE_PROTOCOL_LOCK
        .set(Mutex::new(Some((
            "graph-protocol".to_string(),
            Arc::clone(&graft_start_barrier),
        ))))
        .is_ok());
    let import_loser = Arc::clone(&loser);
    let import = std::thread::spawn(move || {
        medulla_persistence::online_reshard::import_graph_raw(import_loser.as_ref(), "graph-protocol", &rows)
    });
    // The import owns the protocol guard and is paused after its scoped
    // owner transaction, before its separate catalog transaction.
    meet(&stage_barrier, "import staged-payload rendezvous");

    let cleanup_loser = Arc::clone(&loser);
    let cleanup_source = Arc::clone(&source);
    let cleanup = std::thread::spawn(move || {
        medulla_persistence::online_reshard::graft_graph_from(
            &cleanup_loser,
            cleanup_source.as_ref(),
            "graph-protocol",
        )
    });
    // Ensure the competing cleanup has entered the graft operation before
    // probing the actual per-graph mutex held by the paused import.  The
    // cleanup then blocks on that same mutex until the import's catalog
    // transaction completes.
    meet(&graft_start_barrier, "competing graft-start rendezvous");
    let protocol_probe = loser.graft_protocol_guard("graph-protocol").unwrap();
    if protocol_probe.try_lock().is_ok() {
        // Unblock both workers before failing, so the regression never
        // leaves redb files or test threads live after the assertion.
        meet(&stage_barrier, "import staged-payload rendezvous");
        let _ = join_bounded(import, "the paused graft import");
        let _ = join_bounded(cleanup, "the competing graft cleanup");
        panic!("loser cleanup crossed catalog deletion while import held the graph protocol");
    }

    meet(&stage_barrier, "import staged-payload rendezvous");
    assert!(join_bounded(import, "the paused graft import").is_ok());
    let cleanup_result = join_bounded(cleanup, "the competing graft cleanup");
    assert!(cleanup_result.is_err());

    assert!(!loser.graft_destination_reserved("graph-protocol").unwrap());
    assert!(
        medulla_store_apply::redb_store::dump::read_catalog_record(loser.as_ref(), "graph-protocol")
            .unwrap()
            .is_none()
    );
    assert!(
        medulla_store_apply::redb_store::dump::read_catalog_record(loser.as_ref(), "graph-b")
            .unwrap()
            .is_some()
    );
    let stale_import =
        medulla_persistence::online_reshard::import_graph_raw(loser.as_ref(), "graph-protocol", &stale_rows)
            .unwrap_err();
    assert!(stale_import.contains("lost its destination reservation"));
    let read = loser.read(&loser_handle).unwrap();
    assert!(read
        .scoped_owner_table(NODES)
        .unwrap()
        .get(("graph-protocol", "staged-before-import"))
        .unwrap()
        .is_none());

    let _ = std::fs::remove_file(&source_path);
    let _ = std::fs::remove_file(&winner_path);
    let _ = std::fs::remove_file(&loser_path);
}
