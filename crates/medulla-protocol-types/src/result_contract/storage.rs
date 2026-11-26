//! Declared results of the `storage` contract domain.

use super::Dynamic;
#[cfg(feature = "query")]
use crate::storage_wire::SqlSourceBatchResult;
use crate::storage_wire::{
    BackupReceipt, KvCasOutcome, KvEntry, KvNamespaceClass, KvScanPage, KvWriteReceipt,
    RestoreReceipt, SqliteExportReport, SqliteImportReport,
};
use medulla_agent_types::agent_component::AgentComponentCommittedResult;
use medulla_agent_types::agent_component::BlueprintExport;
use medulla_agent_types::agent_component::PromptReceiptVerdict;
use medulla_agent_types::agent_component::PromptRenderResult;
use medulla_agent_types::agent_component::BlueprintProposalResult;
use medulla_agent_types::agent_component::SourceEffectRecordResult;
use medulla_agent_types::agent_component::AgentComponentContentResult;
use medulla_agent_types::agent_component::AgentComponentEntry;
use medulla_agent_types::agent_component::AgentComponentSearchPage;
use medulla_agent_types::agent_graph::AgentGraphCommittedResult;
use medulla_agent_types::agent_graph::AgentGraphEntry;
use medulla_agent_types::agent_guardrail::GuardrailEntailmentResult;
use medulla_agent_types::agent_library::AgentLibraryEntry;
use medulla_agent_types::agent_library::AgentLibraryEntryDraft;
use medulla_agent_types::agent_library::AgentLibraryPage;
use medulla_agent_types::agent_library::AgentLibraryWriteResult;
use medulla_agent_types::agent_template::AgentTemplateCommittedResult;
use medulla_agent_types::agent_template::AgentTemplateEntry;
use medulla_agent_types::concept_lineage::{ConceptEdgeRecord, ConceptReadResult, ConceptRecord};
use medulla_catalog_types::connector_pack::conformance_evidence::ConformanceEvidence;
use medulla_catalog_types::connector_pack::ConnectorPackStatus as ConnectorPackStatusBody;
use medulla_catalog_types::skill_admission::SkillAdmission;
use medulla_catalog_types::{
    connector_pack::ConnectorPackBindingResult, connector_pack::ConnectorPackSourceApprovalResult,
    connector_pack::McpCatalogSnapshotBinding, connector_pack::PackBodyReconcileReport,
    connector_pack::PackImportReceipt, connector_pack::PackImportResult,
    connector_pack::PackRetireResult,
};
use medulla_decision_outcome_types::decision::coverage::CapabilityCoverageResult;
use medulla_decision_outcome_types::decision::execution_answer::ExecutionAnswer;
use medulla_decision_types::{decision::AssemblyResult, decision::DecisionCommitResult};
use medulla_mutation_types::{
    write_back::ReconciliationReceipt, write_back::SourceChangeSet, write_back::WriteBackReceipt,
    write_back::WriteBackReceiptPage,
};
use medulla_query_types::series_expr::DerivedSeriesReceipt;

method_results! {
    visit_storage;
    // The graph snapshot's MessagePack bytes, carried as a JSON array of byte values.
    ToMsgpack(ToMsgpack) => Json<Vec<u8>>;
    FromMsgpack(FromMsgpack) => Text<String>;
    ClearLedger(ClearLedger) => Text<String>;
    ApplyLedger(ApplyLedger) => Text<String>;
    Backup(Backup) => Json<BackupReceipt>;
    Restore(Restore) => Json<RestoreReceipt>;
    AgentLibraryCurrent(AgentLibrary / "current") => Raw<Option<AgentLibraryEntry>>;
    AgentLibraryHistory(AgentLibrary / "history") => Raw<Vec<AgentLibraryEntry>>;
    AgentLibraryPublish(AgentLibrary / "publish") => Raw<AgentLibraryWriteResult>;
    AgentLibraryRetire(AgentLibrary / "retire") => Raw<AgentLibraryWriteResult>;
    AgentLibraryStatus(AgentLibrary / "status") => Raw<Option<AgentLibraryWriteResult>>;
    AgentLibraryList(AgentLibrary / "list") => Raw<AgentLibraryPage>;
    AgentLibraryGet(AgentLibrary / "get") => Raw<Option<medulla_agent_types::agent_library::AgentLibraryListEntry>>;
    AgentGraphCurrent(AgentGraph / "current") => Raw<Option<AgentGraphEntry>>;
    AgentGraphHistory(AgentGraph / "history") => Raw<Vec<AgentGraphEntry>>;
    AgentGraphPublish(AgentGraph / "publish") => Raw<AgentGraphCommittedResult>;
    AgentGraphRetire(AgentGraph / "retire") => Raw<AgentGraphCommittedResult>;
    AgentGraphStatus(AgentGraph / "status") => Raw<Option<AgentGraphCommittedResult>>;
    AgentComponentCurrent(AgentComponent / "current") => Raw<Option<AgentComponentEntry>>;
    AgentComponentHistory(AgentComponent / "history") => Raw<Vec<AgentComponentEntry>>;
    AgentComponentPublish(AgentComponent / "publish") => Raw<AgentComponentCommittedResult>;
    AgentComponentRetire(AgentComponent / "retire") => Raw<AgentComponentCommittedResult>;
    AgentComponentSearch(AgentComponent / "search") => Raw<AgentComponentSearchPage>;
    AgentComponentStatus(AgentComponent / "status") => Raw<Option<AgentComponentCommittedResult>>;
    AgentTemplateCurrent(AgentTemplate / "current") => Raw<Option<AgentTemplateEntry>>;
    AgentTemplateHistory(AgentTemplate / "history") => Raw<Vec<AgentTemplateEntry>>;
    AgentTemplateInstantiate(AgentTemplate / "instantiate") => Raw<AgentLibraryEntryDraft>;
    AgentTemplatePublish(AgentTemplate / "publish") => Raw<AgentTemplateCommittedResult>;
    AgentTemplateRetire(AgentTemplate / "retire") => Raw<AgentTemplateCommittedResult>;
    AgentTemplateStatus(AgentTemplate / "status") => Raw<Option<AgentTemplateCommittedResult>>;
    TsAppend(TsAppend) => Count<u64>;
    // `(ts, values)` points in timestamp order.
    TsRange(TsRange) => Raw<Vec<(i64, Vec<f64>)>>;
    // One matched value (or none) per caller timestamp, in the caller's order.
    TsAsofJoin(TsAsofJoin) => Raw<Vec<Option<f64>>>;
    // `(bucket_start, value, count)` per non-empty bucket.
    TsWindow(TsWindow) => Raw<Vec<(i64, f64, usize)>>;
    // `(grid_ts, value, carried_forward)`; a value before the first observation is NaN.
    TsGapFill(TsGapFill) => Raw<Vec<(i64, f64, bool)>>;
    TsEvict(TsEvict) => Count<u64>;
    TsDeleteSeries(TsDeleteSeries) => Count<u64>;
    TsListSeries(TsListSeries) => Raw<Vec<String>>;
    TsDefineSeries(TsDefineSeries) => Raw<DerivedSeriesReceipt>;
    TsDefineStateSpace(TsDefineStateSpace) => Raw<DerivedSeriesReceipt>;
    BlobBegin(BlobBegin) => Count<u64>;
    BlobChunkPut(BlobChunkPut) => Count<u64>;
    BlobCommit(BlobCommit) => Text<String>;
    // `(cursor, chunk_count)`.
    BlobFetchBegin(BlobFetchBegin) => Raw<(u64, u32)>;
    // One stored chunk of a caller-uploaded blob, as a MessagePack `bin`.
    BlobChunkGet(BlobChunkGet) => Raw<Dynamic> dynamic CallerBytes;
    BlobFetchEnd(BlobFetchEnd) => Bool<bool>;
    BlobRef(BlobRef) => Count<u64>;
    BlobUnref(BlobUnref) => Count<u64>;
    // `(blobs_reclaimed, chunks_reclaimed)`.
    BlobGc(BlobGc) => Raw<(u64, u64)>;
    // The lowercase hex SHA-256 of a stored media object (MEDULLA-MEDIA-VOCAB-R657.1).
    MediaPut(MediaPut) => Text<String>;
    // One media object of the caller's tenant, as a MessagePack `bin` (MEDULLA-MEDIA-VOCAB-R657.2).
    MediaGet(MediaGet) => Raw<Dynamic> dynamic CallerBytes;
    // Whether the tenant's hold was released (MEDULLA-MEDIA-VOCAB-R657.3).
    MediaTombstone(MediaTombstone) => Bool<bool>;
    // The stored value bytes and their version, or null when the key is absent.
    KvGet(KvGet) => Raw<Option<KvEntry>>;
    KvPut(KvPut) => Raw<KvWriteReceipt>;
    KvDelete(KvDelete) => Bool<bool>;
    // One bounded page of `(key, value, version)` rows and the next-page cursor.
    KvScan(KvScan) => Raw<KvScanPage>;
    KvCas(KvCas) => Raw<KvCasOutcome>;
    // The namespace's stored (or default) durability class.
    KvDeclareNamespace(KvDeclareNamespace) => Raw<KvNamespaceClass>;
    KvNamespaceDeclaration(KvNamespaceDeclaration) => Raw<KvNamespaceClass>;
    #[cfg(feature = "query")]
    SqlSourceBatch(SqlSourceBatch) => Raw<SqlSourceBatchResult>;
    AgentComponentContent(AgentComponent / "content") => Raw<AgentComponentContentResult>;
    AgentComponentBlueprintExport(AgentComponent / "export_blueprint") => Raw<BlueprintExport>;
    AgentComponentImportBlueprint(AgentComponent / "import_blueprint") => Raw<AgentComponentCommittedResult>;
    AgentComponentEditSkillSource(AgentComponent / "edit_skill_source") => Raw<AgentComponentCommittedResult>;
    AgentComponentRender(AgentComponent / "render") => Raw<PromptRenderResult>;
    AgentComponentVerifyRender(AgentComponent / "verify_render") => Raw<PromptReceiptVerdict>;
    AgentComponentStoreProposal(AgentComponent / "store_proposal") => Raw<BlueprintProposalResult>;
    AgentComponentRecordSourceEffect(AgentComponent / "record_source_effect") => Raw<SourceEffectRecordResult>;
    AgentComponentAdmitSkillUpgrade(AgentComponent / "admit_skill_upgrade") => Raw<SkillAdmission>;
    AgentAssemble(AgentAssemble) => Raw<AssemblyResult>;
    CapabilityCoverage(CapabilityCoverage) => Raw<CapabilityCoverageResult>;
    AgentExecutionAnswer(AgentExecutionAnswer) => Raw<ExecutionAnswer>;
    RevisionProposalList(RevisionProposalList) => Raw<medulla_decision_outcome_types::decision::revision_list::RevisionProposalPage>;
    GuardrailEntailment(GuardrailEntailment) => Raw<GuardrailEntailmentResult>;
    DecisionCommit(DecisionCommit) => Raw<DecisionCommitResult>;
    ConnectorPackStatus(ConnectorPack / "status") => Raw<ConnectorPackStatusBody>;
    ConnectorPackImport(ConnectorPack / "import") => Raw<PackImportResult>;
    ConnectorPackApproveSource(ConnectorPack / "approve_source") => Raw<ConnectorPackSourceApprovalResult>;
    ConnectorPackBind(ConnectorPack / "bind") => Raw<ConnectorPackBindingResult>;
    ConnectorPackUnbind(ConnectorPack / "unbind") => Raw<ConnectorPackBindingResult>;
    ConnectorPackRetire(ConnectorPack / "retire") => Raw<PackRetireResult>;
    ConnectorPackReproject(ConnectorPack / "reproject") => Raw<PackImportReceipt>;
    ConnectorPackReconcileBodies(ConnectorPack / "reconcile_bodies") => Raw<PackBodyReconcileReport>;
    ConnectorPackReconcileCatalog(ConnectorPack / "reconcile_catalog") => Raw<McpCatalogSnapshotBinding>;
    ConnectorPackAttestSelfServedCatalog(ConnectorPack / "attest_self_served_catalog") => Raw<McpCatalogSnapshotBinding>;
    ConnectorPackCatalogAuthorityStatus(ConnectorPack / "catalog_authority_status") => Raw<Option<McpCatalogSnapshotBinding>>;
    ConnectorPackCatalogOwnerPrincipal(ConnectorPack / "catalog_owner_principal") => Raw<String>;
    ConnectorPackCatalogBindingStatus(ConnectorPack / "catalog_binding_status") => Raw<Option<McpCatalogSnapshotBinding>>;
    ConnectorPackCatalogRequestOwnerPrincipal(ConnectorPack / "catalog_request_owner_principal") => Raw<String>;
    ConnectorPackConformanceEvidence(ConnectorPack / "conformance_evidence") => Raw<ConformanceEvidence>;
    ConnectorPackRevokeConformanceEvidence(ConnectorPack / "revoke_conformance_evidence") => Raw<ConformanceEvidence>;
    DataProductRegister(DataProductRegister) => Raw<medulla_catalog_types::data_product::DataProductRegistered>;
    AccessRequestOpen(AccessRequestOpen) => Raw<medulla_catalog_types::data_product::AccessRequest>;
    AccessRequestDecide(AccessRequestDecide) => Raw<medulla_catalog_types::data_product::AccessRequest>;
    WriteBackCreate(WriteBack / "create") => Raw<SourceChangeSet>;
    WriteBackGet(WriteBack / "get") => Raw<Option<SourceChangeSet>>;
    WriteBackRecordAttempt(WriteBack / "record_attempt") => Raw<WriteBackReceipt>;
    WriteBackRecordReconciliation(WriteBack / "record_reconciliation") => Raw<ReconciliationReceipt>;
    WriteBackReceipts(WriteBack / "receipts") => Raw<WriteBackReceiptPage>;
    ConceptLineageClaim(ConceptLineage / "claim") => Raw<ConceptRecord>;
    ConceptLineageAddEdge(ConceptLineage / "add_edge") => Raw<ConceptEdgeRecord>;
    ConceptLineageRead(ConceptLineage / "read") => Raw<ConceptReadResult>;
    // MEDULLA-DURABLE-KERNEL-R700.3: the report of one retention cleanup step.
    RetentionCleanupRun(RetentionCleanupRun) => Raw<medulla_mutation_types::retention_cleanup::RetentionCleanupReport>;
    ImportSqliteFile(ImportSqliteFile) => Json<SqliteImportReport>;
    ExportSqliteFile(ExportSqliteFile) => Json<SqliteExportReport>;
}
