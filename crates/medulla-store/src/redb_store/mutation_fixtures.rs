//! Shared mutation-batch fixtures: the shard opener, the canonical two-node
//! batch and the commit helpers the store's own mutation-batch tests and the
//! facade's cross-crate store tests both build on.

use super::*;
use medulla_types::mutation_batch::{
    DurabilityDomain, IncarnationId, LogicalName, MutationOperation, MutationOutboxIntent,
    MutationScopeIdentity, MutationSurface, ScopeTenantId, MUTATION_BATCH_VERSION,
};

pub fn open(path: &std::path::Path) -> Shard {
    let shard = Shard::open(path).unwrap();
    // The old fixture seeded the retired graph-version table at 3.  Advance
    // the kernel-owned ledger through three real maintenance admissions so
    // every test keeps the same OCC starting point without recreating a
    // second version authority.
    let members = shard.graph_members(&["graph-a"]).unwrap();
    // Reopening an existing fixture must not re-admit the same maintenance
    // seed keys: the kernel correctly resolves those members as Replay, and
    // replayed members are forbidden from opening owner rows. Seed only a
    // graph whose authoritative ledger version row is absent.
    // Freshness cannot be probed by "is the version row absent?", because this
    // read is what CREATES that row: `read_mutation_graph_version` opens the
    // graph, and `Shard::graph` binds a cold scope (`bind_scope`) which inserts
    // `INITIAL_GRAPH_VERSION`. The absent-row arm below is therefore
    // unreachable through this path, and reading it as "already seeded" left
    // every fixture at version 0 while 47 of them expect the seeded base of 3
    // -- 42 tests failing closed with `STALE_VERSION: expected version 3 but
    // authoritative version is 0`.
    //
    // An unadvanced ledger is the real freshness signal: only seeding moves
    // graph-a off `INITIAL_GRAPH_VERSION`, so a reopened fixture is at 3 or
    // more and is still correctly left alone.
    let seed_required = match read_mutation_graph_version(&shard, "graph-a") {
        Ok(version) => version == INITIAL_GRAPH_VERSION,
        Err(error) if error == "mutation scope binding is missing its version row" => true,
        Err(error) => panic!("unexpected graph version read failure: {error}"),
    };
    if seed_required {
        for index in 0..3 {
            let op_id = format!("mutation-test-seed/{index}");
            let (group, batches) = shard.admit_maintenance(&members, &op_id).unwrap();
            let write = ShardWrite::open(&shard, &group, &members, &batches).unwrap();
            write.finish().unwrap();
            shard.commit_drain(group, &batches, 0).unwrap();
        }
    }
    shard
}

pub fn node(id: &str, value: i64) -> Method {
    Method::AddNode {
        node_id: id.to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({"value": value})).unwrap(),
    }
}

pub fn batch(batch_id: &str, key: &str) -> MutationBatch {
    let identity = MutationScopeIdentity::graph(
        ScopeTenantId::new("tenant-a").unwrap(),
        LogicalName::new("graph-a").unwrap(),
        IncarnationId::new("incarnation:test:redb-store").unwrap(),
    );
    let mut batch = MutationBatch {
        schema_version: MUTATION_BATCH_VERSION,
        batch_id: batch_id.to_string(),
        envelope: fixture_operation_envelope(
            &identity,
            &format!("principal:sha256:{}", "a".repeat(64)),
            42,
            key,
        ),
        identity,
        placement_epoch: 7,
        version_expectation: VersionExpectation::Graph(3),
        fencing_token: Some(9),
        authoritative_state: None,
        operations: vec![
            MutationOperation {
                ordinal: 0,
                surface: MutationSurface::Transaction,
                domain: DurabilityDomain::GraphRows,
                method: node("a", 1),
            },
            MutationOperation {
                ordinal: 1,
                surface: MutationSurface::Transaction,
                domain: DurabilityDomain::GraphRows,
                method: node("b", 2),
            },
        ],
        outbox: vec![MutationOutboxIntent {
            topic: "projection.test".to_string(),
            key: batch_id.to_string(),
            payload: vec![1, 2, 3],
            headers: Default::default(),
        }],
        created_at_ms: 100,
    };
    batch
        .reseal_envelope(medulla_types::contract::Digest256::from_bytes([1_u8; 32]))
        .expect("a fixture batch reseals its envelope over its final body");
    batch
}

pub fn ready_work_item_method(work_item_id: &str, max_attempts: u64) -> Method {
    Method::AddNode {
        node_id: work_item_id.to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
            "node_type": "WorkItem",
            "tenant": "tenant-a",
            "status": "ready",
            "max_attempts": max_attempts,
        }))
        .unwrap(),
    }
}

// Test-only fixture builder: every parameter is an independent field of
// the `ClaimWorkItemRequest` under construction; no natural grouping.
#[allow(clippy::too_many_arguments)]
pub fn native_claim_batch(
    batch_id: &str,
    idempotency_key: &str,
    expected_graph_version: u64,
    work_item_id: Option<&str>,
    worker_id: &str,
    now_ms: u64,
    lease_ms: u64,
    max_tenant_in_flight: u64,
) -> MutationBatch {
    let mut claim = batch(batch_id, idempotency_key);
    claim.version_expectation = VersionExpectation::Graph(expected_graph_version);
    claim.operations = vec![MutationOperation {
        ordinal: 0,
        surface: MutationSurface::Transaction,
        domain: DurabilityDomain::GraphRows,
        method: Method::ClaimWorkItem {
            request: medulla_types::epistemic_operations::ClaimWorkItemRequest {
                schema_version:
                    medulla_types::epistemic_operations::ClaimWorkItemRequestSchemaVersion::V1,
                tenant_ref: "tenant-a".into(),
                work_item_id: work_item_id.map(str::to_string),
                queue_ref: None,
                resource_class: None,
                fairness_group: None,
                worker_ref: worker_id.into(),
                now_ms,
                lease_ms,
                max_tenant_in_flight,
            },
        },
    }];
    claim
        .reseal_envelope(medulla_types::contract::Digest256::from_bytes([1_u8; 32]))
        .expect("native claim fixture reseals its final body");
    claim
}

// Test-only fixture builder; mirrors `native_claim_batch`'s justification
// above plus the `db` handle it commits the built batch against.
#[allow(clippy::too_many_arguments)]
pub fn commit_native_claim(
    shard: &Shard,
    batch_id: &str,
    idempotency_key: &str,
    expected_graph_version: u64,
    work_item_id: Option<&str>,
    worker_id: &str,
    now_ms: u64,
    lease_ms: u64,
    max_tenant_in_flight: u64,
) -> ClaimWorkItemResult {
    let committed = commit_at(
        shard,
        &native_claim_batch(
            batch_id,
            idempotency_key,
            expected_graph_version,
            work_item_id,
            worker_id,
            now_ms,
            lease_ms,
            max_tenant_in_flight,
        ),
        None,
    )
    .unwrap();
    let payload: medulla_types::protocol::ResultPayload = decode_durable(
        committed
            .record
            .result_msgpack
            .as_deref()
            .expect("claim result"),
    )
    .unwrap();
    let bytes = match payload {
        medulla_types::protocol::ResultPayload::Raw(inner) => inner,
        other => panic!("ClaimWorkItem must return a bin-encoded typed result, got {other:?}"),
    };
    decode_durable(&bytes).unwrap()
}

pub fn commit_at_graph(
    shard: &Shard,
    graph_fname: &str,
    batch: &MutationBatch,
    point: Option<MutationBatchCrashpoint>,
) -> Result<MutationBatchCommit, String> {
    #[cfg(feature = "security")]
    let mut audit = AuditTailCache::new();
    commit_mutation_batch_inner(
        shard,
        BatchCommitInput::compact(graph_fname, batch, Some(&[0x81, 0xa2, b'o', b'k']), 101)
            .with_crashpoint(point),
        DurableCrypto::none(),
        #[cfg(feature = "security")]
        &mut audit,
    )
}

pub fn commit_at(
    shard: &Shard,
    batch: &MutationBatch,
    point: Option<MutationBatchCrashpoint>,
) -> Result<MutationBatchCommit, String> {
    commit_at_graph(shard, "graph-a", batch, point)
}
