//! Committed-batch fixtures shared by the restore tests and the restore drill
//! bench (`MEDULLA-ENGINE-PARITY-R016.2` and `MEDULLA-ENGINE-PARITY-R016.3`):
//! one scope of single-node batches, an in-memory archive and a bundle writer.

use std::path::Path;
use std::sync::Arc;

use medulla_mutation_types::mutation_scope::{
    DurabilityDomain, IncarnationId, LogicalName, MutationOutboxIntent, MutationScopeIdentity,
    MutationSurface, ScopeTenantId, MUTATION_BATCH_VERSION,
};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::mutation_batch::{
    MutationBatch, MutationOperation, VersionExpectation, MUTATION_ACTOR_HEADER,
};
use medulla_protocol_types::protocol::Method;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_storage::object::archive::ObjectArchive;

pub const TENANT: &str = "engine";
pub const GRAPH: &str = "replay-graph";

pub fn identity() -> MutationScopeIdentity {
    MutationScopeIdentity::graph(
        ScopeTenantId::new("replay-tenant").unwrap(),
        LogicalName::new(GRAPH).unwrap(),
        IncarnationId::new("incarnation:test:backup-replay").unwrap(),
    )
}

/// Batch number `index` of the scope: it commits at sequence `index + 1`.
pub fn batch(index: u64) -> MutationBatch {
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

pub async fn commit_range(backend: &RedbBackend, range: std::ops::Range<u64>) {
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

pub fn archive() -> ObjectArchive {
    ObjectArchive::new(Arc::new(object_store::memory::InMemory::new()), "root")
}

pub fn bundle_of(backend: &RedbBackend, dir: &Path) {
    backend
        .backup(dir, "test", 1_700_000_000, "replay", &[])
        .expect("bundle is written");
}
