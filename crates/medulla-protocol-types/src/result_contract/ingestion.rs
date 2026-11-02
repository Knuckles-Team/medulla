//! Declared results of the `ingestion` contract domain.

#[cfg(feature = "modality-serving")]
use super::Dynamic;
use medulla_source_types::{source_ingestion::SourceIngestStatus as SourceIngestStatusBody, source_ingestion::SourceIngestionReceipt};
use medulla_index_types::{
    semantic_index::SemanticBinding, semantic_index::SemanticBindingPage,
    semantic_index::SemanticIndexResponse, semantic_index::SemanticSqlSourceManifest,
    semantic_index::SemanticStageIntent, semantic_index::SemanticStageLeasePage,
    semantic_index::SemanticStageTransition,
};
#[cfg(feature = "asr-native")]
use medulla_types::asr_wire::AsrTranscription;
use medulla_types::telemetry_derive::TelemetryDeriveReceipt;
use medulla_types::{
    ingestion_wire::DiscoverHit, ingestion_wire::IndexResult, ingestion_wire::ParseResult,
    ingestion_wire::ScreenObservationResult,
};
#[cfg(feature = "quantum")]
use medulla_types::{
    quantum::QuantumExpectationResult, quantum::QuantumQaoaResult, quantum::QuantumRankResult,
};
#[cfg(feature = "modality-serving")]
mod modality;
mod semantic;
#[cfg(feature = "viz")]
mod viz;

#[cfg(feature = "modality-serving")]
pub use modality::{
    ServedModalityApplyDisposition, ServedModalityApplyOutcome,
    ServedModalityAuthority as ServedModalityAuthorityResult,
    ServedModalityCapabilities as ServedModalityCapabilitiesResult, ServedModalityClassification,
    ServedModalityEvent, ServedModalityEventKind, ServedModalityStats as ServedModalityStatsResult,
    ServedModalityTombstoneCollection,
};
pub use semantic::{
    SemanticMutationReceipt, SemanticOutboxStatus, SemanticSqlSourcePageAdmission,
    SemanticSqlSourceReconciliationAdmission,
};
#[cfg(feature = "viz")]
pub use viz::{
    VizCapabilityEntry, VizCapabilityMatrix as VizCapabilityMatrixResult, VizPayloadRef,
    VizProvenanceRecord, VizRenderResponse, VizViewResult,
};

method_results! {
    visit_ingestion;
    ParseFile(ParseFile) => Json<ParseResult>;
    // One parse result per input file, in input order.
    ParseFiles(ParseFiles) => Json<Vec<ParseResult>>;
    IndexRepository(IndexRepository) => Json<IndexResult>;
    ObserveScreen(ObserveScreen) => Json<ScreenObservationResult>;
    AddEmbedding(AddEmbedding) => Text<String>;
    SourceIngest(SourceIngest) => Raw<SourceIngestionReceipt>;
    SourceIngestStatus(SourceIngestStatus) => Raw<SourceIngestStatusBody>;
    // MEDULLA-UNIFIED-DATA-PLANE-R651.7: chunk, vector and provenance counts.
    EnrichDocument(EnrichDocument) => Raw<medulla_source_types::source_ingestion::web_document::EnrichDocumentReceipt>;
    // EH-408 / EH-409: counts plus the id of every fact node written.
    TelemetryDerive(TelemetryDerive) => Raw<TelemetryDeriveReceipt>;
    // `(node_id, weighted_similarity)` pairs, best first.
    SemanticSearch(SemanticSearch) => Raw<Vec<(String, f32)>>;
    Discover(Discover) => Json<Vec<DiscoverHit>>;
    SemanticIndexAdmitBinding(SemanticIndex / "admit_binding") => Raw<SemanticMutationReceipt>;
    SemanticIndexRefreshBinding(SemanticIndex / "refresh_binding") => Raw<SemanticMutationReceipt>;
    SemanticIndexTransitionBinding(SemanticIndex / "transition_binding") => Raw<SemanticMutationReceipt>;
    SemanticIndexDropBinding(SemanticIndex / "drop_binding") => Raw<SemanticMutationReceipt>;
    SemanticIndexAdmitSourceRecord(SemanticIndex / "admit_source_record") => Raw<SemanticMutationReceipt>;
    SemanticIndexAdmitSourcePage(SemanticIndex / "admit_source_page") => Raw<SemanticSqlSourcePageAdmission>;
    SemanticIndexAdmitSourceReconcile(SemanticIndex / "admit_source_reconcile") => Raw<SemanticSqlSourceReconciliationAdmission>;
    SemanticIndexAdmitSourceReplacement(SemanticIndex / "admit_source_replacement") => Raw<SemanticMutationReceipt>;
    SemanticIndexSubscribeStageConsumer(SemanticIndex / "subscribe_stage_consumer") => Raw<bool>;
    SemanticIndexClaimStageLeases(SemanticIndex / "claim_stage_leases") => Raw<SemanticStageLeasePage>;
    SemanticIndexValidateStageLease(SemanticIndex / "validate_stage_lease") => Raw<SemanticStageIntent>;
    SemanticIndexStageStatus(SemanticIndex / "stage_status") => Raw<SemanticOutboxStatus>;
    SemanticIndexCompleteStage(SemanticIndex / "complete_stage") => Raw<SemanticMutationReceipt>;
    SemanticIndexCompleteGenerationStage(SemanticIndex / "complete_generation_stage") => Raw<SemanticMutationReceipt>;
    SemanticIndexCompleteSqlSourceStage(SemanticIndex / "complete_sql_source_stage") => Raw<SemanticMutationReceipt>;
    SemanticIndexReplayCompletedSqlSourceStage(SemanticIndex / "replay_completed_sql_source_stage") => Raw<Option<(SemanticStageTransition, SemanticMutationReceipt)>>;
    SemanticIndexReleaseStageLease(SemanticIndex / "release_stage_lease") => Raw<bool>;
    SemanticIndexGetBindingRequest(SemanticIndex / "get_binding_request") => Raw<SemanticIndexResponse>;
    SemanticIndexBinding(SemanticIndex / "binding") => Raw<Option<SemanticBinding>>;
    SemanticIndexSqlSourceManifest(SemanticIndex / "sql_source_manifest") => Raw<Option<SemanticSqlSourceManifest>>;
    SemanticIndexListBindings(SemanticIndex / "list_bindings") => Raw<SemanticBindingPage>;
    SemanticIndexLiveGeneration(SemanticIndex / "live_generation") => Raw<Option<u64>>;
    #[cfg(feature = "modality-serving")]
    ServedModalityAuthority(ServedModality / "authority") => Raw<ServedModalityAuthorityResult>;
    #[cfg(feature = "modality-serving")]
    ServedModalityIngest(ServedModality / "ingest") => Raw<ServedModalityApplyOutcome>;
    #[cfg(feature = "modality-serving")]
    ServedModalityIngestStream(ServedModality / "ingest_stream") => Raw<Vec<ServedModalityApplyOutcome>>;
    #[cfg(feature = "modality-serving")]
    ServedModalityQuery(ServedModality / "query") => Raw<Dynamic> dynamic RequestedModality;
    #[cfg(feature = "modality-serving")]
    ServedModalityNativeQuery(ServedModality / "native_query") => Raw<Dynamic> dynamic RequestedModality;
    #[cfg(feature = "modality-serving")]
    ServedModalityDelete(ServedModality / "delete") => Raw<ServedModalityApplyOutcome>;
    #[cfg(feature = "modality-serving")]
    ServedModalityMoveToCold(ServedModality / "move_to_cold") => Raw<ServedModalityApplyOutcome>;
    #[cfg(feature = "modality-serving")]
    ServedModalityRestore(ServedModality / "restore") => Raw<ServedModalityApplyOutcome>;
    #[cfg(feature = "modality-serving")]
    ServedModalityEvents(ServedModality / "events") => Raw<Vec<ServedModalityEvent>>;
    #[cfg(feature = "modality-serving")]
    ServedModalityStats(ServedModality / "stats") => Raw<ServedModalityStatsResult>;
    #[cfg(feature = "modality-serving")]
    ServedModalityCollectTombstones(ServedModality / "collect_tombstones") => Json<ServedModalityTombstoneCollection>;
    #[cfg(feature = "modality-serving")]
    ServedModalityCapabilities(ServedModality / "capabilities") => Json<ServedModalityCapabilitiesResult>;
    #[cfg(feature = "viz")]
    VizRender(Viz / "Render") => Raw<VizRenderResponse>;
    #[cfg(feature = "viz")]
    VizCapabilityMatrix(Viz / "CapabilityMatrix") => Raw<VizCapabilityMatrixResult>;
    #[cfg(feature = "viz")]
    VizRenderProvenance(Viz / "RenderProvenance") => Raw<Option<VizProvenanceRecord>>;
    #[cfg(feature = "quantum")]
    QuantumRank(Quantum / "rank") => Json<QuantumRankResult>;
    #[cfg(feature = "quantum")]
    QuantumOptimizeQaoa(Quantum / "optimize_qaoa") => Json<QuantumQaoaResult>;
    #[cfg(feature = "quantum")]
    QuantumExpectation(Quantum / "expectation") => Json<QuantumExpectationResult>;
    #[cfg(feature = "asr-native")]
    AsrTranscribeFile(Asr / "TranscribeFile") => Json<AsrTranscription>;
}
