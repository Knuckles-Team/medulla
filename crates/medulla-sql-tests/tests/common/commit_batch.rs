//! Shared fixture: one SQL-catalog `MutationBatch` per commit, as the served
//! `commit_sql_catalog_txn` gateway compiles it. Included with `#[path]` by the
//! tests that drive `TableStore::commit_txn_batch` directly.
#![allow(dead_code)]

use medulla_mutation_types::mutation_scope::{
    DurabilityDomain, IncarnationId, LogicalName, MutationOutboxIntent, MutationScopeIdentity,
    MutationSurface, ScopeTenantId, COMPILED_BATCH_INCARNATION, MUTATION_BATCH_VERSION,
};
use medulla_protocol_types::mutation_batch::{
    MutationBatch, MutationEnvelope, MutationOperation, VersionExpectation,
};
use medulla_table_store::tables::TableStore;

pub const TENANT: &str = "tenant-commit-batch";
pub const GRAPH: &str = "graph-commit-batch";

/// Mirrors `sql_context_cache_invalidation.rs`'s `commit()` fixture shape.
pub fn batch(store: &TableStore, batch_id: &str, idempotency_key: &str) -> MutationBatch {
    // Live read, taken fresh for every call so a sequence of commits within one
    // test never trips STALE_VERSION -- same discipline as the old
    // `expected_graph_version: Some(expected)` this replaces.
    let expected = store.mutation_version(TENANT, GRAPH).unwrap();
    // `DurabilityDomain::SqlCatalog` is one of the migration contract's
    // non-graph domains -> native scope. `sql_scope_key` (`medulla-query`'s
    // `tables/store.rs`) reads this batch's `(tenant, resource)` back out via
    // `identity.scope()`'s `Native { resource, .. }` arm and rejects a
    // `Graph`-scoped batch outright, so TENANT/GRAPH map onto tenant/resource
    // here exactly as they did onto the old flat tenant/graph fields.
    let identity = MutationScopeIdentity::native(
        ScopeTenantId::new(TENANT).expect("valid tenant id"),
        DurabilityDomain::SqlCatalog,
        LogicalName::new(GRAPH).expect("valid resource name"),
        IncarnationId::new(COMPILED_BATCH_INCARNATION).expect("valid incarnation id"),
    )
    .expect("sql-catalog native scope identity is valid");
    let mut batch = MutationBatch {
        schema_version: MUTATION_BATCH_VERSION,
        batch_id: batch_id.to_string(),
        envelope: fixture_envelope(&identity, 1, idempotency_key),
        identity,
        placement_epoch: 0,
        version_expectation: VersionExpectation::Native(expected),
        fencing_token: None,
        authoritative_state: None,
        operations: vec![MutationOperation {
            ordinal: 0,
            surface: MutationSurface::Query,
            domain: DurabilityDomain::SqlCatalog,
            method: medulla_protocol_types::protocol::Method::ApplyMutation {
                event_type: "sql_catalog_operation".to_string(),
                query: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
            },
        }],
        outbox: vec![MutationOutboxIntent {
            topic: "engine.projection.rebuild".to_string(),
            key: batch_id.to_string(),
            payload: vec![1],
            headers: Default::default(),
        }],
        created_at_ms: 100,
    };
    batch
        .reseal_envelope(medulla_types::contract::Digest256::from_bytes([0_u8; 32]))
        .expect("a fixture batch reseals its envelope over its final body");
    batch
}

fn fixture_envelope(
    identity: &MutationScopeIdentity,
    request_id: u64,
    idempotency_key: &str,
) -> MutationEnvelope {
    let actor = format!("principal:sha256:{}", "a".repeat(64));
    let method = medulla_types::contract::MethodId::new("sql_catalog_operation").unwrap();
    MutationEnvelope::for_scope(
        medulla_protocol_types::mutation_batch::CompiledScope {
            identity,
            actor: &actor,
            serving_principal: &actor,
            request_id,
            idempotency_key,
            nonce: medulla_types::contract::Nonce::minted(),
            now_ms: 0,
        },
        medulla_protocol_types::mutation_batch::CompiledOperation {
            method_schema_id: medulla_protocol_types::mutation_batch::method_schema_id(&method)
                .unwrap(),
            method,
            method_schema_digest: medulla_types::contract::Digest256::from_bytes([0_u8; 32]),
            canonical_payload_digest: medulla_types::contract::Digest256::from_bytes([1_u8; 32]),
        },
    )
    .unwrap()
}
