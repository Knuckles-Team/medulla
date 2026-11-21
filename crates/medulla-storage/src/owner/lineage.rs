//! Owner-store layout lineage (X11): the one registry of every earlier table
//! set each layout has had, and the one chokepoint that refuses them.
//!
//! An owner file's manifest pins the exact digest of its table contracts, so a
//! file written under an earlier layout fails the normal open. Without a
//! record of earlier layouts that failure is a generic digest mismatch,
//! indistinguishable from corruption or a foreign file. This module keeps that
//! record:
//!
//! * [`layout_predecessors`] names every refused predecessor of a layout. A
//!   predecessor file is refused with `{LAYOUT}_FORMAT_UPGRADE_REQUIRED` and its
//!   removal step. SQL, immediate pre-MCP Agent Library, pre-enrichment
//!   GraphShard, immediate pre-audit-request-index GraphShard and pre-current-plan
//!   GraphShard predecessors have explicit inspected offline upgrades; ordinary opens
//!   still refuse.
//! * [`validate_against_lineage`] is the check every manifest read runs --
//!   open, recovery adoption, classification and backup restore all read the
//!   manifest through `manifest_io::read_manifest` -- so none of them can miss
//!   it (EH-150: the named refusal sits at the manifest read, not the census).
//! * [`refuse_lineage_file`] runs the same check on a read-only open before any
//!   writable open, so a refused file's bytes are never touched.
//! * [`pinned_layout_digest`] pins each layout's current digest. Changing a
//!   layout without declaring its predecessor fails the lineage test, naming
//!   the layout; the only way through is to declare the format change.

use std::path::Path;

use crate::owner::layout::OwnerLayout;
use crate::owner::persisted_layout::LayoutPredecessor;
use crate::physical::incarnation::STORAGE_KERNEL_SCHEMA_VERSION;
use crate::physical::manifest::OwnerManifest;

/// `agent_library.redb` before connector packs and governed write-back.
pub const AGENT_LIBRARY_BEFORE_CONNECTOR_PACKS: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::AgentLibrary,
    label: "Agent Library before connector packs and governed write-back",
    owner_tables: &[
        "agent_library",
        "agent_library_heads",
        "agent_graph",
        "agent_graph_heads",
        "agent_component",
        "agent_component_heads",
        "agent_template",
        "agent_template_heads",
    ],
    data_lost: "its pre-ConnectorPack and governed-write-back Agent Library rows are \
                intentionally not upgraded",
    file_name: "agent_library.redb",
};

/// The previous Agent Library layout, before served MCP catalog authority rows.
pub const AGENT_LIBRARY_PRE_MCP_CATALOG_DIGEST: [u8; 32] = [
    0x7e, 0xa9, 0xbc, 0xde, 0x54, 0xe8, 0x96, 0x1c, 0xed, 0xaa, 0x58, 0x6b, 0x35, 0x20, 0xf8, 0x90,
    0x10, 0x0c, 0x5f, 0x53, 0xf9, 0xf5, 0xf9, 0x57, 0x4a, 0x30, 0xde, 0xa0, 0x2c, 0x88, 0x24, 0x45,
];
pub const AGENT_LIBRARY_BEFORE_MCP_CATALOG: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::AgentLibrary,
    label: "Agent Library before served MCP catalog authority",
    owner_tables: &[
        "agent_library",
        "agent_library_heads",
        "agent_graph",
        "agent_graph_heads",
        "agent_component",
        "agent_component_heads",
        "agent_template",
        "agent_template_heads",
        "connector_pack_heads",
        "connector_pack_members",
        "connector_pack_imports",
        "connector_pack_body_holders",
        "connector_pack_bindings",
        "decision_artifacts",
        "decision_records",
        "write_back_change_sets",
        "write_back_idempotency",
        "write_back_receipts",
        "write_back_receipt_heads",
    ],
    data_lost: "its rows require the explicit offline MCP catalog layout upgrade before normal open, or republishing after moving the file aside",
    file_name: "agent_library.redb",
};

/// The frozen layout digest of [`AGENT_LIBRARY_BEFORE_CONCEPT_LINEAGE`]: the
/// Agent Library pin before the `concept_ids` and `concept_lineage_edges` tables.
pub const AGENT_LIBRARY_PRE_CONCEPT_LINEAGE_DIGEST: [u8; 32] = [
    0xe1, 0x12, 0xd8, 0x11, 0x8e, 0x4e, 0x62, 0xb0, 0x02, 0x79, 0x9b, 0x84, 0xbb, 0xa8, 0xc6, 0x91,
    0x62, 0xe4, 0x77, 0x07, 0x18, 0x61, 0xa8, 0x1d, 0x38, 0x93, 0x27, 0x77, 0x36, 0x72, 0x7e, 0xa0,
];

/// `agent_library.redb` before the known concept ids and the concept lineage
/// edge store (MEDULLA-DURABLE-KERNEL-R094.7.1).
pub const AGENT_LIBRARY_BEFORE_CONCEPT_LINEAGE: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::AgentLibrary,
    label: "Agent Library before known concept ids and concept lineage edges",
    owner_tables: &[
        "agent_library",
        "agent_library_heads",
        "agent_graph",
        "agent_graph_heads",
        "agent_component",
        "agent_component_heads",
        "agent_template",
        "agent_template_heads",
        "connector_pack_heads",
        "connector_pack_members",
        "connector_pack_imports",
        "connector_pack_body_holders",
        "connector_pack_bindings",
        "mcp_catalog_configs",
        "mcp_catalog_scopes",
        "decision_artifacts",
        "decision_records",
        "write_back_change_sets",
        "write_back_idempotency",
        "write_back_receipts",
        "write_back_receipt_heads",
    ],
    data_lost: "none; an explicit offline layout upgrade is required. Normal open refuses this predecessor.",
    file_name: "agent_library.redb",
};

/// The frozen layout digest of [`AGENT_LIBRARY_BEFORE_CAPABILITY_REWARD`]: the
/// Agent Library pin before the `capability_reward` table: the layout with the
/// concept tables and no reward table.
pub const AGENT_LIBRARY_PRE_CAPABILITY_REWARD_DIGEST: [u8; 32] = [
    0x71, 0x0c, 0xe7, 0x43, 0xbc, 0x15, 0xde, 0x6a, 0x15, 0xe7, 0x65, 0x3e, 0x87, 0x3c, 0xa2, 0xbc,
    0xfd, 0xa3, 0x7d, 0x60, 0x02, 0x80, 0x70, 0x6b, 0x33, 0xc0, 0x0f, 0x8b, 0xdb, 0x90, 0x9e, 0x88,
];

/// An Agent Library owner file from before the learned capability reward rows
/// (MEDULLA-DECISION-ENGINE-R172.5.1): it already has the concept tables.
pub const AGENT_LIBRARY_BEFORE_CAPABILITY_REWARD: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::AgentLibrary,
    label: "Agent Library before learned capability rewards",
    owner_tables: &[
        "agent_library",
        "agent_library_heads",
        "agent_graph",
        "agent_graph_heads",
        "agent_component",
        "agent_component_heads",
        "agent_template",
        "agent_template_heads",
        "connector_pack_heads",
        "connector_pack_members",
        "connector_pack_imports",
        "connector_pack_body_holders",
        "connector_pack_bindings",
        "mcp_catalog_configs",
        "mcp_catalog_scopes",
        "concept_ids",
        "concept_lineage_edges",
        "decision_artifacts",
        "decision_records",
        "write_back_change_sets",
        "write_back_idempotency",
        "write_back_receipts",
        "write_back_receipt_heads",
    ],
    data_lost: "none; an explicit offline layout upgrade is required. Normal open refuses this predecessor.",
    file_name: "agent_library.redb",
};

/// The Agent Library table set immediately before the durable retry-allowance
/// table (MEDULLA-DECISION-ENGINE-R113.3.1.1).
const AGENT_LIBRARY_TABLES_BEFORE_RETRY_ALLOWANCES: &[&str] = &[
    "agent_library",
    "agent_library_heads",
    "agent_graph",
    "agent_graph_heads",
    "agent_component",
    "agent_component_heads",
    "agent_template",
    "agent_template_heads",
    "connector_pack_heads",
    "connector_pack_members",
    "connector_pack_imports",
    "connector_pack_body_holders",
    "connector_pack_bindings",
    "mcp_catalog_configs",
    "mcp_catalog_scopes",
    "concept_ids",
    "concept_lineage_edges",
    "decision_artifacts",
    "decision_records",
    "write_back_change_sets",
    "write_back_idempotency",
    "write_back_receipts",
    "write_back_receipt_heads",
    "capability_reward",
];

/// The frozen layout digest of `agent_library.redb` before the durable
/// retry-allowance table: the digest of the layout that has the concept tables
/// and the capability reward table and no retry-allowance table.
pub const AGENT_LIBRARY_PRE_RETRY_ALLOWANCES_DIGEST: [u8; 32] = [
    0xab, 0x99, 0xeb, 0x3e, 0x0a, 0xc4, 0xdc, 0xa9, 0xcc, 0x78, 0x2d, 0xab, 0xc8, 0xea, 0x9c, 0x92,
    0xf9, 0xf9, 0xe0, 0x43, 0x67, 0x8c, 0xf1, 0xa2, 0x5c, 0x8c, 0x8d, 0x34, 0xc6, 0x7c, 0x3f, 0x64,
];

/// `agent_library.redb` before the durable retry-allowance table.
pub const AGENT_LIBRARY_BEFORE_RETRY_ALLOWANCES: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::AgentLibrary,
    label: "Agent Library before durable retry allowances",
    owner_tables: AGENT_LIBRARY_TABLES_BEFORE_RETRY_ALLOWANCES,
    data_lost: "none; an explicit offline layout upgrade is required. Normal open refuses this predecessor.",
    file_name: "agent_library.redb",
};

/// `blob.redb` before holder rows, retention and the cursor high-water mark.
pub const BLOB_BEFORE_HOLDERS: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::Blob,
    label: "blob store from before holder-scoped references",
    owner_tables: &["cas_chunks", "cas_blobs", "cas_refcount", "cas_uploads"],
    data_lost: "its chunks, manifests, reference counts and uploads are not migrated, \
                so media blobs referenced by graphs must be uploaded again",
    file_name: "blob.redb",
};

/// The SQL owner tables of the layout before durable ANN generations, oldest
/// first. The layout before source checkpoints is exactly its first 18.
const SQL_TABLES_BEFORE_DURABLE_ANN: &[&str] = &[
    "__sql_catalog__",
    "__sql_functions__",
    "__sql_ann_indexes__",
    "__sql_secondary_indexes__",
    "__sql_secondary_index_entries__",
    "__sql_hypertables__",
    "__sql_source_authority__",
    "__sql_views__",
    "__sql_extensions__",
    "__sql_rows__",
    "__sql_seq__",
    "__sql_schema_catalog_versions__",
    "__sql_schema_versions__",
    "__sql_schema_migrations__",
    "__sql_schema_migration_order__",
    "__sql_schema_catalog_order__",
    "__sql_property_graphs__",
    "__sql_property_graph_seq__",
    "__sql_source_checkpoints__",
];

/// `sql.redb` before durable SQL source checkpoints. Normal open refuses it;
/// the explicit offline upgrader can preserve its catalog and rows.
pub const SQL_BEFORE_SOURCE_CHECKPOINTS: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::Sql,
    label: "SQL catalog store before durable source checkpoints",
    owner_tables: SQL_TABLES_BEFORE_DURABLE_ANN.split_at(18).0,
    data_lost: SQL_DATA_LOST,
    file_name: "sql.redb",
};

/// `sql.redb` before the maintained ANN authority's durable generations and
/// changed-row log, and the durable edge-index registrations (RF-019 /
/// EH-351 / EH-352).
pub const SQL_BEFORE_DURABLE_ANN: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::Sql,
    label: "SQL catalog store before durable ANN and edge index generations",
    owner_tables: SQL_TABLES_BEFORE_DURABLE_ANN,
    data_lost: SQL_DATA_LOST,
    file_name: "sql.redb",
};

/// The SQL owner tables of the layout before named sequences: everything the
/// current layout declares except `__sql_sequences__`.
const SQL_TABLES_BEFORE_NAMED_SEQUENCES: &[&str] = &[
    "__sql_catalog__",
    "__sql_functions__",
    "__sql_ann_indexes__",
    "__sql_secondary_indexes__",
    "__sql_secondary_index_entries__",
    "__sql_hypertables__",
    "__sql_source_authority__",
    "__sql_views__",
    "__sql_extensions__",
    "__sql_rows__",
    "__sql_seq__",
    "__sql_schema_catalog_versions__",
    "__sql_schema_versions__",
    "__sql_schema_migrations__",
    "__sql_schema_migration_order__",
    "__sql_schema_catalog_order__",
    "__sql_property_graphs__",
    "__sql_property_graph_seq__",
    "__sql_source_checkpoints__",
    "__sql_ann_generations__",
    "__sql_ann_dirty__",
    "__sql_edge_indexes__",
];

/// `sql.redb` before named sequences: a `SERIAL` column took its values from
/// the table's rowid counter, and a named sequence lived beside that counter
/// under reserved keys. The offline upgrade creates `<table>_<column>_seq` for
/// every serial column, seeds its high mark from the old counter and moves the
/// named sequences into `__sql_sequences__` (MEDULLA-ENGINE-PARITY-R013.2).
pub const SQL_BEFORE_NAMED_SEQUENCES: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::Sql,
    label: "SQL catalog store before named sequences",
    owner_tables: SQL_TABLES_BEFORE_NAMED_SEQUENCES,
    data_lost: SQL_DATA_LOST,
    file_name: "sql.redb",
};

/// The SQL owner tables of the layout before the trigger reaction ledger:
/// everything the current layout declares except `__sql_trigger_reactions__`.
const SQL_TABLES_BEFORE_TRIGGER_REACTIONS: &[&str] = &[
    "__sql_catalog__",
    "__sql_functions__",
    "__sql_ann_indexes__",
    "__sql_secondary_indexes__",
    "__sql_secondary_index_entries__",
    "__sql_hypertables__",
    "__sql_source_authority__",
    "__sql_views__",
    "__sql_extensions__",
    "__sql_rows__",
    "__sql_seq__",
    "__sql_schema_catalog_versions__",
    "__sql_schema_versions__",
    "__sql_schema_migrations__",
    "__sql_schema_migration_order__",
    "__sql_schema_catalog_order__",
    "__sql_property_graphs__",
    "__sql_property_graph_seq__",
    "__sql_source_checkpoints__",
    "__sql_ann_generations__",
    "__sql_ann_dirty__",
    "__sql_edge_indexes__",
    "__sql_sequences__",
];

/// `sql.redb` before the trigger reaction ledger. The offline upgrade creates
/// the empty ledger table (MEDULLA-ENGINE-PARITY-R012.2).
pub const SQL_BEFORE_TRIGGER_REACTIONS: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::Sql,
    label: "SQL catalog store before the trigger reaction ledger",
    owner_tables: SQL_TABLES_BEFORE_TRIGGER_REACTIONS,
    data_lost: SQL_DATA_LOST,
    file_name: "sql.redb",
};

/// The SQL owner tables of the layout before materialized views: everything the
/// current layout declares except `__sql_matviews__`.
const SQL_TABLES_BEFORE_MATVIEWS: &[&str] = &[
    "__sql_catalog__",
    "__sql_functions__",
    "__sql_ann_indexes__",
    "__sql_secondary_indexes__",
    "__sql_secondary_index_entries__",
    "__sql_hypertables__",
    "__sql_source_authority__",
    "__sql_views__",
    "__sql_extensions__",
    "__sql_rows__",
    "__sql_seq__",
    "__sql_schema_catalog_versions__",
    "__sql_schema_versions__",
    "__sql_schema_migrations__",
    "__sql_schema_migration_order__",
    "__sql_schema_catalog_order__",
    "__sql_property_graphs__",
    "__sql_property_graph_seq__",
    "__sql_source_checkpoints__",
    "__sql_ann_generations__",
    "__sql_ann_dirty__",
    "__sql_edge_indexes__",
    "__sql_sequences__",
    "__sql_trigger_reactions__",
];

/// `sql.redb` before the materialized-view catalog. The offline upgrade creates
/// the empty catalog table (MEDULLA-ENGINE-PARITY-R014.1).
pub const SQL_BEFORE_MATVIEWS: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::Sql,
    label: "SQL catalog store before the materialized-view catalog",
    owner_tables: SQL_TABLES_BEFORE_MATVIEWS,
    data_lost: SQL_DATA_LOST,
    file_name: "sql.redb",
};

const SQL_DATA_LOST: &str = "its SQL catalog and rows require an explicit offline upgrade \
                             before normal open, or re-ingestion after moving the file aside";

/// Frozen GraphShard census before the EH-557 enrichment budget authority.
/// The shared macro has an explicit insertion slot, so later current-layout
/// tables cannot silently enter this historical predecessor. The pinned
/// predecessor digest test catches edits to any common name or order.
const GRAPH_SHARD_TABLES_BEFORE_ENRICHMENT: &[&str] =
    super::graph_shard::graph_shard_table_names!();

/// The formerly pinned `95ef...` GraphShard layout had three enrichment
/// tables but lacked `repository_enrichment_policy_revisions`. Keep this
/// exact intermediate table set in lineage so an existing file is refused by
/// name instead of being misclassified as corrupt.
const GRAPH_SHARD_TABLES_BEFORE_POLICY_REVISIONS: &[&str] = super::graph_shard::graph_shard_table_names!(
    "repository_enrichment_budgets",
    "repository_enrichment_supersessions",
    "repository_enrichment_parks",
);

/// The formerly pinned `01c2...` GraphShard layout: all four enrichment
/// tables, before the operation audit-append idempotency index
/// `audit_requests`. Every graph shard written before that index existed has
/// exactly this table set.
const GRAPH_SHARD_TABLES_BEFORE_AUDIT_REQUESTS: &[&str] = super::graph_shard::graph_shard_table_names!(
    "repository_enrichment_budgets",
    "repository_enrichment_policy_revisions",
    "repository_enrichment_supersessions",
    "repository_enrichment_parks",
);

const GRAPH_SHARD_TABLES_BEFORE_CURRENT_PLANS: &[&str] = super::graph_shard::graph_shard_table_names!(
    "repository_enrichment_budgets",
    "repository_enrichment_policy_revisions",
    "repository_enrichment_supersessions",
    "repository_enrichment_parks",
    "audit_requests",
);
pub const GRAPH_SHARD_PRE_CURRENT_PLANS_DIGEST: [u8; 32] = [
    0x95, 0x2e, 0xa7, 0x29, 0xe0, 0x26, 0xde, 0xee, 0x49, 0xd9, 0x92, 0x2d, 0xd2, 0x57, 0x7c, 0xd1,
    0xf7, 0x89, 0x86, 0x0b, 0xe0, 0x6b, 0x19, 0xe9, 0x81, 0xcb, 0xb2, 0x94, 0x9c, 0xf2, 0xa2, 0x17,
];
pub const GRAPH_SHARD_BEFORE_CURRENT_PLANS: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::GraphShard,
    label: "graph shard before private current-plan snapshots",
    owner_tables: GRAPH_SHARD_TABLES_BEFORE_CURRENT_PLANS,
    data_lost: "none after the explicit offline current-plan layout upgrade. Normal open refuses this predecessor.",
    file_name: "graph-*.redb",
};

// Frozen census of the last layout before sealed run checkpoints.
const GRAPH_SHARD_TABLES_BEFORE_RUN_CHECKPOINTS: &[&str] = super::graph_shard::graph_shard_table_names!(
    "repository_enrichment_budgets",
    "repository_enrichment_policy_revisions",
    "repository_enrichment_supersessions",
    "repository_enrichment_parks",
    "audit_requests",
    "work_item_current_plans",
);
pub const GRAPH_SHARD_PRE_RUN_CHECKPOINTS_DIGEST: [u8; 32] = [
    0xcf, 0xc4, 0x7e, 0x43, 0x65, 0x4a, 0xbc, 0x32, 0xfb, 0xf6, 0x3c, 0xf3, 0xd9, 0xd7, 0xff, 0xd4,
    0xe0, 0x4c, 0x74, 0x2e, 0x3c, 0x95, 0x5c, 0x25, 0x9d, 0x2d, 0x05, 0xf5, 0x31, 0x6e, 0x0a, 0x27,
];
pub const GRAPH_SHARD_BEFORE_RUN_CHECKPOINTS: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::GraphShard,
    label: "graph shard before private run checkpoints",
    owner_tables: GRAPH_SHARD_TABLES_BEFORE_RUN_CHECKPOINTS,
    data_lost: "none; an explicit offline layout upgrade is required. Normal open refuses this predecessor.",
    file_name: "graph-*.redb",
};

// Frozen census of the last layout before work-item effect receipts and
// admitted requests (MEDULLA-DURABLE-KERNEL-R820.1).
const GRAPH_SHARD_TABLES_BEFORE_EFFECT_RECEIPTS: &[&str] = super::graph_shard::graph_shard_table_names!(
    "repository_enrichment_budgets",
    "repository_enrichment_policy_revisions",
    "repository_enrichment_supersessions",
    "repository_enrichment_parks",
    "audit_requests",
    "work_item_current_plans",
    "work_item_run_checkpoints",
);
/// The `3d32...` digest every graph shard carried before the effect tables.
pub const GRAPH_SHARD_PRE_EFFECT_RECEIPTS_DIGEST: [u8; 32] = [
    0x3d, 0x32, 0xd0, 0x52, 0xbb, 0xa3, 0x44, 0xa0, 0x0e, 0xcd, 0xb4, 0xbd, 0x0a, 0xed, 0xed, 0xaa,
    0xdf, 0x6d, 0x55, 0xe8, 0x6f, 0xe0, 0xd1, 0xe4, 0x35, 0x75, 0xe1, 0x70, 0x5c, 0x78, 0xfc, 0x51,
];
pub const GRAPH_SHARD_BEFORE_EFFECT_RECEIPTS: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::GraphShard,
    label: "graph shard before work-item effect receipts and admitted requests",
    owner_tables: GRAPH_SHARD_TABLES_BEFORE_EFFECT_RECEIPTS,
    data_lost: "none; run `store-upgrade` (the explicit offline layout upgrade). Normal open refuses this predecessor.",
    file_name: "graph-*.redb",
};

// Frozen census of the last layout before the sealed coordinator-decision
// certificate table (MEDULLA-DECISION-ENGINE-R036.105.1).
const GRAPH_SHARD_TABLES_BEFORE_DECISION_CERTIFICATES: &[&str] = super::graph_shard::graph_shard_table_names!(
    "repository_enrichment_budgets",
    "repository_enrichment_policy_revisions",
    "repository_enrichment_supersessions",
    "repository_enrichment_parks",
    "audit_requests",
    "work_item_current_plans",
    "work_item_run_checkpoints",
    "work_item_effect_receipts",
    "work_item_admitted_requests",
);
/// The `f836...` digest every graph shard carried before the certificate table.
pub const GRAPH_SHARD_PRE_DECISION_CERTIFICATES_DIGEST: [u8; 32] = [
    0xf8, 0x36, 0x39, 0x59, 0xe3, 0xfb, 0xa4, 0x82, 0xe9, 0x8b, 0xe4, 0xe5, 0xcd, 0xa9, 0x1b, 0x82,
    0xb2, 0x6e, 0x19, 0x58, 0xd2, 0x15, 0xbe, 0x02, 0x3a, 0x2a, 0xbe, 0xf6, 0x94, 0x4f, 0x71, 0xed,
];
pub const GRAPH_SHARD_BEFORE_DECISION_CERTIFICATES: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::GraphShard,
    label: "graph shard before sealed coordinator-decision certificates",
    owner_tables: GRAPH_SHARD_TABLES_BEFORE_DECISION_CERTIFICATES,
    data_lost: "none; run `store-upgrade` (the explicit offline layout upgrade). Normal open refuses this predecessor.",
    file_name: "graph-*.redb",
};

// Frozen census of the last layout before the reservation record lost its `tombstone` field
// (MEDULLA-PLACEMENT-R781.4): the same table names as the layout after it, with the reservation
// table at logical schema revision 1. Only the stored row shape differs, so the offline upgrade
// converts the rows of `resource_reservations`.
const GRAPH_SHARD_TABLES_BEFORE_RESERVATION_TOMBSTONE_REMOVAL: &[&str] = super::graph_shard::graph_shard_table_names!(
    "repository_enrichment_budgets",
    "repository_enrichment_policy_revisions",
    "repository_enrichment_supersessions",
    "repository_enrichment_parks",
    "audit_requests",
    "work_item_current_plans",
    "work_item_run_checkpoints",
    "work_item_effect_receipts",
    "work_item_admitted_requests",
    "xshard_decision_certificate",
);
/// The `c613...` digest every graph shard carried before the reservation tombstone removal: the
/// GraphShard pin of the parent commit.
pub const GRAPH_SHARD_PRE_RESERVATION_TOMBSTONE_REMOVAL_DIGEST: [u8; 32] = [
    0xc6, 0x13, 0xd4, 0x8a, 0xf8, 0x64, 0x9e, 0x05, 0xa3, 0xa0, 0x73, 0x7c, 0xe6, 0xab, 0xa8, 0xd4,
    0xed, 0xd5, 0x8c, 0x88, 0xc0, 0xca, 0x24, 0x4d, 0x05, 0xfc, 0x11, 0xdd, 0x9e, 0xdc, 0x52, 0xa5,
];
pub const GRAPH_SHARD_BEFORE_RESERVATION_TOMBSTONE_REMOVAL: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::GraphShard,
    label: "graph shard before the reservation tombstone removal",
    owner_tables: GRAPH_SHARD_TABLES_BEFORE_RESERVATION_TOMBSTONE_REMOVAL,
    data_lost: "none; run `store-upgrade` (the explicit offline layout upgrade). Normal open refuses this predecessor.",
    file_name: "graph-*.redb",
};

/// The logical schema revision of the graph-shard reservation table that `predecessor` froze:
/// 1 for every predecessor up to and including
/// [`GRAPH_SHARD_BEFORE_RESERVATION_TOMBSTONE_REMOVAL`], 2 for every predecessor declared after
/// it (the later predecessors take the then-current census). Every other layout is revision 1.
pub(crate) fn frozen_reservation_revision(predecessor: &LayoutPredecessor) -> u32 {
    let declared = layout_predecessors(OwnerLayout::GraphShard);
    let boundary = declared
        .iter()
        .position(|candidate| candidate == &GRAPH_SHARD_BEFORE_RESERVATION_TOMBSTONE_REMOVAL);
    let at = declared.iter().position(|candidate| candidate == predecessor);
    match (at, boundary) {
        (Some(at), Some(boundary)) if at > boundary => 2,
        _ => 1,
    }
}

// Frozen census of the last layout before the per-graph write ledger and creator tables
// (MEDULLA-IDENTITY-R700.7).
const GRAPH_SHARD_TABLES_BEFORE_WRITE_LEDGER: &[&str] = super::graph_shard::graph_shard_table_names!(
    "repository_enrichment_budgets",
    "repository_enrichment_policy_revisions",
    "repository_enrichment_supersessions",
    "repository_enrichment_parks",
    "audit_requests",
    "work_item_current_plans",
    "work_item_run_checkpoints",
    "work_item_effect_receipts",
    "work_item_admitted_requests",
    "xshard_decision_certificate",
);
/// The `c613...` digest every graph shard carried before the write ledger: the GraphShard pin of
/// the parent commit.
pub const GRAPH_SHARD_PRE_WRITE_LEDGER_DIGEST: [u8; 32] = [
    0xc6, 0x13, 0xd4, 0x8a, 0xf8, 0x64, 0x9e, 0x05, 0xa3, 0xa0, 0x73, 0x7c, 0xe6, 0xab, 0xa8, 0xd4,
    0xed, 0xd5, 0x8c, 0x88, 0xc0, 0xca, 0x24, 0x4d, 0x05, 0xfc, 0x11, 0xdd, 0x9e, 0xdc, 0x52, 0xa5,
];
/// A graph shard file from before the write ledger. The offline upgrade creates both tables
/// empty.
pub const GRAPH_SHARD_BEFORE_WRITE_LEDGER: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::GraphShard,
    label: "graph shard before the per-graph write ledger and creator tables",
    owner_tables: GRAPH_SHARD_TABLES_BEFORE_WRITE_LEDGER,
    data_lost: "none; run `store-upgrade` (the explicit offline layout upgrade). Normal open refuses this predecessor.",
    file_name: "graph-*.redb",
};

/// Earlier graph-shard layout before the node-payload scrub cursor (EH-384).
const GRAPH_SHARD_TABLES_BEFORE_STORAGE_SCRUB: &[&str] = GRAPH_SHARD_TABLES_BEFORE_ENRICHMENT
    .split_at(GRAPH_SHARD_TABLES_BEFORE_ENRICHMENT.len() - 1)
    .0;

pub const GRAPH_SHARD_PRE_ENRICHMENT_DIGEST: [u8; 32] = [
    0x34, 0x3e, 0xbe, 0x5b, 0xe2, 0x2f, 0xa5, 0x4d, 0xfe, 0x5e, 0xb8, 0x28, 0x4d, 0xd8, 0xbd, 0xee,
    0x37, 0x1a, 0xe5, 0x7b, 0x34, 0x6a, 0xde, 0x11, 0xa7, 0x6e, 0x4c, 0xd8, 0x3e, 0x0c, 0x84, 0x2d,
];

/// The layout digest every graph shard carried immediately before the
/// operation audit-append idempotency index. Frozen: it is what existing
/// files have on disk, so it is never recomputed from the current tables.
pub const GRAPH_SHARD_PRE_AUDIT_REQUESTS_DIGEST: [u8; 32] = [
    0x01, 0xc2, 0x19, 0xac, 0xd2, 0xbb, 0x2f, 0x1d, 0x52, 0x3a, 0x1b, 0xd8, 0x5e, 0xb0, 0x84, 0x42,
    0xcc, 0xa0, 0x3b, 0xfc, 0x97, 0x5d, 0x60, 0xde, 0x2e, 0xff, 0x2b, 0x91, 0xe3, 0x42, 0x57, 0x93,
];

pub const GRAPH_SHARD_BEFORE_ENRICHMENT: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::GraphShard,
    label: "graph shard before repository enrichment budgets and supersession",
    owner_tables: GRAPH_SHARD_TABLES_BEFORE_ENRICHMENT,
    data_lost: "its graph rows require the explicit offline enrichment layout upgrade before normal open, or re-ingestion after moving the file aside",
    file_name: "graph-*.redb",
};

pub const GRAPH_SHARD_BEFORE_POLICY_REVISIONS: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::GraphShard,
    label: "graph shard before repository enrichment policy revisions",
    owner_tables: GRAPH_SHARD_TABLES_BEFORE_POLICY_REVISIONS,
    data_lost: "its graph rows require re-ingestion after moving the file aside; the offline enrichment upgrade only accepts the exact pre-enrichment layout",
    file_name: "graph-*.redb",
};

pub const GRAPH_SHARD_BEFORE_AUDIT_REQUESTS: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::GraphShard,
    label: "graph shard before the operation audit-append idempotency index",
    owner_tables: GRAPH_SHARD_TABLES_BEFORE_AUDIT_REQUESTS,
    data_lost: "its graph rows require the explicit offline audit-request-index layout upgrade before normal open, or re-ingestion after moving the file aside",
    file_name: "graph-*.redb",
};

/// A graph shard file from before the background node-payload scrub (EH-384;
/// operator ruling: existing graph data is disposable).
pub const GRAPH_SHARD_BEFORE_STORAGE_SCRUB: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::GraphShard,
    label: "graph shard store before the background node-payload scrub",
    owner_tables: GRAPH_SHARD_TABLES_BEFORE_STORAGE_SCRUB,
    data_lost: "its graphs are not migrated; re-ingest the sources",
    file_name: "graph-*.redb",
};

/// The SemanticIndex owner tables before the promotion-gate records
/// (MEDULLA-UNIFIED-DATA-PLANE-R040.3), in registry order.
const SEMANTIC_INDEX_TABLES_BEFORE_PROMOTION: &[&str] = &[
    "semantic_bindings",
    "semantic_binding_heads",
    "semantic_stage_transitions",
    "semantic_binding_state_transitions",
    "semantic_source_progress",
    "semantic_active_pointers",
    "semantic_dead_letters",
    "semantic_tombstones",
    "semantic_sql_source_manifests",
    "semantic_graph_projection_manifests",
    "semantic_authorization_receipts",
    "semantic_generation_checkpoints",
    "semantic_generation_checkpoint_heads",
    "semantic_lexical_manifests",
    "semantic_ann_manifests",
    "semantic_vectors",
    "eg_ann",
];

/// The frozen layout digest of [`SEMANTIC_INDEX_BEFORE_PROMOTION`]: the
/// SemanticIndex pin before the promotion-gate tables.
pub const SEMANTIC_INDEX_PRE_PROMOTION_DIGEST: [u8; 32] = [
    0x30, 0x08, 0xd8, 0x72, 0x6d, 0x15, 0xa8, 0x64, 0xda, 0xf6, 0xc7, 0x23, 0x00, 0x41, 0x52, 0xfc,
    0x61, 0xbb, 0x40, 0xbc, 0x8d, 0xe0, 0xfe, 0xa4, 0x86, 0xdf, 0xda, 0xac, 0xe0, 0xc9, 0x85, 0xd8,
];

/// A semantic index owner file from before the promotion-gate records.
pub const SEMANTIC_INDEX_BEFORE_PROMOTION: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::SemanticIndex,
    label: "semantic index before promotion-gate records",
    owner_tables: SEMANTIC_INDEX_TABLES_BEFORE_PROMOTION,
    data_lost: "none; an explicit offline layout upgrade is required. Normal open refuses this predecessor.",
    file_name: "semantic_*.redb",
};

/// The Rbac owner tables before the profile-use grant records
/// (MEDULLA-DECISION-ENGINE-R036.106), in registry order.
const RBAC_TABLES_BEFORE_PROFILE_GRANTS: &[&str] = &["rbac"];

/// The frozen layout digest of [`RBAC_BEFORE_PROFILE_GRANTS`]: the Rbac pin
/// before the `rbac_profile_use_grant` table.
pub const RBAC_PRE_PROFILE_GRANTS_DIGEST: [u8; 32] = [
    0x3c, 0x89, 0xb3, 0x20, 0x7f, 0xaa, 0x96, 0x2a, 0x94, 0xeb, 0x1c, 0xe7, 0x24, 0xf9, 0x44, 0xdd,
    0xe5, 0xa4, 0xac, 0xf9, 0x48, 0x41, 0xa2, 0x3d, 0x9c, 0x41, 0x68, 0x55, 0x86, 0x6d, 0x4c, 0x8e,
];

/// An Rbac owner file from before the profile-use grant records.
pub const RBAC_BEFORE_PROFILE_GRANTS: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::Rbac,
    label: "rbac before profile-use grant records",
    owner_tables: RBAC_TABLES_BEFORE_PROFILE_GRANTS,
    data_lost: "none; an explicit offline layout upgrade is required. Normal open refuses this predecessor.",
    file_name: "rbac.redb",
};

/// The Rbac owner tables before the data product, contract and access request records
/// (MEDULLA-TYPED-PACKS-R805.3), in registry order.
const RBAC_TABLES_BEFORE_DATA_PRODUCTS: &[&str] = &["rbac", "rbac_profile_use_grant"];

/// The frozen layout digest of [`RBAC_BEFORE_DATA_PRODUCTS`]: the Rbac pin before the
/// `rbac_data_product` table.
pub const RBAC_PRE_DATA_PRODUCTS_DIGEST: [u8; 32] = [
    0xc3, 0x0f, 0x57, 0xc1, 0x12, 0x29, 0xb6, 0x29, 0xfe, 0xfb, 0x44, 0x70, 0xbf, 0x9b, 0x7a, 0xfb,
    0x50, 0x10, 0xc0, 0x6c, 0x24, 0x54, 0x2d, 0x25, 0xcd, 0x20, 0xea, 0x61, 0x3d, 0x72, 0x9d, 0xbd,
];

/// An Rbac owner file from before the data product records.
pub const RBAC_BEFORE_DATA_PRODUCTS: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::Rbac,
    label: "rbac before data product records",
    owner_tables: RBAC_TABLES_BEFORE_DATA_PRODUCTS,
    data_lost: "none; an explicit offline layout upgrade is required. Normal open refuses this predecessor.",
    file_name: "rbac.redb",
};

/// The Kv owner tables before the KV-cache cold rung moved into a namespace of the `kv`
/// table (MEDULLA-DURABLE-KERNEL-R083.5), in registry order.
const KV_TABLES_BEFORE_NAMESPACE_COLD: &[&str] = &["kv", "eg_kvcache_cold"];

/// The frozen layout digest of [`KV_BEFORE_NAMESPACE_COLD`]: the Kv pin while the cache's
/// cold rung had its own `eg_kvcache_cold` table.
pub const KV_PRE_NAMESPACE_COLD_DIGEST: [u8; 32] = [
    0x48, 0xb6, 0x46, 0x4a, 0xd5, 0x96, 0x0b, 0xd1, 0x4f, 0xc0, 0x97, 0x61, 0x19, 0xb6, 0x89, 0xc0,
    0x01, 0x6b, 0xe6, 0x3e, 0x8c, 0xc8, 0xbe, 0x72, 0xdf, 0x4e, 0x70, 0x30, 0xed, 0x59, 0x3f, 0x9e,
];

/// A Kv owner file from before the cache's cold rung became a `kv` namespace. The upgrade
/// deletes the `eg_kvcache_cold` table.
pub const KV_BEFORE_NAMESPACE_COLD: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::Kv,
    label: "kv store before the KV-cache cold rung moved into a kv namespace",
    owner_tables: KV_TABLES_BEFORE_NAMESPACE_COLD,
    data_lost: "the rows of the retired eg_kvcache_cold table, which only held cache bytes the cache rebuilds on a miss; an explicit offline layout upgrade is required. Normal open refuses this predecessor.",
    file_name: "kv.redb",
};

/// The Jobs owner tables before the analytics-job machine (MEDULLA-CQ-ORCH-R811.4): the frozen
/// 13-table census, whose record table is `analytics_jobs`.
const JOBS_TABLES_BEFORE_JOB_MACHINE: &[&str] = &[
    "analytics_jobs",
    "analytics_job_committed_results",
    "job_intents",
    "job_idempotency_ledger",
    "analytics_job_knowledge_batches",
    "analytics_job_scheduler_meta",
    "analytics_job_ready_by_priority",
    "analytics_job_ready_by_capability",
    "analytics_job_lease_by_worker",
    "analytics_job_lease_by_expiry",
    "analytics_job_active_totals_by_tenant",
    "analytics_job_by_deadline",
    "analytics_job_cancellation_reconcile",
];

/// The frozen layout digest of [`JOBS_BEFORE_JOB_MACHINE`]: the Jobs pin while the job record
/// table was `analytics_jobs` and a record carried `cancel_requested`, `lease` and
/// `not_before_ms` beside a six-variant state.
pub const JOBS_PRE_JOB_MACHINE_DIGEST: [u8; 32] = [
    0x64, 0xeb, 0x7f, 0xfd, 0x32, 0x79, 0x80, 0xa8, 0x26, 0x9b, 0xc8, 0xcf, 0x61, 0x14, 0x7b, 0xc2,
    0x69, 0xb4, 0xee, 0x8e, 0xab, 0x1a, 0xe0, 0xf5, 0xde, 0x3a, 0x24, 0x4b, 0xbe, 0xc9, 0xda, 0xd4,
];

/// A Jobs owner file from before the analytics-job machine. The upgrade converts every record
/// of `analytics_jobs` to the machine shape and stores it in `analytics_job_records`.
pub const JOBS_BEFORE_JOB_MACHINE: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::Jobs,
    label: "jobs before the analytics job machine",
    owner_tables: JOBS_TABLES_BEFORE_JOB_MACHINE,
    data_lost: "none; an explicit offline layout upgrade is required. Normal open refuses this predecessor.",
    file_name: "jobs.redb",
};

/// The Jobs owner tables before the ingest task queue totals (MEDULLA-INGEST-WORKER-R001.4.1):
/// the 13-table census whose record table is `analytics_job_records`.
const JOBS_TABLES_BEFORE_TASK_QUEUE: &[&str] = &[
    "analytics_job_records",
    "analytics_job_committed_results",
    "job_intents",
    "job_idempotency_ledger",
    "analytics_job_knowledge_batches",
    "analytics_job_scheduler_meta",
    "analytics_job_ready_by_priority",
    "analytics_job_ready_by_capability",
    "analytics_job_lease_by_worker",
    "analytics_job_lease_by_expiry",
    "analytics_job_active_totals_by_tenant",
    "analytics_job_by_deadline",
    "analytics_job_cancellation_reconcile",
];

/// The frozen layout digest of [`JOBS_BEFORE_TASK_QUEUE`]: the Jobs layout with the
/// `analytics_job_records` record table and without `analytics_job_task_queue_totals` and
/// `analytics_job_task_queue_oldest`.
pub const JOBS_PRE_TASK_QUEUE_DIGEST: [u8; 32] = [
    0x48, 0x4d, 0x6f, 0xc8, 0x2b, 0xf7, 0xaa, 0x8f, 0x3b, 0xab, 0x2e, 0x74, 0x71, 0x3e, 0xdc, 0x72,
    0x03, 0x1b, 0xa8, 0x5e, 0x5d, 0x22, 0x19, 0xdc, 0xee, 0x67, 0x19, 0xd2, 0x96, 0x5d, 0x40, 0xd8,
];

/// A Jobs owner file from before the per-queue task totals and oldest-enqueue index. The
/// upgrade creates both tables empty; a task enqueued before them does not exist, because the
/// enqueue path refused without them.
pub const JOBS_BEFORE_TASK_QUEUE: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::Jobs,
    label: "jobs before the ingest task queue totals",
    owner_tables: JOBS_TABLES_BEFORE_TASK_QUEUE,
    data_lost: "none; an explicit offline layout upgrade is required. Normal open refuses this predecessor.",
    file_name: "jobs.redb",
};

/// The Jobs owner tables before the ingest dead-letter table (MEDULLA-INGEST-WORKER-R001.9):
/// the 15-table census with the task-queue totals and oldest-enqueue tables.
const JOBS_TABLES_BEFORE_TASK_DEAD_LETTERS: &[&str] = &[
    "analytics_job_records",
    "analytics_job_committed_results",
    "job_intents",
    "job_idempotency_ledger",
    "analytics_job_knowledge_batches",
    "analytics_job_scheduler_meta",
    "analytics_job_ready_by_priority",
    "analytics_job_ready_by_capability",
    "analytics_job_lease_by_worker",
    "analytics_job_lease_by_expiry",
    "analytics_job_active_totals_by_tenant",
    "analytics_job_by_deadline",
    "analytics_job_cancellation_reconcile",
    "analytics_job_task_queue_totals",
    "analytics_job_task_queue_oldest",
];

/// The frozen layout digest of [`JOBS_BEFORE_TASK_DEAD_LETTERS`]: the Jobs layout with the
/// task-queue tables and without `analytics_job_task_dead_letters`.
pub const JOBS_PRE_TASK_DEAD_LETTERS_DIGEST: [u8; 32] = [
    0xc4, 0x9b, 0x1c, 0x73, 0xe2, 0x1a, 0xb9, 0x0c, 0x50, 0x91, 0x53, 0xa0, 0x65, 0x31, 0xf4, 0x7c,
    0x29, 0x8b, 0x04, 0xbb, 0xc4, 0xc5, 0x1e, 0xf4, 0x45, 0xdb, 0x88, 0xbc, 0xcd, 0xf5, 0x33, 0xce,
];

/// A Jobs owner file from before the ingest dead-letter table. The upgrade creates it empty.
pub const JOBS_BEFORE_TASK_DEAD_LETTERS: LayoutPredecessor = LayoutPredecessor {
    layout: OwnerLayout::Jobs,
    label: "jobs before the ingest dead-letter table",
    owner_tables: JOBS_TABLES_BEFORE_TASK_DEAD_LETTERS,
    data_lost: "none; an explicit offline layout upgrade is required. Normal open refuses this predecessor.",
    file_name: "jobs.redb",
};

/// Every predecessor of `layout` this build refuses by name, oldest first.
///
/// Exhaustive on purpose: a new layout must state that it has none.
pub fn layout_predecessors(layout: OwnerLayout) -> &'static [LayoutPredecessor] {
    match layout {
        OwnerLayout::AgentLibrary => &[
            AGENT_LIBRARY_BEFORE_CONNECTOR_PACKS,
            AGENT_LIBRARY_BEFORE_MCP_CATALOG,
            AGENT_LIBRARY_BEFORE_CONCEPT_LINEAGE,
            AGENT_LIBRARY_BEFORE_CAPABILITY_REWARD,
            AGENT_LIBRARY_BEFORE_RETRY_ALLOWANCES,
        ],
        OwnerLayout::Blob => &[BLOB_BEFORE_HOLDERS],
        OwnerLayout::SemanticIndex => &[SEMANTIC_INDEX_BEFORE_PROMOTION],
        OwnerLayout::Sql => &[
            SQL_BEFORE_SOURCE_CHECKPOINTS,
            SQL_BEFORE_DURABLE_ANN,
            SQL_BEFORE_NAMED_SEQUENCES,
            SQL_BEFORE_TRIGGER_REACTIONS,
            SQL_BEFORE_MATVIEWS,
        ],
        OwnerLayout::Rbac => &[RBAC_BEFORE_PROFILE_GRANTS, RBAC_BEFORE_DATA_PRODUCTS],
        OwnerLayout::Kv => &[KV_BEFORE_NAMESPACE_COLD],
        OwnerLayout::Jobs => &[
            JOBS_BEFORE_JOB_MACHINE,
            JOBS_BEFORE_TASK_QUEUE,
            JOBS_BEFORE_TASK_DEAD_LETTERS,
        ],
        OwnerLayout::LedgerOnly
        | OwnerLayout::Statechart
        | OwnerLayout::TimeSeries
        | OwnerLayout::PathIndex
        | OwnerLayout::RequestReplay
        | OwnerLayout::VizProvenance
        | OwnerLayout::ColdTier
        | OwnerLayout::TenantCatalog
        | OwnerLayout::NodeInfo
        | OwnerLayout::ClusterHierarchy
        | OwnerLayout::BranchRegistry
        | OwnerLayout::RetrievalState
        | OwnerLayout::SchedulerConfig
        | OwnerLayout::GraphSourceBindings => &[],
        OwnerLayout::GraphShard => &[
            GRAPH_SHARD_BEFORE_STORAGE_SCRUB,
            GRAPH_SHARD_BEFORE_ENRICHMENT,
            GRAPH_SHARD_BEFORE_POLICY_REVISIONS,
            GRAPH_SHARD_BEFORE_AUDIT_REQUESTS,
            GRAPH_SHARD_BEFORE_CURRENT_PLANS,
            GRAPH_SHARD_BEFORE_RUN_CHECKPOINTS,
            GRAPH_SHARD_BEFORE_EFFECT_RECEIPTS,
            GRAPH_SHARD_BEFORE_DECISION_CERTIFICATES,
            GRAPH_SHARD_BEFORE_RESERVATION_TOMBSTONE_REMOVAL,
            GRAPH_SHARD_BEFORE_WRITE_LEDGER,
        ],
    }
}

/// Every layout, in registry order.
pub const ALL_LAYOUTS: [OwnerLayout; 22] = [
    OwnerLayout::LedgerOnly,
    OwnerLayout::Rbac,
    OwnerLayout::Jobs,
    OwnerLayout::Statechart,
    OwnerLayout::TimeSeries,
    OwnerLayout::Kv,
    OwnerLayout::Blob,
    OwnerLayout::SemanticIndex,
    OwnerLayout::Sql,
    OwnerLayout::PathIndex,
    OwnerLayout::RequestReplay,
    OwnerLayout::VizProvenance,
    OwnerLayout::ColdTier,
    OwnerLayout::TenantCatalog,
    OwnerLayout::NodeInfo,
    OwnerLayout::ClusterHierarchy,
    OwnerLayout::GraphShard,
    OwnerLayout::AgentLibrary,
    OwnerLayout::BranchRegistry,
    OwnerLayout::RetrievalState,
    OwnerLayout::SchedulerConfig,
    OwnerLayout::GraphSourceBindings,
];

/// The digest each layout had when its lineage was last declared, hex.
///
/// A closed contract pin, not a baseline: when a layout changes, this pin and
/// that layout's [`layout_predecessors`] change together, in the commit that
/// changes the layout.
pub fn pinned_layout_digest(layout: OwnerLayout) -> &'static str {
    match layout {
        OwnerLayout::LedgerOnly => {
            "9faad03bf787a2a893618b7a8b386de30bdd5b4e09f3930448040c3e47c59e3e"
        }
        OwnerLayout::Rbac => "c30f57c11229b629fefb4470bf9b7afb5010c06c24542d25cd20ea613d729dbd",
        OwnerLayout::Jobs => "313e863a75ffa34576a66f5e898d0c60970ea9662b59ff9f9b879c019083a077",
        OwnerLayout::Statechart => {
            "d8ab661d50af9249a342a101537f0ef92c13df6808b5fd149c037d8587cff8a7"
        }
        OwnerLayout::TimeSeries => {
            "5f2b379c6ea02f36f38bcfd2fd8f6277e5174467f3c81b309fffdf87ec1a1581"
        }
        // LAYOUT-REGEN needed: the compiled Kv digest changed with the retired table; the gate
        // fixer runs the digest-printing test and replaces this pin.
        OwnerLayout::Kv => "48b6464ad5960bd14fc0976119b689c0016be63e8cc8be72df4e7030ed593f9e",
        OwnerLayout::Blob => "982e463fd13db3ec48fbdda77249ecaefed3b9174276c61ac7cd2eb206c47d6b",
        OwnerLayout::SemanticIndex => {
            "b2cdb28a67aab60a08142133a035765520c6ba94a1a2921aafca0ed9bc000bac"
        }
        OwnerLayout::Sql => "21dc6828e29152b788d8ddd44d80cc4dc1964f1c810294b46417dcf6bee5c31c",
        OwnerLayout::PathIndex => {
            "c79d89a41ad97d55cf35238ca8706db8c958923624de068a430c3bca72656df9"
        }
        OwnerLayout::RequestReplay => {
            "ccb7051fef1a624274939944f7af9ee0f3ee03eee062794eeaf105d1c6117809"
        }
        OwnerLayout::VizProvenance => {
            "57e566cd7fae6e67c83778cb50423130b8c250c2ce58dd3e93f5b203151661d3"
        }
        OwnerLayout::ColdTier => "9170bbb3960d0eddb71c4b080878abcb4440698472124911e78125aa6f3931ad",
        OwnerLayout::TenantCatalog => {
            "01b945d16fa187e69ca3dd5777a27066310b64a8b351380af1ba810f5dcd57aa"
        }
        OwnerLayout::NodeInfo => "02e5ca262822f4de0a1507780b2e908e605655d3a758e78c2a75f205ffa80347",
        OwnerLayout::ClusterHierarchy => {
            "0561a2a2ae13f067bf01a4c94d6cbaeb280aaefa56c68036a1f01da92cba8431"
        }
        // LAYOUT-REGEN needed: the compiled GraphShard digest changed with `write_days` and
        // `graph_creator` (MEDULLA-IDENTITY-R700.7); the gate fixer runs the digest-printing test
        // and replaces this pin. The previous pin lives on as
        // `GRAPH_SHARD_PRE_WRITE_LEDGER_DIGEST`.
        OwnerLayout::GraphShard => {
            "c613d48af8649e05a3a0737ce6aba8d4edd58c88c0ca244d05fc11dd9edc52a5"
        }
        OwnerLayout::AgentLibrary => {
            "2c6c76fd721bf9b0115dc2f2fc51245d6449027069238b63b66bb36d8bc8a569"
        }
        OwnerLayout::BranchRegistry => {
            "f8b3f0bc54cc11357787200c0091ebf8ccd7ae67af7fbea1930837e1d24019b4"
        }
        OwnerLayout::RetrievalState => {
            "3002bba378ed03980325c554c27a2f2dd410e9367372226db9173cf154d69575"
        }
        // Not yet measured: the layout-digest test prints the compiled digest;
        // replace this pin with it in the same commit that lands the layout.
        OwnerLayout::SchedulerConfig => {
            "0000000000000000000000000000000000000000000000000000000000000000"
        }
        // Not yet measured: the layout-digest test prints the compiled digest;
        // the coordinator replaces this pin with it.
        OwnerLayout::GraphSourceBindings => {
            "0000000000000000000000000000000000000000000000000000000000000000"
        }
    }
}

/// `layout`'s compiled digest, lowercase hex, as [`pinned_layout_digest`]
/// spells it.
pub fn layout_digest_hex(layout: OwnerLayout) -> String {
    layout
        .digest()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The lineage chokepoint: accept a current manifest, refuse a declared
/// predecessor by name, and report any other digest as unknown -- possibly
/// corrupt or written by another build -- rather than as a predecessor.
pub(crate) fn validate_against_lineage(manifest: &OwnerManifest) -> Result<(), String> {
    let Err(error) = manifest.validate_current() else {
        return Ok(());
    };
    if let Some(predecessor) = layout_predecessors(manifest.layout)
        .iter()
        .find(|predecessor| predecessor.matches(manifest))
    {
        return Err(predecessor.refusal(None));
    }
    if manifest.schema_version == STORAGE_KERNEL_SCHEMA_VERSION
        && manifest.layout_digest != manifest.layout.digest()
    {
        return Err(format!(
            "OWNER_STORE_FORMAT_UNKNOWN: the {} store's layout digest is not in this build's \
             lineage; the file may be corrupt or written by another build ({error})",
            manifest.layout.canonical_name()
        ));
    }
    Err(error)
}

/// Run the lineage check on a read-only open of `path` before anything opens
/// it for write. A file that cannot be inspected read-only is left to the
/// normal open, which is the authority on what is wrong with it.
pub(crate) fn refuse_lineage_file(path: &Path, layout: OwnerLayout) -> Result<(), String> {
    for predecessor in layout_predecessors(layout) {
        crate::owner::persisted_layout::refuse_known_predecessor(path, predecessor)?;
    }
    Ok(())
}

/// The operator documentation of every layout's format history, rendered from
/// this registry on demand and never committed (MEDULLA-SDLC-R020.1):
/// `cargo run -p medulla-storage --example gen_owner_store_formats` prints it.
pub fn render_owner_store_formats() -> String {
    let mut out = String::from(
        "# Owner-store formats\n\n\
         Rendered from `crates/medulla-storage/src/owner/lineage.rs` by \
         `cargo run -p medulla-storage --example gen_owner_store_formats`.\n\n\
         Every durable owner file records the digest of its exact table layout. A file whose \
         layout is a declared predecessor below is refused on ordinary open with the named \
         error. SQL and immediate pre-MCP Agent Library predecessors have explicit offline, data-preserving upgrades. \
         GraphShard also supports these upgrades for pre-enrichment, immediate pre-audit-request-index and pre-current-plan predecessors. \
         Other predecessors require moving files aside and recreating them. Any other digest is refused as `OWNER_STORE_FORMAT_UNKNOWN`.\n\n\
         | Store | Current layout digest | Refused predecessors |\n|---|---|---|\n",
    );
    for layout in ALL_LAYOUTS {
        let predecessors = layout_predecessors(layout);
        let refused = if predecessors.is_empty() {
            "none".to_string()
        } else {
            let codes: std::collections::BTreeSet<String> = predecessors
                .iter()
                .map(|predecessor| format!("`{}`", predecessor.error_code()))
                .collect();
            codes.into_iter().collect::<Vec<_>>().join(", ")
        };
        out.push_str(&format!(
            "| `{}` | `{}` | {} |\n",
            layout.canonical_name(),
            layout_digest_hex(layout),
            refused
        ));
    }
    for layout in ALL_LAYOUTS {
        let predecessors = layout_predecessors(layout);
        for (index, predecessor) in predecessors.iter().enumerate() {
            let previous = index.checked_sub(1).map(|index| &predecessors[index]);
            out.push_str(&format!(
                "\n## `{code}`: {label}\n\n- Store file: `{file}`\n- Refused generation: {label}\n\
                 - Data lost: {lost}\n- Tables: {tables}\n\
                 - Removal step: {removal}\n",
                code = predecessor.error_code(),
                file = predecessor.file_name,
                label = predecessor.label,
                lost = render_predecessor_prose(predecessor.data_lost),
                tables = render_predecessor_tables(predecessor, previous),
                removal = render_predecessor_prose(
                    &crate::owner::offline_upgrade::render_removal_step(predecessor),
                ),
            ));
        }
    }
    out.push_str(&crate::owner::offline_upgrade::render_offline_upgrade_procedure());
    out
}

/// Separate independent operator clauses without changing persisted lineage data.
fn render_predecessor_prose(text: &str) -> String {
    text.replace("; ", ". ")
        .replace(", or ", ". Alternatively, ")
        .replace(", so ", ". Thus, ")
}

/// Describe inherited table sets once; additions do not imply manifest ordering.
fn render_predecessor_tables(
    predecessor: &LayoutPredecessor,
    previous: Option<&LayoutPredecessor>,
) -> String {
    let shared = previous.filter(|previous| {
        previous.layout == predecessor.layout
            && previous
                .owner_tables
                .iter()
                .all(|table| predecessor.owner_tables.contains(table))
    });
    let tables = predecessor
        .owner_tables
        .iter()
        .filter(|table| shared.is_none_or(|previous| !previous.owner_tables.contains(table)))
        .map(|table| format!("`{table}`"))
        .collect::<Vec<_>>()
        .join(", ");
    match shared {
        Some(previous) if tables.is_empty() => {
            format!("Same table set as **{}** above.", previous.label)
        }
        Some(previous) => format!(
            "Same table set as **{}** above. Added tables: {tables}.",
            previous.label
        ),
        None => tables,
    }
}

#[cfg(test)]
#[path = "lineage_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "lineage_fixture_tests.rs"]
mod fixture_tests;
