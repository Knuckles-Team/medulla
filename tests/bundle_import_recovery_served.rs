//! MEDULLA-DURABLE-KERNEL-R877.3: an import that stopped before it was `ready` leaves its target
//! graph hidden from every read and an `importing` record in the control graph; the recovery pass
//! the leader tick runs deletes the graph and records the import `aborted`. The pass takes its
//! clock as an input, so the test moves time by value and never sleeps.

#![cfg(all(feature = "redb", feature = "jobs", feature = "security"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_h_jobs::bundle_import_recovery::recover_bundle_imports;
use medulla_protocol_types::protocol::{GraphType, Method};
use medulla_server_auth::access::importing;
use medulla_types::knowledge_bundle::{ImportRecord, ImportState, IMPORT_GRACE_MS};

const SECRET: &str = "bundle-import-recovery-secret";
const ENCRYPTION_KEY: &str = "bundle-import-recovery-encryption-key-0123456789";
const TARGET: &str = "half-imported-copy";
const STARTED_MS: u64 = 1_000;

async fn stored_state(state: &test_support::SharedState) -> Option<ImportState> {
    let control = medulla_server_api::knob_config::default_ingest_graph();
    let core = state.read().await.registry.get(control)?.core.clone();
    let id = core.nodes_by_property("type", "BundleImport").ok()?.into_iter().next()?;
    let raw = core.get_node_properties(&id)?;
    let properties: serde_json::Value = medulla_types::msgpack::decode_property_value(&raw).ok()?;
    ImportRecord::from_properties(&properties).map(|record| record.state)
}

// spec: MEDULLA-DURABLE-KERNEL-R877.3
// spec: MEDULLA-DURABLE-KERNEL-R877.4
#[tokio::test(flavor = "multi_thread")]
async fn a_crashed_import_is_deleted_by_the_recovery_pass() {
    let control = medulla_server_api::knob_config::default_ingest_graph();
    let (_dir, _backend, state) = test_support::durable_graph_state(
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

    // Before the grace passes the graph is hidden again and nothing is deleted.
    let early = STARTED_MS + IMPORT_GRACE_MS - 1;
    assert_eq!(recover_bundle_imports(&state, early).await.expect("pass"), 0);
    assert!(state.read().await.registry.exists(TARGET));
    assert!(importing::hidden_from_current(TARGET), "a half-written graph is hidden from every read");

    // An import this process is still running is never touched, however old its record is.
    importing::mark_live(&record.import_id);
    assert_eq!(recover_bundle_imports(&state, early + IMPORT_GRACE_MS).await.expect("pass"), 0);
    assert!(state.read().await.registry.exists(TARGET));
    importing::mark_done(&record.import_id);

    // Once the grace passes the pass deletes the graph and records the import `aborted`.
    let late = STARTED_MS + IMPORT_GRACE_MS;
    assert_eq!(recover_bundle_imports(&state, late).await.expect("pass"), 1);
    assert!(!state.read().await.registry.exists(TARGET));
    assert_eq!(stored_state(&state).await, Some(ImportState::Aborted));
    assert!(!importing::hidden_from_current(TARGET));

    // The pass is idempotent: the aborted record is left alone.
    assert_eq!(recover_bundle_imports(&state, late + 1).await.expect("pass"), 0);
}
