//! Declared results of the `query` contract domain.

use super::Dynamic;
use crate::change_envelope::{ChangeCursor, ChangeEnvelopeRecord, ContentVersion};
#[cfg(feature = "epistemic")]
use crate::protocol::{
    CausalCounterfactualResult, CausalEstimateResult, EpistemicStatusResult, ExplainBeliefResponse,
    ExplainEvidenceResult, MaterializationStatusResult, RankByProvenanceResult,
    RecomputeMaterializationResult, ResolveConflictResult, StaleMaterializationsResult,
    WhatChangedResult,
};
#[cfg(feature = "query")]
use crate::protocol::{ExplainPlanResult, ExplainPolicyResult};
use medulla_decision_types::decision::DecisionBatch;
use medulla_index_types::{managed_index::EdgeIndexStatusView, managed_index::EdgeSearchView};
#[cfg(feature = "knowledge-batch")]
use medulla_ingestion_types::knowledge_stream::KnowledgeStreamBatch;
use medulla_result_types::epistemic_operations::EvidenceBundle;
use medulla_types::types::ContextView;

method_results! {
    visit_query;
    GetContextView(GetContextView) => Json<ContextView>;
    GetChangeEnvelope(GetChangeEnvelope) => Raw<Option<ChangeEnvelopeRecord>>;
    GetContentVersion(GetContentVersion) => Raw<Option<ContentVersion>>;
    GetChangeCursor(GetChangeCursor) => Raw<Option<ChangeCursor>>;
    // `{columns, rows}` with each row a MessagePack array of the statement's cells.
    Sql(Sql) => Raw<Dynamic> dynamic QueryRows;
    // `{columns, rows}` with each row a MessagePack array of the query's cells.
    CypherQuery(CypherQuery) => Raw<Dynamic> dynamic QueryRows;
    // The GraphQL `{data: ...}` response the document selects.
    GraphQl(GraphQl) => Raw<Dynamic> dynamic QueryRows;
    #[cfg(feature = "knowledge-batch")]
    KnowledgeStream(KnowledgeStream) => Raw<KnowledgeStreamBatch>;
    // `[node id, score | nil]` rows of the cross-modal plan; a source row adds its named
    // text values as a third element.
    UnifiedQuery(UnifiedQuery) => Raw<Vec<medulla_types::unified_row::UnifiedRow>>;
    // MEDULLA-REPO-INGEST-R006.3: one page of a revision-bound code map.
    UnifiedQueryCodeMap(UnifiedQuery / "code_map") => Raw<medulla_types::code_map::CodeMapResult>;
    UnifiedQuerySpecReport(UnifiedQuery / "spec_report") => Raw<medulla_types::spec_report::SpecIntelligenceReport>;
    // MEDULLA-FEDERATED-QUERY-R039: the rows of a plan that sets `report_shortfall`, with the typed shortfall.
    UnifiedQueryShortfall(UnifiedQuery / "shortfall") => Raw<medulla_types::retrieval::RankedRows>;
    LiveArtifactPut(LiveArtifactPut) => Json<medulla_source_types::live_artifact::LiveArtifact>;
    LiveArtifactGet(LiveArtifactGet) => Json<medulla_source_types::live_artifact::LiveArtifactView>;
    LiveArtifactRefresh(LiveArtifactRefresh) => Json<medulla_source_types::live_artifact::RefreshEntry>;
    // MEDULLA-FEDERATED-QUERY-R708.3: the published record, and one call's rows with their freshness.
    // MEDULLA-FEDERATED-QUERY-R039.2.5: the receipt and vector of one encoded document, and a search over them.
    EncodeDocument(EncodeDocument) => Json<medulla_types::retrieval::EncodedDocument>;
    EncodedDocumentSearch(EncodedDocumentSearch) => Json<medulla_types::retrieval::EncodedSearchResult>;
    ProjectionRead(ProjectionRead) => Json<medulla_types::projection_read::ProjectionReadResponse>;
    StoredQueryPut(StoredQueryPut) => Json<medulla_source_types::stored_query::StoredQueryReceipt>;
    ApproveEntityLink(ApproveEntityLink) => Json<medulla_catalog_types::entity_link_proposal::EntityLinkApproval>;
    StoredQueryCall(StoredQueryCall) => Json<medulla_source_types::stored_query::StoredCall>;
    // MEDULLA-FEDERATED-QUERY-R708.12: one version as written, one page of names, one tombstone.
    StoredQueryGet(StoredQueryGet) => Json<medulla_source_types::stored_query::StoredQueryView>;
    StoredQueryList(StoredQueryList) => Json<medulla_source_types::stored_query::StoredQueryPage>;
    StoredQueryRetire(StoredQueryRetire) => Json<medulla_source_types::stored_query::StoredQueryRetired>;
    RetrievalFeedbackRecord(RetrievalFeedbackRecord) => Json<medulla_result_types::feedback_event::RetrievalFeedback>;
    ArgumentMapPut(ArgumentMapPut) => Json<medulla_result_types::argument_map::ArgumentMap>;
    ArgumentMapGet(ArgumentMapGet) => Json<medulla_result_types::argument_map::ArgumentMap>;
    ArgumentMapEvaluate(ArgumentMapEvaluate) => Json<medulla_result_types::argument_map::ExtensionResult>;
    #[cfg(feature = "federation")]
    QueryForeignColumns(QueryForeignColumns) => Raw<Vec<medulla_query_types::wire::ForeignColumnRow>>;
    // MEDULLA-FEDERATED-QUERY-R085.3: the bounded validity of one source-query approval.
    VerifySourceQueryApproval(VerifySourceQueryApproval) => Raw<medulla_result_types::source_query::SourceQueryApprovalValidity>;
    #[cfg(feature = "federation")]
    ForeignReadOpen(ForeignReadOpen) => Raw<medulla_catalog_types::foreign_read::ForeignReadOpenResult>;
    #[cfg(feature = "federation")]
    ForeignReadChunk(ForeignReadChunk) => Raw<medulla_catalog_types::foreign_read::ForeignReadChunkResult>;
    #[cfg(feature = "federation")]
    ForeignReadComplete(ForeignReadComplete) => Raw<medulla_catalog_types::foreign_read::ForeignReadCompleteResult>;
    #[cfg(feature = "federation")]
    ForeignReadAbort(ForeignReadAbort) => Raw<medulla_catalog_types::foreign_read::ForeignReadAbortResult>;
    // A UQL statement's rows (with named score channels), EXPLAIN report or PROFILE.
    #[cfg(feature = "query")]
    Uql(Uql) => Raw<medulla_query_types::wire::UqlResult>;
    // MEDULLA-UNIFIED-DATA-PLANE-R763.2: column edges and gaps of one SQL query.
    // MEDULLA-FEDERATED-QUERY-R650.3: the compiled context bundle with its certificate.
    // MEDULLA-FEDERATED-QUERY-R081.11: the rows and verdicts of one retrieval round.
    #[cfg(feature = "query")]
    RetrievalRound(RetrievalRound) => Json<medulla_result_types::retrieval_round::RoundResult>;
    // MEDULLA-FEDERATED-QUERY-R653.3: the rows of one corpus mode.
    CorpusRetrieve(CorpusRetrieve) => Raw<medulla_types::corpus_retrieval::CorpusResult>;
    // MEDULLA-FEDERATED-QUERY-R081.103.1: the bins of one viewport query.
    ViewportQuery(ViewportQuery) => Raw<medulla_types::viewport::ViewportResult>;
    // MEDULLA-FEDERATED-QUERY-R081.103.3: the raw points of one resolved selection token.
    ResolveViewportSelection(ResolveViewportSelection) => Raw<medulla_types::viewport::ResolvedSelection>;
    // MEDULLA-SRC-AUTH-R004.6.1: the verdicts and audit of one source-coverage read.
    SourceCoverage(SourceCoverage) => Raw<medulla_identity_types::source_coverage::SourceCoverageReply>;
    // MEDULLA-DECISION-ENGINE-R100.100.4: the spans of one context search.
    RetrievalContextSearch(RetrievalContextSearch) => Raw<medulla_decision_types::decision::statistical::context_search::RetrievalContextSearchResult>;
    // MEDULLA-DECISION-ENGINE-R144.3: the rows and the cost report of one indexless retrieval.
    IndexlessRetrieve(IndexlessRetrieve) => Raw<medulla_decision_types::decision::indexless_retrieval::IndexlessResult>;
    // MEDULLA-FEDERATED-QUERY-R702.3: the accepted task or the typed rejection of one synthesis.
    SynthesizeSearchTask(SynthesizeSearchTask) => Raw<medulla_work_types::search_task::SynthesizeSearchTaskAnswer>;
    // MEDULLA-FEDERATED-QUERY-R654.3: code context and symptom diagnosis.
    CodeContext(CodeContext) => Raw<medulla_types::code_context::CodeContextAnswer>;
    // MEDULLA-REPO-INGEST-R700.3: the findings of one code-health sweep.
    CodeHealthSweep(CodeHealthSweep) => Raw<medulla_ingestion_types::code_health::CodeHealthReport>;
    Diagnose(Diagnose) => Raw<medulla_types::code_context::SymptomDiagnosis>;
    // MEDULLA-MEDIA-VOCAB-R001.102.2, R001.102.3: bookmark search and relations.
    BookmarkSemanticSearch(BookmarkSemanticSearch) => Raw<medulla_types::bookmark_query::BookmarkSearchResult>;
    BookmarkRelations(BookmarkRelations) => Raw<medulla_types::bookmark_query::BookmarkRelationsResult>;
    ContextCompile(ContextCompile) => Raw<medulla_decision_types::decision::context_bundle::ContextBundle>;
    ColumnLineage(ColumnLineage) => Raw<medulla_catalog_types::lineage::LineageResult>;
    // MEDULLA-CQ-XSRC-R008.4: the report of one cross-source question.
    CrossSourceQuestion(CrossSourceQuestion) => Raw<medulla_source_types::cross_source_report::CrossSourceReport>;
    // MEDULLA-CQ-XSRC-R011: the supply-chain impact report.
    SupplyChainImpact(SupplyChainImpact) => Raw<medulla_source_types::supply_chain_report::SupplyChainImpactReport>;
    // MEDULLA-UNIFIED-DATA-PLANE-R764.8: frequent users and usage suggestions of one asset.
    CatalogUsage(CatalogUsage) => Raw<medulla_catalog_types::asset_usage::CatalogUsageAnswer>;
    // MEDULLA-TYPED-PACKS-R108.17: every catalog read answers one `CatalogReadAnswer`.
    CatalogReadBrowse(CatalogRead / "browse") => Raw<medulla_catalog_types::catalog_read::CatalogReadAnswer>;
    CatalogReadSearch(CatalogRead / "search") => Raw<medulla_catalog_types::catalog_read::CatalogReadAnswer>;
    CatalogReadDetail(CatalogRead / "detail") => Raw<medulla_catalog_types::catalog_read::CatalogReadAnswer>;
    // MEDULLA-TYPED-PACKS-R108.18: the owners and stewards of a target, and one page of curation work.
    CatalogReadGovernance(CatalogRead / "governance") => Raw<medulla_catalog_types::catalog_read::CatalogReadAnswer>;
    CatalogReadProposals(CatalogRead / "proposals") => Raw<medulla_catalog_types::catalog_read::CatalogReadAnswer>;
    // MEDULLA-UNIFIED-DATA-PLANE-R029.2: what one attached table is, joins to, maps to and impacts.
    SchemaContext(SchemaContext) => Raw<medulla_catalog_types::schema_context::SchemaContextAnswer>;
    #[cfg(feature = "query")]
    ExplainPlan(ExplainPlan) => Raw<ExplainPlanResult>;
    ExplainProvenance(ExplainProvenance) => Raw<EvidenceBundle>;
    ExplainProvenanceByIds(ExplainProvenanceByIds) => Raw<EvidenceBundle>;
    #[cfg(feature = "query")]
    ExplainPolicy(ExplainPolicy) => Raw<ExplainPolicyResult>;
    #[cfg(feature = "epistemic")]
    ExplainBelief(ExplainBelief) => Raw<ExplainBeliefResponse>;
    #[cfg(feature = "epistemic")]
    EpistemicStatus(EpistemicStatus) => Raw<EpistemicStatusResult>;
    #[cfg(feature = "epistemic")]
    WhatChanged(WhatChanged) => Raw<WhatChangedResult>;
    #[cfg(feature = "epistemic")]
    RecomputeMaterialization(RecomputeMaterialization) => Raw<RecomputeMaterializationResult>;
    #[cfg(feature = "epistemic")]
    MaterializationStatus(MaterializationStatus) => Raw<MaterializationStatusResult>;
    #[cfg(feature = "epistemic")]
    StaleMaterializations(StaleMaterializations) => Raw<StaleMaterializationsResult>;
    #[cfg(feature = "epistemic")]
    ResolveConflict(ResolveConflict) => Raw<ResolveConflictResult>;
    #[cfg(feature = "epistemic")]
    ExplainEvidence(ExplainEvidence) => Raw<ExplainEvidenceResult>;
    #[cfg(feature = "epistemic")]
    CausalEstimate(CausalEstimate) => Raw<CausalEstimateResult>;
    #[cfg(feature = "epistemic")]
    CausalCounterfactual(CausalCounterfactual) => Raw<CausalCounterfactualResult>;
    #[cfg(feature = "epistemic")]
    CausalVerify(CausalVerify) => Raw<medulla_query_types::causal_wire::CausalVerdict>;
    #[cfg(feature = "epistemic")]
    RankByProvenance(RankByProvenance) => Raw<RankByProvenanceResult>;
    NlQuery(NlQuery) => Raw<Vec<(String, Option<f32>)>>;
    // The executed UQL text alongside the same `[node id, score | nil]` rows as `NlQuery`.
    NlQueryExplain(NlQueryExplain) => Raw<medulla_query_types::nl_query::NlQueryExplainResult>;
    // The recorded certified correction (MEDULLA-FEDERATED-QUERY-R058.7).
    NlQueryCorrect(NlQueryCorrect) => Raw<medulla_query_types::nl_query::NlCorrection>;
    // The unexecuted UQL candidate and its pins for the exact empty-parameter text.
    NlQueryPlan(NlQueryPlan) => Raw<medulla_query_types::nl_query::NlQueryPlanResult>;
    TxnUnifiedQuery(TxnUnifiedQuery) => Raw<Vec<medulla_types::unified_row::UnifiedRow>>;
    // A UQL statement's result inside a transaction (EH-434).
    #[cfg(feature = "query")]
    TxnUql(TxnUql) => Raw<medulla_query_types::wire::UqlResult>;
    Decide(Decide) => Raw<DecisionBatch>;
    // MEDULLA-DECISION-ENGINE-R802: ordered selection with reasons, and the skips.
    SelectWebPages(SelectWebPages) => Raw<medulla_decision_outcome_types::decision::web_fetch_selection::SelectWebPagesAnswer>;
    // Every op answers the request graph's edge indexes after it.
    EdgeIndex(EdgeIndex) => Raw<EdgeIndexStatusView>;
    EdgeSearch(EdgeSearch) => Raw<EdgeSearchView>;
}
