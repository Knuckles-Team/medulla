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
use medulla_types::epistemic_operations::EvidenceBundle;
#[cfg(feature = "knowledge-batch")]
use medulla_types::knowledge_stream::KnowledgeStreamBatch;
use medulla_types::types::ContextView;
use medulla_types::{managed_index::EdgeIndexStatusView, managed_index::EdgeSearchView};

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
    #[cfg(feature = "federation")]
    QueryForeignColumns(QueryForeignColumns) => Raw<Vec<medulla_types::wire::ForeignColumnRow>>;
    #[cfg(feature = "federation")]
    ForeignReadOpen(ForeignReadOpen) => Raw<medulla_types::foreign_read::ForeignReadOpenResult>;
    #[cfg(feature = "federation")]
    ForeignReadChunk(ForeignReadChunk) => Raw<medulla_types::foreign_read::ForeignReadChunkResult>;
    #[cfg(feature = "federation")]
    ForeignReadComplete(ForeignReadComplete) => Raw<medulla_types::foreign_read::ForeignReadCompleteResult>;
    #[cfg(feature = "federation")]
    ForeignReadAbort(ForeignReadAbort) => Raw<medulla_types::foreign_read::ForeignReadAbortResult>;
    // A UQL statement's rows (with named score channels), EXPLAIN report or PROFILE.
    #[cfg(feature = "query")]
    Uql(Uql) => Raw<medulla_types::wire::UqlResult>;
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
    NlQueryExplain(NlQueryExplain) => Raw<medulla_types::nl_query::NlQueryExplainResult>;
    // The unexecuted UQL candidate and its pins for the exact empty-parameter text.
    NlQueryPlan(NlQueryPlan) => Raw<medulla_types::nl_query::NlQueryPlanResult>;
    TxnUnifiedQuery(TxnUnifiedQuery) => Raw<Vec<(String, Option<f32>)>>;
    // A UQL statement's result inside a transaction (EH-434).
    #[cfg(feature = "query")]
    TxnUql(TxnUql) => Raw<medulla_types::wire::UqlResult>;
    Decide(Decide) => Raw<DecisionBatch>;
    // MEDULLA-DECISION-ENGINE-R802: ordered selection with reasons, and the skips.
    SelectWebPages(SelectWebPages) => Raw<medulla_decision_types::decision::web_fetch_selection::SelectWebPagesAnswer>;
    // Every op answers the request graph's edge indexes after it.
    EdgeIndex(EdgeIndex) => Raw<EdgeIndexStatusView>;
    EdgeSearch(EdgeSearch) => Raw<EdgeSearchView>;
}
