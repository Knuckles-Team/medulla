"""MEDULLA-DURABLE-KERNEL-R003: hashed, signed and at-rest identity tags keep their original bytes.

The rename cutover rewrote ~80 digest-domain, signature-domain, marker and
persisted-key strings from their `eg`/`epistemic-graph` form to `medulla`,
which silently moved every stored id, digest and signature. Each tag below is
pinned to the exact original literal in the source file (or client) that
feeds it. Renaming one fails here, naming the file and the tag.

Each row is (file, original tag as written in source, the renamed form that
must NOT reappear).  `\\0` is the literal two-character escape in source.
"""

from __future__ import annotations

from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.no_engine

PINNED_TAGS: tuple[tuple[str, str, str], ...] = (
    ("clients/go/client.go", "eg-envelope-v2", "medulla-envelope-v2"),
    ("clients/go/client.go", "eg-multisig-mutation-v2", "medulla-multisig-mutation-v2"),
    ("clients/go/client.go", "eg-register-identity-v2", "medulla-register-identity-v2"),
    ("clients/js/index.mjs", "eg-envelope-v2", "medulla-envelope-v2"),
    ("clients/js/index.mjs", "eg-multisig-mutation-v2", "medulla-multisig-mutation-v2"),
    ("clients/js/index.mjs", "eg-register-identity-v2", "medulla-register-identity-v2"),
    ("crates/medulla-h-graph/src/topology.rs", "epistemic-graph/cluster-discovery/v1\\0", "medulla-engine/cluster-discovery/v1\\0"),
    ("crates/medulla-h-graph/src/topology.rs", "epistemic-graph/single-node-cluster/v1", "medulla/single-node-cluster/v1"),
    ("crates/medulla-h-jobs/src/jobs.rs", "eg-jobs-v1", "medulla-jobs-v1"),
    ("crates/medulla-h-txn/src/txn/consensus.rs", "epistemic-graph-consensus-participant-v1\\0", "medulla-consensus-participant-v1\\0"),
    ("crates/medulla-jobs/src/model.rs", "eg-jobs.params_digest.v1\\0", "medulla-jobs.params_digest.v1\\0"),
    ("crates/medulla-jobs/src/model.rs", "eg-jobs.result_ref.v2\\0", "medulla-jobs.result_ref.v2\\0"),
    ("crates/medulla-jobs/src/result.rs", "eg-jobs.knowledge-batch.v1\\0", "medulla-jobs.knowledge-batch.v1\\0"),
    ("crates/medulla-jobs/src/store.rs", "eg-jobs.consensus-job-id.v1\\0", "medulla-jobs.consensus-job-id.v1\\0"),
    ("crates/medulla-jobs/src/store/index_keys.rs", "eg-jobs.capability-index.v1\\0", "medulla-jobs.capability-index.v1\\0"),
    ("crates/medulla-jobs/src/store/index_keys.rs", "eg-jobs.tenant-index.v1\\0", "medulla-jobs.tenant-index.v1\\0"),
    ("crates/medulla-kernel/src/audit.rs", "EG-PROVENANCE-ANCHOR-MISSING-NODE", "MEDULLA-PROVENANCE-ANCHOR-MISSING-NODE"),
    ("crates/medulla-kernel/src/crypto.rs", "epistemic-graph-encryption-canary", "medulla-encryption-canary"),
    ("crates/medulla-lake/src/iceberg.rs", "epistemic-graph-lsn", "medulla-lsn"),
    ("crates/medulla-lake/src/parquet_io.rs", "epistemic_graph.lake_schema.v1", "medulla.lake_schema.v1"),
    ("crates/medulla-persistence-records/src/decision_record.rs", "eg-decision:", "medulla-decision:"),
    ("crates/medulla-persistence/src/node_info_store.rs", "epistemic-graph/member-identity/v1\\0", "medulla/member-identity/v1\\0"),
    ("crates/medulla-persistence/src/redb_backend.rs", "epistemic-graph-encryption-canary", "medulla-encryption-canary"),
    ("crates/medulla-persistence/src/shard_migrate/readiness.rs", "eg-shard-migration-v2", "medulla-shard-migration-v2"),
    ("crates/medulla-program/src/commit.rs", "eg-program.active-pointer.v1\\0", "medulla-program.active-pointer.v1\\0"),
    ("crates/medulla-program/src/commit.rs", "eg-program.candidate-authority.v1\\0", "medulla-program.candidate-authority.v1\\0"),
    ("crates/medulla-program/src/commit.rs", "eg-program.candidate.v3\\0", "medulla-program.candidate.v3\\0"),
    ("crates/medulla-program/src/commit.rs", "eg-program.revision.v3\\0", "medulla-program.revision.v3\\0"),
    ("crates/medulla-program/src/optimizer.rs", "eg-program.candidate.v3\\0", "medulla-program.candidate.v3\\0"),
    ("crates/medulla-program/src/optimizer.rs", "eg-program.plan.v1\\0", "medulla-program.plan.v1\\0"),
    ("crates/medulla-program/src/optimizer.rs", "eg-program.random-search.v1\\0", "medulla-program.random-search.v1\\0"),
    ("crates/medulla-protocol-types/src/protocol.rs", "eg-envelope-v2", "medulla-envelope-v2"),
    ("crates/medulla-quantum-jobs/src/circuit.rs", "eg-quantum-jobs.induced_subgraph_ghz", "medulla-quantum-jobs.induced_subgraph_ghz"),
    ("crates/medulla-quantum-jobs/src/job.rs", "eg-quantum-jobs/", "medulla-quantum-jobs/"),
    ("crates/medulla-quantum-jobs/src/job.rs", "eg-quantum-sim", "medulla-quantum-sim"),
    ("crates/medulla-raft-xshard/src/cross_node_elasticity/mod.rs", "epistemic-graph/cross-node-delta/v1\\0", "medulla/cross-node-delta/v1\\0"),
    ("crates/medulla-raft-xshard/src/cross_node_elasticity/mod.rs", "epistemic-graph/cross-node-fence/v1\\0", "medulla/cross-node-fence/v1\\0"),
    ("crates/medulla-raft-xshard/src/cross_node_elasticity/mod.rs", "epistemic-graph/cross-node-input/v1\\0", "medulla/cross-node-input/v1\\0"),
    ("crates/medulla-raft-xshard/src/cross_node_elasticity/mod.rs", "epistemic-graph/cross-node-move/v1\\0", "medulla/cross-node-move/v1\\0"),
    ("crates/medulla-raft-xshard/src/cross_node_elasticity/mod.rs", "epistemic-graph/cross-node-proposal/v1\\0", "medulla/cross-node-proposal/v1\\0"),
    ("crates/medulla-raft-xshard/src/cross_node_elasticity/mod.rs", "epistemic-graph/cross-node-snapshot/v1\\0", "medulla/cross-node-snapshot/v1\\0"),
    ("crates/medulla-raft-xshard/src/cross_shard_txn/prepare_record.rs", "eg.graph-prepare.v1\\0", "medulla.graph-prepare.v1\\0"),
    ("crates/medulla-raft/src/raft/command/native/sealed.rs", "epistemic-graph-native-raft-v1\\0", "medulla-native-raft-v1\\0"),
    ("crates/medulla-raft/src/raft/config.rs", "epistemic-graph/cluster-id/v1\\0", "medulla-engine/cluster-id/v1\\0"),
    ("crates/medulla-raft/src/raft/config.rs", "epistemic-graph/raft-transport-key/v1\\0", "medulla-engine/raft-transport-key/v1\\0"),
    ("crates/medulla-raft/src/raft/membership_shrink.rs", "epistemic-graph/membership-shrink/v1\\0", "medulla/membership-shrink/v1\\0"),
    ("crates/medulla-raft/src/raft/network.rs", "epistemic-graph/raft-nonce/v1\\0", "medulla-engine/raft-nonce/v1\\0"),
    ("crates/medulla-raft/src/raft/network.rs", "epistemic-graph/raft-session/v1\\0", "medulla-engine/raft-session/v1\\0"),
    ("crates/medulla-raft/src/raft/placement/move_journal.rs", "epistemic-graph/partition-move/v1\\0", "medulla/partition-move/v1\\0"),
    ("crates/medulla-raft/src/raft/reshard.rs", "epistemic-graph/partition-move-lock/v1\\0", "medulla/partition-move-lock/v1\\0"),
    ("crates/medulla-server-api/src/api/persistence.rs", "epistemic-graph-raft-control-plane", "medulla-raft-control-plane"),
    ("crates/medulla-server-api/src/batch_compile/digest.rs", "epistemic-graph.work-item-terminal.v1", "medulla.work-item-terminal.v1"),
    ("crates/medulla-server-api/src/viz_engine.rs", "epistemic-graph.viz_render_cache_key.v1\\0", "medulla.viz_render_cache_key.v1\\0"),
    ("crates/medulla-server-auth/src/auth.rs", "eg-multisig-mutation-v2", "medulla-multisig-mutation-v2"),
    ("crates/medulla-server-auth/src/auth.rs", "eg-register-identity-v2", "medulla-register-identity-v2"),
    ("crates/medulla-server-auth/src/sql_catalog_acl.rs", "epistemic-graph/sql-catalog-debug-principal\\0", "medulla/sql-catalog-debug-principal\\0"),
    ("crates/medulla-server-auth/src/sql_catalog_acl.rs", "epistemic-graph/sql-semantic-complete-snapshot-v1", "medulla/sql-semantic-complete-snapshot-v1"),
    ("crates/medulla-server-auth/src/sql_catalog_acl.rs", "epistemic-graph/sql-semantic-cursor-position-v1", "medulla/sql-semantic-cursor-position-v1"),
    ("crates/medulla-server-auth/src/sql_catalog_acl.rs", "epistemic-graph/sql-semantic-cursor-tag-v1", "medulla/sql-semantic-cursor-tag-v1"),
    ("crates/medulla-server-auth/src/sql_catalog_acl.rs", "epistemic-graph/sql-semantic-record-identity-v1", "medulla/sql-semantic-record-identity-v1"),
    ("crates/medulla-server-auth/src/sql_catalog_acl.rs", "epistemic-graph/sql-source-acl-decision", "medulla/sql-source-acl-decision"),
    ("crates/medulla-server-auth/src/sql_catalog_acl.rs", "epistemic-graph/sql-source-acl-operation", "medulla/sql-source-acl-operation"),
    ("crates/medulla-server-auth/src/sql_catalog_acl.rs", "epistemic-graph/sql-source-acl-state", "medulla/sql-source-acl-state"),
    ("crates/medulla-server-auth/src/sql_catalog_acl.rs", "epistemic-graph/sql-source-operation-id", "medulla/sql-source-operation-id"),
    ("crates/medulla-server-auth/src/sql_tables.rs", "epistemic-graph/sql-catalog-owner\\0", "medulla/sql-catalog-owner\\0"),
    ("crates/medulla-server-auth/src/sql_tables.rs", "epistemic-graph/sql-catalog-registry\\0", "medulla/sql-catalog-registry\\0"),
    ("crates/medulla-server-auth/src/sql_tables.rs", "epistemic-graph/sql-catalog-tenant\\0", "medulla/sql-catalog-tenant\\0"),
    ("crates/medulla-server-lake/src/lake/mod.rs", "epistemic-graph-lsn", "medulla-lsn"),
    ("crates/medulla-server-lake/src/policy_export/mod.rs", "eg-policy-bundle-epoch-v1\\0", "medulla-policy-bundle-epoch-v1\\0"),
    ("crates/medulla-server-mutation/src/mutation_batch/commit/work_item.rs", "epistemic-graph.authenticated-work-item.v1", "medulla.authenticated-work-item.v1"),
    ("crates/medulla-server-obs/src/obs/mod.rs", "epistemic-graph/observability-stream\\0", "medulla/observability-stream\\0"),
    ("crates/medulla-server-ops/src/api/query_txn/txn_intent.rs", "epistemic-graph/txn-intent-compensation\\0", "medulla/txn-intent-compensation\\0"),
    ("crates/medulla-server-ops/src/api/query_txn/txn_intent.rs", "epistemic-graph/txn-intent-file\\0", "medulla/txn-intent-file\\0"),
    ("crates/medulla-server-ops/src/api/query_txn/txn_intent.rs", "epistemic-graph/txn-intent-owner\\0", "medulla/txn-intent-owner\\0"),
    ("crates/medulla-server/src/server_startup.rs", "epistemic-graph anonymous local store v1\\n", "medulla anonymous local store v1\\n"),
    ("crates/medulla-statechart/src/model.rs", "eg-statechart.def_id.v1\\0", "medulla-statechart.def_id.v1\\0"),
    ("crates/medulla-statechart/src/store/batches.rs", "eg-statechart.consensus-instance-id.v1\\0", "medulla-statechart.consensus-instance-id.v1\\0"),
    ("crates/medulla-store/src/redb_store/store_read.rs", "epistemic-graph-incarnation-legacy-v1\\0", "medulla-incarnation-legacy-v1\\0"),
    ("crates/medulla-store/src/redb_store/store_read.rs", "epistemic-graph-incarnation-v1\\0", "medulla-incarnation-v1\\0"),
    ("crates/medulla-table-store/src/tables/embedding_binding.rs", "epistemic-graph/embedding-model-digest\\0", "medulla/embedding-model-digest\\0"),
    ("crates/medulla-table-store/src/tables/property_graph/catalog.rs", "epistemic-graph/sql-pgq/catalog-record/v2\\0", "medulla/sql-pgq/catalog-record/v2\\0"),
    ("crates/medulla-table-store/src/tables/property_graph/catalog.rs", "epistemic-graph/sql-pgq/dependencies/v1\\0", "medulla/sql-pgq/dependencies/v1\\0"),
    ("crates/medulla-table-store/src/tables/property_graph/model.rs", "epistemic-graph/sql-pgq/property-graph/v1\\0", "medulla/sql-pgq/property-graph/v1\\0"),
    ("crates/medulla-table-store/src/tables/property_graph/validate.rs", "epistemic-graph/sql-pgq/statement/v1\\0", "medulla/sql-pgq/statement/v1\\0"),
    ("crates/medulla-tables/src/migration.rs", "epistemic-graph/schema-migration-v1\\0", "medulla/schema-migration-v1\\0"),
    ("crates/medulla-tables/src/schema.rs", "epistemic-graph/table-schema-digest\\0", "medulla/table-schema-digest\\0"),
    ("crates/medulla-viz-columnstore/src/chunk.rs", "eg-viz-columnstore.chunk.v1\\0", "medulla-viz-columnstore.chunk.v1\\0"),
    ("crates/medulla-viz-columnstore/src/store.rs", "eg-viz-columnstore.content_fingerprint.v1\\0", "medulla-viz-columnstore.content_fingerprint.v1\\0"),
    ("crates/medulla-viz-core/src/job.rs", "eg-viz-core.query_hash.v1\\0", "medulla-viz-core.query_hash.v1\\0"),
    ("crates/medulla-viz-core/src/job.rs", "eg-viz-core.spec_digest.v1\\0", "medulla-viz-core.spec_digest.v1\\0"),
    ("crates/medulla-wire-core/src/wire/mod.rs", "epistemic-graph/wire-signed-operation\\0", "medulla/wire-signed-operation\\0"),
    ("crates/medulla-work-store/src/redb_store/work_item_capability.rs", "epistemic-graph.work-item-claim-capability.v1\\0", "medulla.work-item-claim-capability.v1\\0"),
    ("medulla/client.py", "eg-envelope-v2", "medulla-envelope-v2"),
    ("medulla/client.py", "eg-multisig-mutation-v2", "medulla-multisig-mutation-v2"),
    ("medulla/client.py", "eg-register-identity-v2", "medulla-register-identity-v2"),
    ("medulla/client.py", "epistemic-graph/cluster-discovery/v1\\0", "medulla-engine/cluster-discovery/v1\\0"),
)


@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R003")
@pytest.mark.parametrize(("path", "original", "renamed"), PINNED_TAGS)
def test_identity_tag_keeps_original_bytes(path: str, original: str, renamed: str) -> None:
    text = (ROOT / path).read_text(encoding="utf-8")
    assert original in text, f"{path}: pinned identity tag {original!r} is gone"
    assert renamed not in text, (
        f"{path}: {renamed!r} reappeared; {original!r} is hashed/persisted and must not be renamed"
    )
