//! MEDULLA-DURABLE-KERNEL-R877.3: an import that stopped before it was `ready` leaves its target
//! graph hidden from every read and an `importing` record in the control graph; the recovery pass
//! the leader tick runs deletes the graph and records the import `aborted`. The pass takes its
//! clock as an input, so the test moves time by value and never sleeps. The hiding is the graph's
//! own durable registry marker, not process state: a second test restarts the engine and proves
//! the graph is hidden before any recovery pass has run.

#![cfg(all(feature = "redb", feature = "jobs", feature = "security"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_h_jobs::bundle_import_recovery::recover_bundle_imports;
use medulla_protocol_types::protocol::{GraphType, Method};
use medulla_persistence::AsRedb;
use medulla_protocol_types::protocol::Response;
use medulla_server_auth::access::importing;
use medulla_types::knowledge_bundle::{ImportRecord, ImportState, IMPORT_GRACE_MS};

const SECRET: &str = "bundle-import-recovery-secret";
const ENCRYPTION_KEY: &str = "bundle-import-recovery-encryption-key-0123456789";
const TARGET: &str = "half-imported-copy";
const SIBLING: &str = "ordinary-copy";
const STARTED_MS: u64 = 1_000;

async fn stored_state(state: &test_support::SharedState) -> Option<ImportState> {
    let control = medulla_server_api::knob_config::default_ingest_graph();
    let core = state.read().await.registry.get(control)?.core.clone();
    let id = core.nodes_by_property("type", "BundleImport")?.into_iter().next()?;
    let raw = core.get_node_properties(&id)?;
    let properties: serde_json::Value = medulla_types::msgpack::decode_property_value(&raw).ok()?;
    ImportRecord::from_properties(&properties).map(|record| record.state)
}

fn is_denied(response: &Response) -> bool {
    format!("{:?}", response.error).contains("ACCESS_DENIED")
}

async fn read(state: &test_support::SharedState, id: u64, graph: &str) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, graph, Method::NodeCount)).await
}

/// Mark `graph` as written by import `import_id`, on its durable registry row only.
async fn mark_durably(backend: &test_support::SharedPersistence, graph: &str, import_id: &str) {
    backend
        .as_redb()
        .expect("a redb backend")
        .set_graph_importing(&medulla_kernel::persist::sanitize(graph), Some(import_id))
        .await
        .expect("the import marker is written");
}

// spec: MEDULLA-DURABLE-KERNEL-R877.3
// spec: MEDULLA-DURABLE-KERNEL-R877.4
#[tokio::test(flavor = "multi_thread")]
async fn a_crashed_import_is_deleted_by_the_recovery_pass() {
    let control = medulla_server_api::knob_config::default_ingest_graph();
    let (_dir, backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "bundle-import-recovery",
        common::current_isolation(),
        control,
    )
    .await;
    test_support::call_ok(
        &state,
        SECRET,
        2,
        TARGET,
        Method::CreateGraph {
            graph_name: TARGET.to_string(),
            graph_type: GraphType::Global,
        },
    )
    .await;

    // What a crash leaves: the record says `importing`, the graph exists, and nothing finished.
    let record = ImportRecord::begin(
        common::TEST_TENANT,
        "cas:sha256:00",
        "half-",
        "crash",
        vec![TARGET.to_string()],
        STARTED_MS,
    );
    let operation = record.node_operation("importer");
    let control_core = state.read().await.registry.get(control).expect("control graph").core.clone();
    control_core.add_node(
        operation["id"].as_str().unwrap().to_string(),
        rmp_serde::to_vec_named(&operation["properties"]).unwrap(),
    );
    assert_eq!(stored_state(&state).await, Some(ImportState::Importing));
    assert!(!is_denied(&read(&state, 3, TARGET).await), "an unmarked graph answers");
    mark_durably(&backend, TARGET, &record.import_id).await;
    state.write().await.registry.set_importing(TARGET, Some(record.import_id.clone()));

    // A live lease is never deleted, and the marked graph is hidden from every read.
    let live = record.lease_until_ms - 1;
    assert_eq!(recover_bundle_imports(&state, live).await.expect("pass"), 0);
    assert!(state.read().await.registry.exists(TARGET));
    assert!(is_denied(&read(&state, 4, TARGET).await), "a half-written graph is hidden from every read");
    // The import's own work, under its own id, reaches the graph.
    let own = importing::within_import(record.import_id.clone(), read(&state, 5, TARGET)).await;
    assert!(!is_denied(&own));
    // Another import's work does not.
    let other = importing::within_import("another".to_string(), read(&state, 6, TARGET)).await;
    assert!(is_denied(&other));

    // The importer renews the lease on the record, so a pass past the first lease still leaves
    // the graph alone: the lease is durable state any node reads, not a process set.
    let renewed_at = STARTED_MS + IMPORT_GRACE_MS / 3;
    let renewed = record.renewed(renewed_at);
    let renewal = renewed.node_operation("importer");
    control_core.add_node(
        renewal["id"].as_str().unwrap().to_string(),
        rmp_serde::to_vec_named(&renewal["properties"]).unwrap(),
    );
    assert_eq!(
        recover_bundle_imports(&state, record.lease_until_ms).await.expect("pass"),
        0
    );
    assert!(state.read().await.registry.exists(TARGET));

    // Once the renewed lease lapses the pass deletes the graph and records the import `aborted`.
    let late = renewed.lease_until_ms;
    assert_eq!(recover_bundle_imports(&state, late).await.expect("pass"), 1);
    assert!(!state.read().await.registry.exists(TARGET));
    assert_eq!(stored_state(&state).await, Some(ImportState::Aborted));

    // The pass is idempotent: the aborted record is left alone.
    assert_eq!(recover_bundle_imports(&state, late + 1).await.expect("pass"), 0);
}

// spec: MEDULLA-DURABLE-KERNEL-R877.3
#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_process_hides_a_marked_graph_before_any_recovery_pass() {
    let (dir, backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "bundle-import-restart",
        common::current_isolation(),
        TARGET,
    )
    .await;
    test_support::call_ok(
        &state,
        SECRET,
        2,
        SIBLING,
        Method::CreateGraph {
            graph_name: SIBLING.to_string(),
            graph_type: GraphType::Global,
        },
    )
    .await;
    // The process that began the import wrote the marker with the graph and then stopped; the
    // marker is on the durable row alone, no registry entry of this process carries it.
    mark_durably(&backend, TARGET, "crashed-import").await;

    // Restart: release the first engine, then recover the catalog the way boot does.
    state.write().await.persistence = None;
    drop(state);
    test_support::reopen_after_sole_reference(backend, || Ok::<(), String>(()), "release the first engine").await;
    let restarted = test_support::durable_state_read_through(
        SECRET,
        common::current_isolation(),
        dir.to_str().expect("a utf-8 path"),
    )
    .await;

    // No recovery pass has run: the marker alone hides the graph, and an unmarked graph answers.
    assert!(is_denied(&read(&restarted, 3, TARGET).await), "the restarted process hides the graph");
    assert!(!is_denied(&read(&restarted, 4, SIBLING).await), "an unmarked graph is unaffected");
    assert_eq!(
        restarted.read().await.registry.catalog_record(TARGET).and_then(|record| record.importing),
        Some("crashed-import".to_string())
    );
}
