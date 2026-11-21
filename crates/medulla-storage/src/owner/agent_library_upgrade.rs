//! Explicit offline, data-preserving owner-table transitions.
//!
//! Normal opens never run this migration. Inspection uses the same bounded,
//! anonymous native-recovery preview as the SQL owner upgrader; the exclusive
//! writer rechecks the pinned descriptor, byte fingerprint, table census and
//! strict typed evidence before its one immediate durable commit.

use super::domain::{
    AgentLibraryOwner, GraphShardOwner, JobsOwner, KvOwner, RbacOwner, SemanticIndexOwner,
};
use super::identity::PhysicalStoreIdentity;
use super::layout::{layout_digest_over, OwnerLayout};
use super::lineage::{
    AGENT_LIBRARY_BEFORE_CAPABILITY_REWARD, AGENT_LIBRARY_BEFORE_CONCEPT_LINEAGE,
    AGENT_LIBRARY_BEFORE_MCP_CATALOG, AGENT_LIBRARY_BEFORE_RETRY_ALLOWANCES,
    GRAPH_SHARD_BEFORE_AUDIT_REQUESTS, GRAPH_SHARD_BEFORE_CURRENT_PLANS,
    GRAPH_SHARD_BEFORE_ENRICHMENT, JOBS_BEFORE_JOB_MACHINE, KV_BEFORE_NAMESPACE_COLD,
    RBAC_BEFORE_DATA_PRODUCTS, RBAC_BEFORE_PROFILE_GRANTS, SEMANTIC_INDEX_BEFORE_PROMOTION,
};
use super::persisted_layout::LayoutPredecessor;
use super::registry::predecessor_evidence;
use super::sql_checkpoint_upgrade::{
    open_upgrade_writer, open_verified_upgrade_database, pin_upgrade_source, preview,
    read_predecessor_manifest, validate_descriptor_path, validate_exact_table_census,
    validate_pinned_upgrade_file, validate_reopened_predecessor,
    SqlSourceCheckpointInspectionOptions, ValidatedUpgradeSource, UPGRADE_CACHE_BYTES,
};
use crate::physical::incarnation::{StoreIncarnation, STORAGE_KERNEL_SCHEMA_VERSION};
use crate::physical::integrity::{authenticate_with, PrivatePayloadIntegrity};
use crate::physical::manifest::{OwnerManifest, TableContract, TableOwnership};
use crate::recovery::evidence::{HashSnapshot, StrictRecoveryEvidence};
use crate::recovery::validate::validate_recovery_content;
use crate::{StorageKernel, StoreOpenOptions};
use redb::{Database, MultimapTableHandle, ReadTransaction, ReadableDatabase, TableHandle};
use std::collections::BTreeSet;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

pub type AgentLibraryInspectionOptions = SqlSourceCheckpointInspectionOptions;
pub type GraphShardInspectionOptions = SqlSourceCheckpointInspectionOptions;

mod commit;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum UpgradeTarget {
    AgentLibrary(AgentLibrarySource),
    GraphShard(GraphShardSource),
    /// The semantic index before the promotion-gate records.
    SemanticIndex,
    /// An RBAC store of one generation; every one ends at the current layout.
    Rbac(RbacSource),
    /// The kv store while the KV-cache cold rung still had its own table.
    Kv,
    /// A jobs store of one generation; every one ends at the current layout.
    Jobs(JobsSource),
}

/// The RBAC generation an offline upgrade starts from. Every one ends at the current layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RbacSource {
    /// Before the profile-use grant records (MEDULLA-DECISION-ENGINE-R036.106).
    BeforeProfileGrants,
    /// Before the data product, contract and access request records (MEDULLA-TYPED-PACKS-R805.3).
    BeforeDataProducts,
}

impl RbacSource {
    fn descriptor(self) -> (&'static LayoutPredecessor, [u8; 32]) {
        match self {
            Self::BeforeProfileGrants => (
                &RBAC_BEFORE_PROFILE_GRANTS,
                super::lineage::RBAC_PRE_PROFILE_GRANTS_DIGEST,
            ),
            Self::BeforeDataProducts => (
                &RBAC_BEFORE_DATA_PRODUCTS,
                super::lineage::RBAC_PRE_DATA_PRODUCTS_DIGEST,
            ),
        }
    }
}

/// The jobs generation an offline upgrade starts from. Every one ends at the current layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum JobsSource {
    /// Before the analytics job machine (MEDULLA-CQ-ORCH-R811.4): every record is converted to
    /// the machine shape.
    BeforeJobMachine,
    /// Before the per-queue task totals and oldest-enqueue index (MEDULLA-INGEST-WORKER-R001.4.1).
    BeforeTaskQueue,
    /// Before the ingest dead-letter table (MEDULLA-INGEST-WORKER-R001.9).
    BeforeTaskDeadLetters,
}

impl JobsSource {
    fn descriptor(self) -> (&'static LayoutPredecessor, [u8; 32]) {
        match self {
            Self::BeforeJobMachine => (
                &JOBS_BEFORE_JOB_MACHINE,
                super::lineage::JOBS_PRE_JOB_MACHINE_DIGEST,
            ),
            Self::BeforeTaskQueue => (
                &super::lineage::JOBS_BEFORE_TASK_QUEUE,
                super::lineage::JOBS_PRE_TASK_QUEUE_DIGEST,
            ),
            Self::BeforeTaskDeadLetters => (
                &super::lineage::JOBS_BEFORE_TASK_DEAD_LETTERS,
                super::lineage::JOBS_PRE_TASK_DEAD_LETTERS_DIGEST,
            ),
        }
    }
}

/// The Agent Library generation an offline upgrade starts from. Every one ends
/// at the current layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentLibrarySource {
    /// Before served MCP catalog authority rows.
    McpCatalog,
    /// Before the known concept ids and concept lineage edges.
    ConceptLineage,
    /// Before the learned capability reward rows.
    CapabilityReward,
    /// Before the durable retry-allowance table.
    RetryAllowances,
}

impl AgentLibrarySource {
    fn descriptor(self) -> (&'static LayoutPredecessor, [u8; 32]) {
        match self {
            Self::McpCatalog => (
                &AGENT_LIBRARY_BEFORE_MCP_CATALOG,
                super::lineage::AGENT_LIBRARY_PRE_MCP_CATALOG_DIGEST,
            ),
            Self::ConceptLineage => (
                &AGENT_LIBRARY_BEFORE_CONCEPT_LINEAGE,
                super::lineage::AGENT_LIBRARY_PRE_CONCEPT_LINEAGE_DIGEST,
            ),
            Self::CapabilityReward => (
                &AGENT_LIBRARY_BEFORE_CAPABILITY_REWARD,
                super::lineage::AGENT_LIBRARY_PRE_CAPABILITY_REWARD_DIGEST,
            ),
            Self::RetryAllowances => (
                &AGENT_LIBRARY_BEFORE_RETRY_ALLOWANCES,
                super::lineage::AGENT_LIBRARY_PRE_RETRY_ALLOWANCES_DIGEST,
            ),
        }
    }
}

/// The graph-shard generation an offline upgrade starts from. Every one ends
/// at the current layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphShardSource {
    /// Before the repository-enrichment tables.
    BeforeEnrichment,
    /// Lacking only the operation audit-append idempotency index.
    BeforeAuditRequests,
    /// Before private current-plan snapshots.
    BeforeCurrentPlans,
    /// Before private context-bound run checkpoints.
    BeforeRunCheckpoints,
    /// Before work-item effect receipts and admitted requests.
    BeforeEffectReceipts,
    /// Before sealed coordinator-decision certificates.
    BeforeDecisionCertificates,
    /// Before the reservation record lost its `tombstone` field (MEDULLA-PLACEMENT-R781.4).
    BeforeReservationTombstoneRemoval,
    /// Before the per-graph write ledger and creator tables (MEDULLA-IDENTITY-R700.7).
    BeforeWriteLedger,
}

impl GraphShardSource {
    fn descriptor(self) -> (&'static LayoutPredecessor, [u8; 32]) {
        match self {
            Self::BeforeEnrichment => (
                &GRAPH_SHARD_BEFORE_ENRICHMENT,
                super::lineage::GRAPH_SHARD_PRE_ENRICHMENT_DIGEST,
            ),
            Self::BeforeAuditRequests => (
                &GRAPH_SHARD_BEFORE_AUDIT_REQUESTS,
                super::lineage::GRAPH_SHARD_PRE_AUDIT_REQUESTS_DIGEST,
            ),
            Self::BeforeCurrentPlans => (
                &GRAPH_SHARD_BEFORE_CURRENT_PLANS,
                super::lineage::GRAPH_SHARD_PRE_CURRENT_PLANS_DIGEST,
            ),
            Self::BeforeRunCheckpoints => (
                &super::lineage::GRAPH_SHARD_BEFORE_RUN_CHECKPOINTS,
                super::lineage::GRAPH_SHARD_PRE_RUN_CHECKPOINTS_DIGEST,
            ),
            Self::BeforeEffectReceipts => (
                &super::lineage::GRAPH_SHARD_BEFORE_EFFECT_RECEIPTS,
                super::lineage::GRAPH_SHARD_PRE_EFFECT_RECEIPTS_DIGEST,
            ),
            Self::BeforeDecisionCertificates => (
                &super::lineage::GRAPH_SHARD_BEFORE_DECISION_CERTIFICATES,
                super::lineage::GRAPH_SHARD_PRE_DECISION_CERTIFICATES_DIGEST,
            ),
            Self::BeforeReservationTombstoneRemoval => (
                &super::lineage::GRAPH_SHARD_BEFORE_RESERVATION_TOMBSTONE_REMOVAL,
                super::lineage::GRAPH_SHARD_PRE_RESERVATION_TOMBSTONE_REMOVAL_DIGEST,
            ),
            Self::BeforeWriteLedger => (
                &super::lineage::GRAPH_SHARD_BEFORE_WRITE_LEDGER,
                super::lineage::GRAPH_SHARD_PRE_WRITE_LEDGER_DIGEST,
            ),
        }
    }

    fn declared_generation(self) -> &'static LayoutPredecessor {
        self.descriptor().0
    }

    fn frozen_digest(self) -> [u8; 32] {
        self.descriptor().1
    }
}

impl UpgradeTarget {
    fn layout(self) -> OwnerLayout {
        match self {
            Self::AgentLibrary(_) => OwnerLayout::AgentLibrary,
            Self::GraphShard(_) => OwnerLayout::GraphShard,
            Self::SemanticIndex => OwnerLayout::SemanticIndex,
            Self::Rbac(_) => OwnerLayout::Rbac,
            Self::Kv => OwnerLayout::Kv,
            Self::Jobs(_) => OwnerLayout::Jobs,
        }
    }

    fn predecessor(self) -> &'static LayoutPredecessor {
        match self {
            Self::AgentLibrary(source) => source.descriptor().0,
            Self::GraphShard(source) => source.declared_generation(),
            Self::SemanticIndex => &SEMANTIC_INDEX_BEFORE_PROMOTION,
            Self::Rbac(source) => source.descriptor().0,
            Self::Kv => &KV_BEFORE_NAMESPACE_COLD,
            Self::Jobs(source) => source.descriptor().0,
        }
    }

    fn pinned(self) -> [u8; 32] {
        match self {
            Self::AgentLibrary(source) => source.descriptor().1,
            Self::GraphShard(source) => source.frozen_digest(),
            Self::SemanticIndex => super::lineage::SEMANTIC_INDEX_PRE_PROMOTION_DIGEST,
            Self::Rbac(source) => source.descriptor().1,
            Self::Kv => super::lineage::KV_PRE_NAMESPACE_COLD_DIGEST,
            Self::Jobs(source) => source.descriptor().1,
        }
    }
}

/// An exact predecessor inspection, tied to the source descriptor and bytes.
/// This token cannot be cloned or constructed outside the storage kernel.
pub struct ValidatedOwnerLayoutUpgrade {
    target: UpgradeTarget,
    source: ValidatedUpgradeSource,
}
pub type ValidatedAgentLibraryUpgrade = ValidatedOwnerLayoutUpgrade;
pub type ValidatedGraphShardUpgrade = ValidatedOwnerLayoutUpgrade;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerLayoutUpgradeReport {
    pub previous_layout_digest: [u8; 32],
    pub current_layout_digest: [u8; 32],
    pub previous_authority_epoch: u64,
    pub current_authority_epoch: u64,
}
pub type AgentLibraryUpgradeReport = OwnerLayoutUpgradeReport;
pub type GraphShardUpgradeReport = OwnerLayoutUpgradeReport;

/// Inspect an Agent Library owner file from before the known concept ids and
/// the concept lineage edge store (MEDULLA-DURABLE-KERNEL-R094.7.1).
pub fn inspect_agent_library_concept_lineage_upgrade(
    path: &Path,
    expected_physical_identity: PhysicalStoreIdentity,
    private_integrity: Option<Arc<dyn PrivatePayloadIntegrity>>,
    options: AgentLibraryInspectionOptions,
) -> Result<ValidatedAgentLibraryUpgrade, String> {
    inspect_layout_upgrade(
        path,
        expected_physical_identity,
        private_integrity,
        options,
        UpgradeTarget::AgentLibrary(AgentLibrarySource::ConceptLineage),
    )
}

/// Inspect an Agent Library owner file from before the learned capability
/// reward rows (MEDULLA-DECISION-ENGINE-R172.5.1).
pub fn inspect_agent_library_capability_reward_upgrade(
    path: &Path,
    expected_physical_identity: PhysicalStoreIdentity,
    private_integrity: Option<Arc<dyn PrivatePayloadIntegrity>>,
    options: AgentLibraryInspectionOptions,
) -> Result<ValidatedAgentLibraryUpgrade, String> {
    inspect_layout_upgrade(
        path,
        expected_physical_identity,
        private_integrity,
        options,
        UpgradeTarget::AgentLibrary(AgentLibrarySource::CapabilityReward),
    )
}

pub fn inspect_agent_library_mcp_catalog_upgrade(
    path: &Path,
    expected_physical_identity: PhysicalStoreIdentity,
    private_integrity: Option<Arc<dyn PrivatePayloadIntegrity>>,
    options: AgentLibraryInspectionOptions,
) -> Result<ValidatedAgentLibraryUpgrade, String> {
    inspect_layout_upgrade(
        path,
        expected_physical_identity,
        private_integrity,
        options,
        UpgradeTarget::AgentLibrary(AgentLibrarySource::McpCatalog),
    )
}

/// Inspect an Agent Library owner file from before the durable retry-allowance
/// table (MEDULLA-DECISION-ENGINE-R113.3.1.1).
pub fn inspect_agent_library_retry_allowance_upgrade(
    path: &Path,
    expected_physical_identity: PhysicalStoreIdentity,
    private_integrity: Option<Arc<dyn PrivatePayloadIntegrity>>,
    options: AgentLibraryInspectionOptions,
) -> Result<ValidatedAgentLibraryUpgrade, String> {
    inspect_layout_upgrade(
        path,
        expected_physical_identity,
        private_integrity,
        options,
        UpgradeTarget::AgentLibrary(AgentLibrarySource::RetryAllowances),
    )
}

/// Inspect a graph shard of the generation `source` names. A file of any
/// other generation, the current one included, is refused.
pub fn inspect_graph_shard_upgrade(
    path: &Path,
    expected_physical_identity: PhysicalStoreIdentity,
    private_integrity: Option<Arc<dyn PrivatePayloadIntegrity>>,
    options: GraphShardInspectionOptions,
    source: GraphShardSource,
) -> Result<ValidatedGraphShardUpgrade, String> {
    inspect_layout_upgrade(
        path,
        expected_physical_identity,
        private_integrity,
        options,
        UpgradeTarget::GraphShard(source),
    )
}

/// Inspect a semantic index owner file from before the promotion-gate
/// records (MEDULLA-UNIFIED-DATA-PLANE-R040.3).
pub fn inspect_semantic_index_promotion_upgrade(
    path: &Path,
    expected_physical_identity: PhysicalStoreIdentity,
    private_integrity: Option<Arc<dyn PrivatePayloadIntegrity>>,
    options: SqlSourceCheckpointInspectionOptions,
) -> Result<ValidatedOwnerLayoutUpgrade, String> {
    inspect_layout_upgrade(
        path,
        expected_physical_identity,
        private_integrity,
        options,
        UpgradeTarget::SemanticIndex,
    )
}

/// Inspect an RBAC owner file of the generation `source` names: from before the profile-use
/// grant records (MEDULLA-DECISION-ENGINE-R036.106) or before the data product records
/// (MEDULLA-TYPED-PACKS-R805.3).
pub(super) fn inspect_rbac_upgrade(
    path: &Path,
    expected_physical_identity: PhysicalStoreIdentity,
    private_integrity: Option<Arc<dyn PrivatePayloadIntegrity>>,
    options: SqlSourceCheckpointInspectionOptions,
    source: RbacSource,
) -> Result<ValidatedOwnerLayoutUpgrade, String> {
    inspect_layout_upgrade(
        path,
        expected_physical_identity,
        private_integrity,
        options,
        UpgradeTarget::Rbac(source),
    )
}

/// Inspect a kv owner file from before the KV-cache cold rung moved into a
/// namespace of the `kv` table (MEDULLA-DURABLE-KERNEL-R083.5).
pub fn inspect_kv_cold_tier_upgrade(
    path: &Path,
    expected_physical_identity: PhysicalStoreIdentity,
    private_integrity: Option<Arc<dyn PrivatePayloadIntegrity>>,
    options: SqlSourceCheckpointInspectionOptions,
) -> Result<ValidatedOwnerLayoutUpgrade, String> {
    inspect_layout_upgrade(
        path,
        expected_physical_identity,
        private_integrity,
        options,
        UpgradeTarget::Kv,
    )
}

pub(super) fn inspect_layout_upgrade(
    path: &Path,
    expected_physical_identity: PhysicalStoreIdentity,
    private_integrity: Option<Arc<dyn PrivatePayloadIntegrity>>,
    options: SqlSourceCheckpointInspectionOptions,
    target: UpgradeTarget,
) -> Result<ValidatedOwnerLayoutUpgrade, String> {
    let (pinned_file, incarnation, physical_path) = pin_upgrade_source(path)?;
    let (manifest, evidence, source_fingerprint) = preview::inspect_with(
        &pinned_file,
        &incarnation,
        &expected_physical_identity,
        private_integrity.as_deref(),
        &options,
        |database, incarnation, physical, integrity| {
            inspect_preview_read(database, incarnation, physical, integrity, target)
        },
    )?;
    let source_bytes = pinned_file
        .metadata()
        .map_err(|error| error.to_string())?
        .len();
    successor(&manifest, target)?;
    validate_descriptor_path(&pinned_file, path)?;
    if StoreIncarnation::derive(path)? != (incarnation.clone(), physical_path.clone()) {
        return Err("owner physical file changed during inspection".into());
    }
    pinned_file.unlock().map_err(|error| error.to_string())?;
    Ok(ValidatedOwnerLayoutUpgrade {
        target,
        source: ValidatedUpgradeSource::new(
            path,
            (pinned_file, incarnation, physical_path),
            (manifest, evidence, source_fingerprint),
            source_bytes,
            private_integrity,
        )
        .with_row_cipher(options.row_cipher()),
    })
}

pub fn upgrade_agent_library_mcp_catalog(
    token: ValidatedAgentLibraryUpgrade,
) -> Result<(StorageKernel, AgentLibraryUpgradeReport), String> {
    upgrade_layout(
        token,
        UpgradeTarget::AgentLibrary(AgentLibrarySource::McpCatalog),
    )
}

/// Bring an inspected Agent Library file from before the retry-allowance table
/// to the current layout in one durable commit: every existing row is kept and
/// the `decision_retry_allowance` table is created empty.
pub fn upgrade_agent_library_retry_allowances(
    token: ValidatedAgentLibraryUpgrade,
) -> Result<(StorageKernel, AgentLibraryUpgradeReport), String> {
    upgrade_layout(
        token,
        UpgradeTarget::AgentLibrary(AgentLibrarySource::RetryAllowances),
    )
}

/// Bring an inspected Agent Library file to the current layout in one durable
/// commit: every row is kept and the capability reward table is created empty.
pub fn upgrade_agent_library_capability_reward(
    token: ValidatedAgentLibraryUpgrade,
) -> Result<(StorageKernel, AgentLibraryUpgradeReport), String> {
    upgrade_layout(
        token,
        UpgradeTarget::AgentLibrary(AgentLibrarySource::CapabilityReward),
    )
}

/// Bring an inspected Agent Library to the current layout in one durable
/// commit: every row is kept and the concept id, lineage edge and capability
/// reward tables are created empty.
pub fn upgrade_agent_library_concept_lineage(
    token: ValidatedAgentLibraryUpgrade,
) -> Result<(StorageKernel, AgentLibraryUpgradeReport), String> {
    upgrade_layout(
        token,
        UpgradeTarget::AgentLibrary(AgentLibrarySource::ConceptLineage),
    )
}

/// Bring an inspected graph shard to the current layout in one durable
/// commit: every existing row is kept and each table its generation lacked is
/// created empty. The token carries the generation it was inspected as.
pub fn upgrade_graph_shard(
    token: ValidatedGraphShardUpgrade,
) -> Result<(StorageKernel, GraphShardUpgradeReport), String> {
    let target = token.target;
    if target.layout() != OwnerLayout::GraphShard {
        return Err("owner layout upgrade token targets another store".into());
    }
    upgrade_layout(token, target)
}

/// Bring an inspected jobs file to the current layout in one durable commit. A file from before
/// the analytics job machine has every record converted into `analytics_job_records` and the
/// retired table deleted; every generation gets the task-queue tables created empty. The token
/// carries the generation it was inspected as.
pub(super) fn upgrade_jobs(
    token: ValidatedOwnerLayoutUpgrade,
) -> Result<(StorageKernel, OwnerLayoutUpgradeReport), String> {
    let target = token.target;
    if target.layout() != OwnerLayout::Jobs {
        return Err("owner layout upgrade token targets another store".into());
    }
    upgrade_layout(token, target)
}

/// Bring an inspected semantic index file to the current layout in one
/// durable commit: every existing row is kept and the four promotion-gate
/// tables are created empty.
pub fn upgrade_semantic_index_promotion(
    token: ValidatedOwnerLayoutUpgrade,
) -> Result<(StorageKernel, OwnerLayoutUpgradeReport), String> {
    upgrade_layout(token, UpgradeTarget::SemanticIndex)
}

/// Bring an inspected RBAC file of either generation to the current layout in one durable
/// commit: every authority, reservation and grant row is kept and the profile-use grant and
/// data product tables are created empty where the generation lacks them. The token carries the
/// generation it was inspected as.
pub(super) fn upgrade_rbac(
    token: ValidatedOwnerLayoutUpgrade,
) -> Result<(StorageKernel, OwnerLayoutUpgradeReport), String> {
    let target = token.target;
    if target.layout() != OwnerLayout::Rbac {
        return Err("owner layout upgrade token targets another store".into());
    }
    upgrade_layout(token, target)
}

/// Bring an inspected kv file to the current layout in one durable commit: every `kv` row
/// is kept and the retired `eg_kvcache_cold` table is deleted with the cache bytes it held.
pub fn upgrade_kv_cold_tier(
    token: ValidatedOwnerLayoutUpgrade,
) -> Result<(StorageKernel, OwnerLayoutUpgradeReport), String> {
    upgrade_layout(token, UpgradeTarget::Kv)
}

pub(super) fn upgrade_layout(
    token: ValidatedOwnerLayoutUpgrade,
    target: UpgradeTarget,
) -> Result<(StorageKernel, OwnerLayoutUpgradeReport), String> {
    if token.target != target {
        return Err("owner layout upgrade token targets another store".into());
    }
    let file = open_upgrade_writer(&token.source.path)?;
    validate_descriptor(&token, &file)?;
    let (backend, opened_file) = preview::exclusive_backend(file, &token.source.path)?;
    let database = open_verified_upgrade_database(backend, &opened_file, &token.source, |file| {
        validate_descriptor(&token, file)
    })?;
    validate_reopened_predecessor(
        &database,
        &token.source.manifest,
        "owner predecessor manifest changed after inspection",
        |read| {
            inspect_predecessor(
                read,
                &token.source.incarnation,
                &token.source.manifest.physical_identity,
                token.source.private_integrity.as_deref(),
                target,
            )
            .map(|(manifest, _)| manifest)
        },
    )?;
    let current = commit::commit_upgrade(&database, &token, &opened_file, target)?;
    let report = OwnerLayoutUpgradeReport {
        previous_layout_digest: token.source.manifest.layout_digest,
        current_layout_digest: current.layout_digest,
        previous_authority_epoch: token.source.manifest.authority_epoch,
        current_authority_epoch: current.authority_epoch,
    };
    drop(database);
    drop(opened_file);
    let options = StoreOpenOptions::default().with_cache_bytes(UPGRADE_CACHE_BYTES)?;
    let kernel = match target {
        UpgradeTarget::AgentLibrary(_) => StorageKernel::open_owner_with::<AgentLibraryOwner>(
            &token.source.path,
            current.physical_identity,
            token.source.private_integrity,
            options,
        )?,
        UpgradeTarget::GraphShard(_) => StorageKernel::open_owner_with::<GraphShardOwner>(
            &token.source.path,
            current.physical_identity,
            token.source.private_integrity,
            options,
        )?,
        UpgradeTarget::SemanticIndex => StorageKernel::open_owner_with::<SemanticIndexOwner>(
            &token.source.path,
            current.physical_identity,
            token.source.private_integrity,
            options,
        )?,
        UpgradeTarget::Rbac(_) => StorageKernel::open_owner_with::<RbacOwner>(
            &token.source.path,
            current.physical_identity,
            token.source.private_integrity,
            options,
        )?,
        UpgradeTarget::Kv => StorageKernel::open_owner_with::<KvOwner>(
            &token.source.path,
            current.physical_identity,
            token.source.private_integrity,
            options,
        )?,
        UpgradeTarget::Jobs(_) => StorageKernel::open_owner_with::<JobsOwner>(
            &token.source.path,
            current.physical_identity,
            token.source.private_integrity,
            options,
        )?,
    };
    Ok((kernel, report))
}

fn contracts_for(
    manifest: &OwnerManifest,
    target: UpgradeTarget,
) -> Result<Vec<TableContract>, String> {
    let contracts: Vec<_> = super::contract::predecessor_contracts_of(target.predecessor())
        .into_iter()
        .filter(|contract| {
            contract.ownership == TableOwnership::Ledger
                || target
                    .predecessor()
                    .owner_tables
                    .contains(&contract.table_id.as_str())
        })
        .collect();
    // This is the frozen digest of the actual predecessor format. A file
    // with a self-consistent but fabricated old table contract is not admitted.
    let pinned = target.pinned();
    if manifest.layout_digest != pinned
        || layout_digest_over(target.layout(), &contracts) != pinned
        || manifest.tables != contracts
    {
        return Err("owner manifest is not the pinned layout predecessor".into());
    }
    Ok(contracts)
}

fn validate_predecessor(manifest: &OwnerManifest, target: UpgradeTarget) -> Result<(), String> {
    manifest.physical_identity.validate()?;
    if manifest.schema_version != STORAGE_KERNEL_SCHEMA_VERSION
        || manifest.layout != target.layout()
        || manifest.layout_digest == target.layout().digest()
    {
        return Err("owner manifest is not an exact layout predecessor".into());
    }
    contracts_for(manifest, target)?;
    Ok(())
}

fn successor(manifest: &OwnerManifest, target: UpgradeTarget) -> Result<OwnerManifest, String> {
    let mut next = OwnerManifest::new(manifest.physical_identity.clone(), target.layout())?;
    next.authority_epoch = manifest
        .authority_epoch
        .checked_add(1)
        .ok_or_else(|| "owner layout authority epoch exhausted".to_string())?;
    next.validate()?;
    Ok(next)
}

fn inspect_preview_read(
    database: &Database,
    incarnation: &StoreIncarnation,
    physical: &PhysicalStoreIdentity,
    integrity: Option<&dyn PrivatePayloadIntegrity>,
    target: UpgradeTarget,
) -> Result<(OwnerManifest, StrictRecoveryEvidence), String> {
    let read = database.begin_read().map_err(|error| error.to_string())?;
    inspect_predecessor(&read, incarnation, physical, integrity, target)
}

fn inspect_predecessor(
    read: &ReadTransaction,
    incarnation: &StoreIncarnation,
    physical: &PhysicalStoreIdentity,
    integrity: Option<&dyn PrivatePayloadIntegrity>,
    target: UpgradeTarget,
) -> Result<(OwnerManifest, StrictRecoveryEvidence), String> {
    let manifest = read_predecessor_manifest(read, incarnation)?;
    validate_predecessor(&manifest, target)?;
    if manifest.physical_identity != *physical {
        return Err("layout predecessor physical owner mismatch".into());
    }
    validate_census(
        &manifest,
        target,
        read.list_tables().map_err(|error| error.to_string())?,
        read.list_multimap_tables()
            .map_err(|error| error.to_string())?,
    )?;
    let evidence = predecessor_evidence(
        HashSnapshot::Read(read),
        target.layout(),
        target.predecessor().owner_tables,
    )?;
    let authenticate = |sealed: &[u8], digest: &str| authenticate_with(integrity, sealed, digest);
    validate_recovery_content(incarnation, read, &authenticate)?;
    Ok((manifest, evidence))
}

fn validate_census<N, M>(
    manifest: &OwnerManifest,
    target: UpgradeTarget,
    normal: N,
    multimap: M,
) -> Result<(), String>
where
    N: IntoIterator,
    N::Item: TableHandle,
    M: IntoIterator,
    M::Item: MultimapTableHandle,
{
    let expected: BTreeSet<_> = contracts_for(manifest, target)?
        .into_iter()
        .map(|contract| contract.table_id)
        .collect();
    validate_exact_table_census(
        expected,
        normal,
        multimap,
        "layout predecessor table census is not exact",
    )
}

fn validate_descriptor(token: &ValidatedAgentLibraryUpgrade, file: &File) -> Result<(), String> {
    validate_pinned_upgrade_file(
        &token.source.pinned_file,
        file,
        &token.source.path,
        (
            &token.source.incarnation,
            token.source.physical_path.as_path(),
        ),
        "owner physical file changed after inspection",
    )
}

#[cfg(test)]
mod tests;
