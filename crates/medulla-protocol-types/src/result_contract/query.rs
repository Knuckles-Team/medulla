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
use medulla_result_types::epistemic_operations::EvidenceBundle;
#[cfg(feature = "knowledge-batch")]
use medulla_types::knowledge_stream::KnowledgeStreamBatch;
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
    // `[node id, score | nil]` rows of the cross-modal plan.
    UnifiedQuery(UnifiedQuery) => Raw<Vec<(String, Option<f32>)>>;
    // MEDULLA-REPO-INGEST-R006.3: one page of a revision-bound code map.
    UnifiedQueryCodeMap(UnifiedQuery / "code_map") => Raw<medulla_types::code_map::CodeMapResult>;
    LiveArtifactPut(LiveArtifactPut) => Json<medulla_source_types::live_artifact::LiveArtifact>;
    LiveArtifactGet(LiveArtifactGet) => Json<medulla_source_types::live_artifact::LiveArtifactView>;
    LiveArtifactRefresh(LiveArtifactRefresh) => Json<medulla_source_types::live_artifact::RefreshEntry>;
    // MEDULLA-FEDERATED-QUERY-R708.3: the published record, and one call's rows with their freshness.
    // MEDULLA-FEDERATED-QUERY-R039.2.5: the receipt and vector of one encoded document, and a search over them.
    EncodeDocument(EncodeDocument) => Json<medulla_types::retrieval::EncodedDocument>;
    EncodedDocumentSearch(EncodedDocumentSearch) => Json<medulla_types::retrieval::EncodedSearchResult>;
    StoredQueryPut(StoredQueryPut) => Json<medulla_source_types::stored_query::StoredQuery>;
    ApproveEntityLink(ApproveEntityLink) => Json<medulla_types::entity_link_proposal::EntityLinkApproval>;
    StoredQueryCall(StoredQueryCall) => Json<medulla_source_types::stored_query::StoredCall>;
    ArgumentMapPut(ArgumentMapPut) => Json<medulla_result_types::argument_map::ArgumentMap>;
    ArgumentMapGet(ArgumentMapGet) => Json<medulla_result_types::argument_map::ArgumentMap>;
    ArgumentMapEvaluate(ArgumentMapEvaluate) => Json<medulla_result_types::argument_map::ExtensionResult>;
    #[cfg(feature = "federation")]
    QueryForeignColumns(QueryForeignColumns) => Raw<Vec<medulla_types::wire::ForeignColumnRow>>;
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
    Uql(Uql) => Raw<medulla_types::wire::UqlResult>;
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
    ContextCompile(ContextCompile) => Raw<medulla_decision_types::decision::context_bundle::ContextBundle>;
    ColumnLineage(ColumnLineage) => Raw<medulla_catalog_types::lineage::LineageResult>;
    // MEDULLA-CQ-XSRC-R008.4: the report of one cross-source question.
    CrossSourceQuestion(CrossSourceQuestion) => Raw<medulla_source_types::cross_source_report::CrossSourceReport>;
    // MEDULLA-UNIFIED-DATA-PLANE-R764.8: frequent users and usage suggestions of one asset.
    CatalogUsage(CatalogUsage) => Raw<medulla_types::asset_usage::CatalogUsageAnswer>;
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
    RankByProvenance(RankByProvenance) => Raw<RankByProvenanceResult>;
    NlQuery(NlQuery) => Raw<Vec<(String, Option<f32>)>>;
    // The executed UQL text alongside the same `[node id, score | nil]` rows as `NlQuery`.
    NlQueryExplain(NlQueryExplain) => Raw<medulla_query_types::nl_query::NlQueryExplainResult>;
    // The unexecuted UQL candidate and its pins for the exact empty-parameter text.
    NlQueryPlan(NlQueryPlan) => Raw<medulla_query_types::nl_query::NlQueryPlanResult>;
    TxnUnifiedQuery(TxnUnifiedQuery) => Raw<Vec<(String, Option<f32>)>>;
    // A UQL statement's result inside a transaction (EH-434).
    #[cfg(feature = "query")]
    TxnUql(TxnUql) => Raw<medulla_types::wire::UqlResult>;
    Decide(Decide) => Raw<DecisionBatch>;
    // MEDULLA-DECISION-ENGINE-R802: ordered selection with reasons, and the skips.
    SelectWebPages(SelectWebPages) => Raw<medulla_decision_outcome_types::decision::web_fetch_selection::SelectWebPagesAnswer>;
    // Every op answers the request graph's edge indexes after it.
    EdgeIndex(EdgeIndex) => Raw<EdgeIndexStatusView>;
    EdgeSearch(EdgeSearch) => Raw<EdgeSearchView>;
}
