//! The one durable write of an inspected Agent Library, GraphShard, SemanticIndex, Rbac or
//! Kv layout.

use super::{
    successor, validate_census, validate_descriptor, validate_predecessor, AgentLibrarySource,
    GraphShardSource, JobsSource, UpgradeTarget, ValidatedOwnerLayoutUpgrade,
};
use crate::owner::graph_shard::{
    ADMITTED_REQUESTS, AUDIT_REQUESTS, CURRENT_PLANS, EFFECT_RECEIPTS,
    REPOSITORY_ENRICHMENT_BUDGETS, REPOSITORY_ENRICHMENT_PARKS,
    REPOSITORY_ENRICHMENT_POLICY_REVISIONS, REPOSITORY_ENRICHMENT_SUPERSESSIONS, RUN_CHECKPOINTS,
    GRAPH_CREATOR, WRITE_DAYS, XSHARD_DECISION_CERTIFICATE,
};
use crate::owner::registry::{
    predecessor_evidence, CAPABILITY_REWARD, CONCEPT_IDS, CONCEPT_LINEAGE_EDGES,
    DECISION_RETRY_ALLOWANCES, KV_COLD, MCP_CATALOG_CONFIGS, MCP_CATALOG_SCOPES,
    RBAC_DATA_PRODUCT, RBAC_PROFILE_USE_GRANT, SEMANTIC_EMBEDDING_VERSION_TAGS, SEMANTIC_MODALITY_CHECKS,
    SEMANTIC_PROMOTION_VERDICTS, SEMANTIC_STAGE_RECEIPTS, TASK_DEAD_LETTERS, TASK_QUEUE_OLDEST,
    TASK_QUEUE_TOTALS,
};
use crate::owner::sql_checkpoint_upgrade::{
    begin_immediate_upgrade_write, read_upgrade_write_manifest, stage_upgrade_manifest,
};
use crate::owner::{validate_declared_tables_write, validate_manifest_write};
use crate::physical::integrity::StoredRowCipher;
use crate::physical::manifest::OwnerManifest;
use crate::physical::root::{store_handle, validate_handle_write};
use crate::recovery::evidence::HashSnapshot;
use redb::{Database, WriteTransaction};
use std::fs::File;

pub(super) fn commit_upgrade(
    database: &Database,
    token: &ValidatedOwnerLayoutUpgrade,
    file: &File,
    target: UpgradeTarget,
) -> Result<OwnerManifest, String> {
    let write = begin_immediate_upgrade_write(database)?;
    validate_descriptor(token, file)?;
    validate_handle_write(&store_handle(token.source.incarnation.clone()), &write)?;
    validate_census(
        &token.source.manifest,
        target,
        write.list_tables().map_err(|error| error.to_string())?,
        write
            .list_multimap_tables()
            .map_err(|error| error.to_string())?,
    )?;
    let old = read_upgrade_write_manifest(&write)?;
    validate_predecessor(&old, target)?;
    if old != token.source.manifest
        || predecessor_evidence(
            HashSnapshot::Write(&write),
            target.layout(),
            target.predecessor().owner_tables,
        )? != token.source.evidence
    {
        return Err("owner predecessor changed before atomic upgrade".into());
    }
    let current = successor(&old, target)?;
    retire_predecessor_tables(&write, target, token.source.row_cipher.as_deref())?;
    open_successor_tables(&write, target)?;
    stage_upgrade_manifest(&write, &current, "upgraded owner manifest")?;
    validate_manifest_write(&write, &current.physical_identity, target.layout())?;
    validate_declared_tables_write(&write, target.layout())?;
    validate_descriptor(token, file)?;
    write.commit().map_err(|error| error.to_string())?;
    Ok(current)
}

/// Delete, with its rows, every table the predecessor had and the current layout retired.
fn retire_predecessor_tables(
    write: &WriteTransaction,
    target: UpgradeTarget,
    row_cipher: Option<&dyn StoredRowCipher>,
) -> Result<(), String> {
    if target == UpgradeTarget::Kv {
        write
            .delete_table(KV_COLD)
            .map_err(|error| error.to_string())?;
    }
    // The jobs successor stores every record in its machine shape under the new table name, so
    // the retired table is converted and then deleted (MEDULLA-CQ-ORCH-R811.4).
    if target == UpgradeTarget::Jobs(JobsSource::BeforeJobMachine) {
        crate::owner::jobs_upgrade::convert_job_rows(write)?;
    }
    // Every graph shard generation stores its reservation rows with the retired `tombstone`
    // field; each upgrade ends at the current layout, so each one converts them
    // (MEDULLA-PLACEMENT-R781.4).
    if matches!(target, UpgradeTarget::GraphShard(_)) {
        crate::owner::offline_upgrade::reservation_tombstone::convert(write, row_cipher)?;
    }
    Ok(())
}

fn open_successor_tables(write: &WriteTransaction, target: UpgradeTarget) -> Result<(), String> {
    match target {
        UpgradeTarget::AgentLibrary(source) => open_agent_library_tables(write, source)?,
        UpgradeTarget::GraphShard(source) => open_graph_shard_tables(write, source)?,
        UpgradeTarget::SemanticIndex => open_promotion_tables(write)?,
        // Every Rbac generation ends at the current layout, so the grant table
        // (MEDULLA-DECISION-ENGINE-R036.106) and the data product table
        // (MEDULLA-TYPED-PACKS-R805.3) are created empty where the generation lacks them.
        UpgradeTarget::Rbac(_) => {
            write
                .open_table(RBAC_PROFILE_USE_GRANT)
                .map_err(|error| error.to_string())?;
            write
                .open_table(RBAC_DATA_PRODUCT)
                .map_err(|error| error.to_string())?;
        }
        // The Kv successor adds no table: the retired table is deleted above.
        UpgradeTarget::Kv => {}
        // Every Jobs generation ends at the current layout, so the two task-queue tables
        // (MEDULLA-INGEST-WORKER-R001.4.1) and the dead-letter table (R001.9) are created empty
        // for all of them. The record table of the job-machine generation is created by the
        // conversion above.
        UpgradeTarget::Jobs(_) => {
            write
                .open_table(TASK_QUEUE_TOTALS)
                .map_err(|error| error.to_string())?;
            write
                .open_table(TASK_QUEUE_OLDEST)
                .map_err(|error| error.to_string())?;
            write
                .open_table(TASK_DEAD_LETTERS)
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// Create, empty, the learned capability reward table.
fn open_capability_reward_table(write: &WriteTransaction) -> Result<(), String> {
    write
        .open_table(CAPABILITY_REWARD)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Create, empty, the known concept id and lineage edge tables.
fn open_concept_lineage_tables(write: &WriteTransaction) -> Result<(), String> {
    write
        .open_table(CONCEPT_IDS)
        .map_err(|error| error.to_string())?;
    write
        .open_table(CONCEPT_LINEAGE_EDGES)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Create, empty, every table the current Agent Library has that `source`
/// lacked. Every upgrade ends at the current layout, so the retry-allowance
/// table is created for all of them; each earlier table only for the
/// generations that predate it.
fn open_agent_library_tables(
    write: &WriteTransaction,
    source: AgentLibrarySource,
) -> Result<(), String> {
    if source == AgentLibrarySource::McpCatalog {
        write
            .open_table(MCP_CATALOG_CONFIGS)
            .map_err(|error| error.to_string())?;
        write
            .open_table(MCP_CATALOG_SCOPES)
            .map_err(|error| error.to_string())?;
    }
    if matches!(
        source,
        AgentLibrarySource::McpCatalog | AgentLibrarySource::ConceptLineage
    ) {
        open_concept_lineage_tables(write)?;
    }
    if source != AgentLibrarySource::RetryAllowances {
        open_capability_reward_table(write)?;
    }
    write
        .open_table(DECISION_RETRY_ALLOWANCES)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Create, empty, every table the current graph shard has that `source`
/// lacked. Every graph upgrade ends at the current layout, so the operation
/// audit-append idempotency index is created for all of them; the
/// repository-enrichment tables only for the generation that predates them.
fn open_graph_shard_tables(
    write: &WriteTransaction,
    source: GraphShardSource,
) -> Result<(), String> {
    if source == GraphShardSource::BeforeEnrichment {
        open_enrichment_tables(write)?;
    }
    write
        .open_table(WRITE_DAYS)
        .map_err(|error| error.to_string())?;
    write
        .open_table(GRAPH_CREATOR)
        .map_err(|error| error.to_string())?;
    write
        .open_table(XSHARD_DECISION_CERTIFICATE)
        .map_err(|error| error.to_string())?;
    write
        .open_table(EFFECT_RECEIPTS)
        .map_err(|error| error.to_string())?;
    write
        .open_table(ADMITTED_REQUESTS)
        .map_err(|error| error.to_string())?;
    write
        .open_table(RUN_CHECKPOINTS)
        .map_err(|error| error.to_string())?;
    write
        .open_table(CURRENT_PLANS)
        .map_err(|error| error.to_string())?;
    write
        .open_table(AUDIT_REQUESTS)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn open_enrichment_tables(write: &WriteTransaction) -> Result<(), String> {
    write
        .open_table(REPOSITORY_ENRICHMENT_BUDGETS)
        .map_err(|error| error.to_string())?;
    write
        .open_table(REPOSITORY_ENRICHMENT_POLICY_REVISIONS)
        .map_err(|error| error.to_string())?;
    write
        .open_table(REPOSITORY_ENRICHMENT_SUPERSESSIONS)
        .map_err(|error| error.to_string())?;
    write
        .open_table(REPOSITORY_ENRICHMENT_PARKS)
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Create, empty, the four promotion-gate tables of the semantic index
/// (MEDULLA-UNIFIED-DATA-PLANE-R040.3).
fn open_promotion_tables(write: &WriteTransaction) -> Result<(), String> {
    write
        .open_table(SEMANTIC_MODALITY_CHECKS)
        .map_err(|error| error.to_string())?;
    write
        .open_table(SEMANTIC_STAGE_RECEIPTS)
        .map_err(|error| error.to_string())?;
    write
        .open_table(SEMANTIC_PROMOTION_VERDICTS)
        .map_err(|error| error.to_string())?;
    write
        .open_table(SEMANTIC_EMBEDDING_VERSION_TAGS)
        .map(|_| ())
        .map_err(|error| error.to_string())
}
