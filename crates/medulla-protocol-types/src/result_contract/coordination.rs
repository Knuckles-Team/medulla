//! Declared results of the `coordination` contract domain.

#[cfg(feature = "jobs")]
mod jobs;
mod resources;
#[cfg(feature = "statechart")]
mod statechart;
#[cfg(feature = "statechart")]
mod statechart_instance_machine;
mod work_items;

#[cfg(feature = "jobs")]
pub use jobs::*;
pub use resources::*;
#[cfg(feature = "statechart")]
pub use statechart::*;
#[cfg(feature = "statechart")]
pub use statechart_instance_machine::*;
pub use work_items::*;

use medulla_agent_types::delegation::KgDelegateResult;
use medulla_decision_types::decision::statistical::retrieval::LearningRecorded;
use medulla_decision_types::decision::DecisionJobRecord;
use medulla_decision_types::{
    decision::statistical::log::DecisionLogCommitted,
    decision::statistical::log::DecisionLogCompacted, decision::statistical::log::DecisionLogEntry,
    decision::statistical::log::DecisionLogVerification,
    decision::statistical::log::OutcomeAggregate, decision::statistical::log::StoredEvaluation,
    decision::statistical::log::StoredResolution,
};
use medulla_mutation_types::sealed_record::SealedRecordRetirement;
use medulla_result_types::{
    epistemic_operations::ClaimWorkItemResult,
    epistemic_operations::DevelopmentLaneCleanupCompleteResult,
    epistemic_operations::DevelopmentLaneFinishResult,
    epistemic_operations::DevelopmentLaneObserveResult,
    epistemic_operations::DevelopmentLaneQueryResult,
    epistemic_operations::DevelopmentLaneQuotaUpdateResult,
    epistemic_operations::DevelopmentLaneRenewResult, epistemic_operations::DevelopmentLaneResult,
    epistemic_operations::DevelopmentLaneStatusResult,
    epistemic_operations::ResourceHostUpdateResult,
    epistemic_operations::ResourceReservationResult,
    epistemic_operations::ResourceReservationStatusResult,
};
use medulla_result_types::{
    epistemic_operations_ext::CasWorkItemMetadataResult,
    epistemic_operations_ext::WorkItemClaimCapabilityResult,
};
use medulla_work_types::{
    control_lease::ControlLeaseIssued, control_lease::ControlLeasePage,
    control_lease::ControlLeaseTransition, control_lease::ControlLeaseView,
};
use medulla_work_types::{
    native_control::CapacityAcquireResult, native_control::CapacityCellUpdateResult,
    native_control::CapacityMutationResult, native_control::CapacityReclaimResult,
    native_control::CapacityStatusResult, native_control::SubmitWorkItemResult,
    native_control::SubmitWorkItemsResult,
};
use medulla_work_types::{
    work_item_read::WorkItemOutcomeView, work_item_read::WorkItemPage, work_item_read::WorkItemView,
};
use medulla_work_types::{
    work_market::GapPage, work_market::GapSettled, work_market::GapTransitioned,
    work_market::GapUpserted, work_market::GapView, work_market::WorkOfferRecorded,
};

method_results! {
    visit_coordination;
    // The claimed node's id and its updated properties, or nil when nothing matched.
    ClaimNext(ClaimNext) => Raw<Option<(String, serde_json::Value)>>;
    Batch(Batch) => Raw<medulla_query_types::wire::BatchResult>;
    ClaimWorkItem(ClaimWorkItem) => Raw<ClaimWorkItemResult>;
    // MEDULLA-UNIFIED-DATA-PLANE-R700.3: the registered action name; the stored run.
    ActionRegister(ActionRegister) => Raw<String>;
    // MEDULLA-DURABLE-KERNEL-R072: the stored task id, the leased task or nil, the recorded outcome.
    TaskQueueEnqueue(TaskQueueEnqueue) => Raw<medulla_work_types::task_queue::TaskEnqueued>;
    // MEDULLA-INGEST-WORKER-R001.4.1: the receipt of one ingest enqueue.
    IngestEnqueue(IngestEnqueue) => Raw<medulla_source_types::ingest_job::IngestEnqueueReceipt>;
    // MEDULLA-INGEST-WORKER-R001.11: the depth and oldest age of the tenant's ingest queues.
    IngestQueueDepth(IngestQueueDepth) => Raw<medulla_source_types::ingest_job::IngestQueueDepthReport>;
    TaskQueueClaim(TaskQueueClaim) => Raw<Option<medulla_work_types::task_queue::ClaimedTask>>;
    TaskQueueRenew(TaskQueueRenew) => Raw<medulla_work_types::task_queue::TaskRenewed>;
    DeadLetterList(DeadLetterList) => Raw<medulla_ingestion_types::ingest_capabilities::DeadLetterListed>;
    DeadLetterRedrive(DeadLetterRedrive) => Raw<medulla_ingestion_types::ingest_capabilities::DeadLetterRedriven>;
    TaskQueueAck(TaskQueueAck) => Raw<medulla_work_types::task_queue::TaskAcked>;
    StartSourceDrain(StartSourceDrain) => Raw<medulla_ingestion_types::ingest_drain::DrainHandle>;
    SourceDrainStatus(SourceDrainStatus) => Raw<medulla_ingestion_types::ingest_drain::DrainStatus>;
    // MEDULLA-DURABLE-KERNEL-R076.2: the policy in force, and the policy just stored.
    GetSchedulerConfig(GetSchedulerConfig) => Raw<medulla_work_types::scheduler_config::SchedulerConfig>;
    SetSchedulerConfig(SetSchedulerConfig) => Raw<medulla_work_types::scheduler_config::SchedulerConfig>;
    // MEDULLA-DURABLE-KERNEL-R074.2: the worker record just registered, and the one just renewed.
    WorkerRegister(WorkerRegister) => Raw<medulla_work_types::worker_registry::WorkerRecord>;
    WorkerHeartbeat(WorkerHeartbeat) => Raw<medulla_work_types::worker_registry::WorkerRecord>;
    // MEDULLA-DURABLE-KERNEL-R101.3: the latest downtime tick's decision.
    DowntimeSelections(DowntimeSelections) => Raw<medulla_work_types::downtime::DowntimeSelections>;
    ActionSubmit(ActionSubmit) => Raw<medulla_work_types::action_spec::ActionRun>;
    // MEDULLA-UNIFIED-DATA-PLANE-R792: the registered actions with their parameter schemas.
    ActionList(ActionList) => Raw<medulla_work_types::action_spec::ActionList>;
    AcquireCapacity(AcquireCapacity) => Raw<CapacityAcquireResult>;
    RenewCapacity(RenewCapacity) => Raw<CapacityMutationResult>;
    ReleaseCapacity(ReleaseCapacity) => Raw<CapacityMutationResult>;
    ReclaimExpiredCapacity(ReclaimExpiredCapacity) => Raw<CapacityReclaimResult>;
    ReconcileCapacity(ReconcileCapacity) => Raw<CapacityStatusResult>;
    CapacityStatus(CapacityStatus) => Raw<CapacityStatusResult>;
    UpdateCapacityCell(UpdateCapacityCell) => Raw<CapacityCellUpdateResult>;
    ThrottleCapacityCell(ThrottleCapacityCell) => Raw<medulla_work_types::native_control::CapacityThrottleResult>;
    KgDelegate(KgDelegate) => Raw<KgDelegateResult>;
    SubmitWorkItem(SubmitWorkItem) => Raw<SubmitWorkItemResult>;
    SubmitWorkItems(SubmitWorkItems) => Raw<SubmitWorkItemsResult>;
    MintWorkItemClaimCapability(MintWorkItemClaimCapability) => Raw<WorkItemClaimCapabilityResult>;
    VerifyWorkItemClaimCapability(VerifyWorkItemClaimCapability) => Raw<WorkItemClaimCapabilityResult>;
    RenewWorkItemLease(RenewWorkItemLease) => Json<WorkItemLeaseRenewal>;
    CommitWorkItemResult(CommitWorkItemResult) => Json<WorkItemTransition<WorkItemCommitOutcome>>;
    CancelWorkItem(CancelWorkItem) => Json<WorkItemTransition<WorkItemCancelOutcome>>;
    DeferWorkItem(DeferWorkItem) => Json<WorkItemDeferral>;
    CasWorkItemMetadata(CasWorkItemMetadata) => Raw<CasWorkItemMetadataResult>;
    // EH-219 typed WorkItem reads. `null` when no WorkItem with this id is
    // visible to the verified tenant.
    PublishCurrentPlan(PublishCurrentPlan) => Raw<medulla_decision_outcome_types::current_plan::CurrentPlanPublished>;
    PublishRunCheckpoint(PublishRunCheckpoint) => Raw<medulla_work_types::run_checkpoint::RunCheckpointPublished>;
    GetRunCheckpoint(GetRunCheckpoint) => Raw<Option<medulla_work_types::run_checkpoint::RunCheckpointRead>>;
    GetCurrentPlan(GetCurrentPlan) => Raw<Option<medulla_decision_outcome_types::current_plan::CurrentPlanSnapshot>>;
    GetSessionUsage(GetSessionUsage) => Raw<medulla_work_types::run_usage::SessionUsage>;
    GetShardWriterWidth(GetShardWriterWidth) => Raw<medulla_work_types::worker_registry::ShardWriterWidth>;
    AdmitWorkerTask(AdmitWorkerTask) => Raw<medulla_work_types::worker_registry::WorkerAdmission>;
    GetRunWorkItem(GetRunWorkItem) => Raw<Option<medulla_work_types::work_item_read::RunWorkItemView>>;
    GetWorkItem(GetWorkItem) => Raw<Option<WorkItemView>>;
    ListWorkItemTransitions(ListWorkItemTransitions) => Raw<Option<medulla_work_types::work_item_read::WorkItemTransitionPage>>;
    // MEDULLA-DURABLE-KERNEL-R820.3 uncertain-effect protocol. Reads are
    // `null` when absent or not visible to the verified tenant (and session).
    MarkWorkItemEffectStarted(MarkWorkItemEffectStarted) => Raw<medulla_work_types::work_item_read::EffectWriteResult>;
    RecordEffectReceipt(RecordEffectReceipt) => Raw<medulla_work_types::work_item_read::EffectWriteResult>;
    SubmitAdmittedRequest(SubmitAdmittedRequest) => Raw<medulla_work_types::work_item_read::EffectWriteResult>;
    GetEffectReceipt(GetEffectReceipt) => Raw<Option<medulla_work_types::work_item_read::EffectReceipt>>;
    GetAdmittedRequest(GetAdmittedRequest) => Raw<Option<medulla_work_types::work_item_read::AdmittedRequestRecord>>;
    // MEDULLA-DECISION-ENGINE-R133.7.3: one authorized page of a stored recursive trace.
    TraceCritique(TraceCritique) => Raw<medulla_decision_types::decision::recursive_trace::TraceCritiquePage>;
    ListWorkItems(ListWorkItems) => Raw<WorkItemPage>;
    // graph-os MEDULLA-3: `null` when the WorkItem is not visible or has no bundle.
    GetWorkItemOutcome(GetWorkItemOutcome) => Raw<Option<WorkItemOutcomeView>>;
    // graph-os MEDULLA-2 native control leases. `null` when no lease with this id
    // is visible to the verified tenant.
    IssueHostStartGrant(IssueHostStartGrant) => Json<ControlLeaseIssued>;
    // MEDULLA-FEDERATED-QUERY-R085.2: the dedicated source-query approval issuer.
    ApproveSourceQuery(ApproveSourceQuery) => Json<medulla_work_types::source_query_approval::SourceQueryApprovalIssued>;
    IssueControlLease(IssueControlLease) => Json<ControlLeaseIssued>;
    TransitionControlLease(TransitionControlLease) => Json<ControlLeaseTransition>;
    GetControlLease(GetControlLease) => Raw<Option<ControlLeaseView>>;
    ListControlLeases(ListControlLeases) => Raw<ControlLeasePage>;
    // EH-348 work market. `GapGet` is `null` when no Gap with this id is
    // visible to the verified tenant.
    GapUpsert(GapUpsert) => Json<GapUpserted>;
    GapTransition(GapTransition) => Json<GapTransitioned>;
    GapSettle(GapSettle) => Json<GapSettled>;
    WorkOfferPut(WorkOfferPut) => Json<WorkOfferRecorded>;
    GapGet(GapGet) => Raw<Option<GapView>>;
    GapList(GapList) => Raw<GapPage>;
    // EH-558 sealed-record retirement (the owning op).
    RetireSealedRecord(RetireSealedRecord) => Json<SealedRecordRetirement>;
    ReserveWorkItemResources(ReserveWorkItemResources) => Raw<ResourceReservationResult>;
    ReleaseWorkItemResources(ReleaseWorkItemResources) => Raw<ResourceReservationResult>;
    ReclaimWorkItemResources(ReclaimWorkItemResources) => Raw<ResourceReservationResult>;
    QueryWorkItemReservation(QueryWorkItemReservation) => Raw<ResourceReservationResult>;
    ResourceReservationStatus(ResourceReservationStatus) => Raw<ResourceReservationStatusResult>;
    UpdateResourceHost(UpdateResourceHost) => Raw<ResourceHostUpdateResult>;
    ReserveDevelopmentLane(ReserveDevelopmentLane) => Raw<DevelopmentLaneResult>;
    RenewDevelopmentLane(RenewDevelopmentLane) => Raw<DevelopmentLaneRenewResult>;
    ObserveDevelopmentLane(ObserveDevelopmentLane) => Raw<DevelopmentLaneObserveResult>;
    FinishDevelopmentLane(FinishDevelopmentLane) => Raw<DevelopmentLaneFinishResult>;
    CleanupDevelopmentLane(CleanupDevelopmentLane) => Raw<DevelopmentLaneCleanupCompleteResult>;
    QueryDevelopmentLane(QueryDevelopmentLane) => Raw<DevelopmentLaneQueryResult>;
    DevelopmentLaneStatus(DevelopmentLaneStatus) => Raw<DevelopmentLaneStatusResult>;
    UpdateDevelopmentLaneQuota(UpdateDevelopmentLaneQuota) => Raw<DevelopmentLaneQuotaUpdateResult>;
    ResourceStatsPage(ResourceStatsPage) => Json<ResourceSnapshot>;
    #[cfg(feature = "jobs")]
    JobSubmit(AnalyticsJob / "Submit") => Json<AnalyticsJobRecord>;
    #[cfg(feature = "jobs")]
    JobStatus(AnalyticsJob / "Status") => Json<AnalyticsJobRecord>;
    #[cfg(feature = "jobs")]
    JobCancel(AnalyticsJob / "Cancel") => Json<AnalyticsJobRecord>;
    #[cfg(feature = "jobs")]
    JobResume(AnalyticsJob / "Resume") => Json<AnalyticsJobRecord>;
    // `null` when no eligible job is waiting.
    #[cfg(feature = "jobs")]
    JobWorkerClaim(AnalyticsJob / "WorkerClaim") => Json<Option<WorkerJobClaim>>;
    #[cfg(feature = "jobs")]
    JobWorkerRenew(AnalyticsJob / "WorkerRenew") => Json<JobWorkerLease>;
    #[cfg(feature = "jobs")]
    JobWorkerCheckpoint(AnalyticsJob / "WorkerCheckpoint") => Json<AnalyticsJobRecord>;
    #[cfg(feature = "jobs")]
    JobWorkerStage(AnalyticsJob / "WorkerStage") => Json<AnalyticsJobRecord>;
    #[cfg(feature = "jobs")]
    JobWorkerPublish(AnalyticsJob / "WorkerPublish") => Json<AnalyticsJobRecord>;
    #[cfg(feature = "jobs")]
    JobWorkerFail(AnalyticsJob / "WorkerFail") => Json<AnalyticsJobRecord>;
    #[cfg(feature = "jobs")]
    JobWorkerCancel(AnalyticsJob / "WorkerCancel") => Json<AnalyticsJobRecord>;
    #[cfg(feature = "jobs")]
    JobScheduleCreate(AnalyticsJob / "ScheduleCreate") => Json<ScheduleView>;
    #[cfg(feature = "jobs")]
    JobSchedulePause(AnalyticsJob / "SchedulePause") => Json<ScheduleView>;
    #[cfg(feature = "jobs")]
    JobScheduleResume(AnalyticsJob / "ScheduleResume") => Json<ScheduleView>;
    #[cfg(feature = "jobs")]
    JobScheduleGet(AnalyticsJob / "ScheduleGet") => Json<ScheduleView>;
    #[cfg(feature = "jobs")]
    JobScheduleUpdate(AnalyticsJob / "ScheduleUpdate") => Json<ScheduleView>;
    #[cfg(feature = "jobs")]
    JobScheduleDelete(AnalyticsJob / "ScheduleDelete") => Json<ScheduleRemoved>;
    #[cfg(feature = "jobs")]
    JobScheduleList(AnalyticsJob / "ScheduleList") => Json<ScheduleViewList>;
    #[cfg(feature = "jobs")]
    JobScheduleTicks(AnalyticsJob / "ScheduleTicks") => Json<ScheduleTickList>;
    #[cfg(feature = "jobs")]
    JobScheduleCalendar(AnalyticsJob / "ScheduleCalendar") => Json<ScheduleCalendar>;
    #[cfg(feature = "statechart")]
    StatechartDefine(Statechart / "Define") => Json<StatechartDefinitionId>;
    #[cfg(feature = "statechart")]
    StatechartInstantiate(Statechart / "Instantiate") => Json<StatechartInstance>;
    #[cfg(feature = "statechart")]
    StatechartSendEvent(Statechart / "SendEvent") => Json<StatechartEventOutcome>;
    #[cfg(feature = "statechart")]
    StatechartGetState(Statechart / "GetState") => Json<StatechartInstance>;
    #[cfg(feature = "statechart")]
    StatechartList(Statechart / "List") => Json<StatechartInstanceList>;
    DecisionFitSubmit(DecisionFit / "submit") => Raw<DecisionJobRecord>;
    DecisionFitStatus(DecisionFit / "status") => Raw<Option<DecisionJobRecord>>;
    DecisionEvalSubmit(DecisionEval / "submit") => Raw<DecisionJobRecord>;
    DecisionEvalStatus(DecisionEval / "status") => Raw<Option<DecisionJobRecord>>;
    DecisionEvalRetrievalRankingSubmit(DecisionEval / "retrieval_ranking_submit") => Raw<DecisionJobRecord>;
    DecisionEvalGroundedAnswerSubmit(DecisionEval / "grounded_answer_submit") => Raw<DecisionJobRecord>;
    DecisionEvalCertifyLocalSkill(DecisionEval / "certify_local_skill") => Raw<DecisionJobRecord>;
    DecisionEvalReceiptGet(DecisionEval / "receipt") => Raw<Option<medulla_decision_types::decision::DecisionEvalReceipt>>;
    DecisionEvalReceipts(DecisionEval / "receipts") => Raw<medulla_decision_types::decision::DecisionReceiptPage>;
    DecisionEvalTimeline(DecisionEval / "timeline") => Raw<medulla_decision_types::decision::DecisionReceiptTimelinePage>;
    DecisionEvalSkillEval(DecisionEval / "skill_eval") => Raw<medulla_decision_types::skill_eval::SkillEvalOutcome>;
    DecisionEvalSkillEvalCaseAdd(DecisionEval / "skill_eval_case_add") => Raw<medulla_decision_types::skill_eval::SkillEvalCaseAdded>;
    DecisionEvalSkillEvalReport(DecisionEval / "skill_eval_report") => Raw<medulla_decision_types::skill_eval::SkillEvalReportAnswer>;
    DecisionEvalSkillEvalCalibrate(DecisionEval / "skill_eval_calibrate") => Raw<medulla_decision_types::skill_eval::SkillEvalCalibrated>;
    DecisionEvalDelegationQuality(DecisionEval / "delegation_quality") => Raw<medulla_decision_types::decision::delegation_quality::DelegationQualityOutcome>;
    DecisionLogCommit(DecisionLog / "commit") => Raw<DecisionLogCommitted>;
    DecisionLogEvaluate(DecisionLog / "evaluate") => Raw<StoredEvaluation>;
    DecisionLogGet(DecisionLog / "get") => Raw<Option<DecisionLogEntry>>;
    DecisionLogAggregate(DecisionLog / "aggregate") => Raw<OutcomeAggregate>;
    DecisionLogCompact(DecisionLog / "compact") => Raw<DecisionLogCompacted>;
    DecisionLogVerify(DecisionLog / "verify") => Raw<DecisionLogVerification>;
    DecisionLogResolve(DecisionLog / "resolve") => Raw<StoredResolution>;
    DecisionLogLearn(DecisionLog / "learn") => Raw<LearningRecorded>;
    DecisionLogPromoteRule(DecisionLog / "promote_rule") => Raw<medulla_decision_types::decision::statistical::log::RulePromotionResult>;
    DecisionLogProposeRule(DecisionLog / "propose_rule") => Raw<medulla_agent_types::agent_guardrail::RuleVersion>;
    DecisionLogApproveRule(DecisionLog / "approve_rule") => Raw<medulla_agent_types::agent_guardrail::RuleVersion>;
    DecisionLogRevokeRule(DecisionLog / "revoke_rule") => Raw<medulla_agent_types::agent_guardrail::RuleVersion>;
    DecisionLogPolicyActivation(DecisionLog / "policy_activation") => Raw<medulla_types::policy_document::PolicyActivated>;
    DecisionLogWriteback(DecisionLog / "writeback") => Raw<medulla_decision_types::decision::statistical::writeback::WritebackOutcome>;
    DecisionLogRunRecord(DecisionLog / "run_record") => Raw<medulla_decision_types::decision::program_store::RunRecordStored>;
    DecisionLogCapabilityOutcome(DecisionLog / "capability_outcome") => Raw<medulla_decision_types::decision::capability_designation::CapabilityOutcomeApplied>;
    DecisionLogGenerationSpace(DecisionLog / "generation_space") => Raw<medulla_decision_types::decision::statistical::retrieval_generation::GenerationSpaceRead>;
    DecisionLogWritebackAudit(DecisionLog / "writeback_audit") => Raw<medulla_decision_types::decision::statistical::writeback::WritebackAuditRows>;
    DecisionLogHarnessAdoption(DecisionLog / "harness_adoption") => Raw<medulla_decision_types::decision::statistical::log::HarnessAdoptionStored>;
    DecisionLogHarnessLessons(DecisionLog / "harness_lessons") => Raw<medulla_decision_types::decision::statistical::log::HarnessLessonsFound>;
    DecisionLogPointer(DecisionLog / "pointer") => Raw<medulla_decision_outcome_types::decision::promotion::ChampionPointer>;
    DecisionLogLifecycleRecord(DecisionLog / "lifecycle_record") => Raw<medulla_decision_types::decision::statistical::lifecycle_record::LifecycleRecordStored>;
    DecisionLogLifecycleRecords(DecisionLog / "lifecycle_records") => Raw<medulla_decision_types::decision::statistical::lifecycle_record::LifecycleRecordsFound>;
    DecisionLogList(DecisionLog / "list") => Raw<medulla_decision_types::decision::statistical::log::DecisionLogList>;
    DecisionLogProvenance(DecisionLog / "provenance") => Raw<medulla_decision_types::decision::statistical::log::DecisionProvenance>;
    DecisionLogWhyNot(DecisionLog / "why_not") => Raw<medulla_decision_types::decision::statistical::log::DecisionWhyNot>;
    FeatureLedgerOpen(FeatureLedger / "open") => Raw<medulla_decision_types::decision::feature_match::FeatureLedgerView>;
    FeatureLedgerPlan(FeatureLedger / "plan") => Raw<medulla_decision_types::decision::feature_match::FeatureLedgerView>;
    FeatureLedgerClose(FeatureLedger / "close") => Raw<medulla_decision_types::decision::feature_match::FeatureLedgerView>;
    FeatureLedgerGet(FeatureLedger / "get") => Raw<medulla_decision_types::decision::feature_match::FeatureLedgerView>;
    BeliefRevise(BeliefRevise) => Raw<medulla_decision_types::belief_revision::BeliefPassResult>;
    PublishSkillPackage(PublishSkillPackage) => Raw<medulla_decision_types::decision::package_validation::PublicationReceipt>;
    // MEDULLA-DECISION-ENGINE-R702.3: a recorded correction, its nearest neighbours and a read by receipt.
    CorrectionRecordRun(CorrectionRecord / "record") => Raw<medulla_decision_types::correction::CorrectionRecorded>;
    CorrectionRecordNearest(CorrectionRecord / "nearest") => Raw<medulla_decision_types::correction::CorrectionNearest>;
    CorrectionRecordGet(CorrectionRecord / "get") => Raw<medulla_decision_types::correction::CorrectionRead>;
    // MEDULLA-DECISION-ENGINE-R702.8: the independent outcomes per rule candidate.
    CorrectionRecordOutcomes(CorrectionRecord / "outcomes") => Raw<medulla_decision_types::correction::CorrectionOutcomes>;
    // MEDULLA-DECISION-ENGINE-R701.3: failure-pattern ingest, mark-addressed and get.
    FailureIngestRun(FailureIngest / "ingest") => Raw<medulla_decision_types::failure_patterns::FailureIngestResult>;
    FailureMarkAddressed(FailureIngest / "mark_addressed") => Raw<medulla_decision_types::failure_patterns::GapTopic>;
    FailureGet(FailureIngest / "get") => Raw<medulla_decision_types::failure_patterns::GapTopic>;
    // MEDULLA-DECISION-ENGINE-R708.3: knowledge flag raise, resolve, get and list.
    KnowledgeFlagRaise(KnowledgeFlag / "raise") => Raw<medulla_decision_types::knowledge_flag::KnowledgeFlagReceipt>;
    KnowledgeFlagResolve(KnowledgeFlag / "resolve") => Raw<medulla_decision_types::knowledge_flag::KnowledgeFlagReceipt>;
    KnowledgeFlagGet(KnowledgeFlag / "get") => Raw<medulla_decision_types::knowledge_flag::KnowledgeFlagView>;
    KnowledgeFlagListFlags(KnowledgeFlag / "list") => Raw<medulla_decision_types::knowledge_flag::KnowledgeFlagList>;
    // MEDULLA-DECISION-ENGINE-R709.3: knowledge curation inputs, propose, status and metrics.
    KnowledgeCurationInputs(KnowledgeCuration / "inputs") => Raw<medulla_decision_types::knowledge_curation::CurationInputs>;
    KnowledgeCurationPropose(KnowledgeCuration / "propose") => Raw<medulla_decision_types::knowledge_curation::CurationAdmitted>;
    KnowledgeCurationStatus(KnowledgeCuration / "status") => Raw<medulla_decision_types::knowledge_curation::CurationStatus>;
    KnowledgeCurationMetrics(KnowledgeCuration / "metrics") => Raw<medulla_decision_types::knowledge_curation::LoopMetrics>;
    // MEDULLA-DECISION-ENGINE-R710.3: friction report, transition, group and get.
    FrictionReportFile(FrictionReport / "report") => Raw<medulla_decision_types::friction::FrictionFiled>;
    FrictionReportMove(FrictionReport / "transition") => Raw<medulla_decision_types::friction::FrictionMoved>;
    FrictionReportGroup(FrictionReport / "group") => Raw<medulla_decision_types::friction::FrictionGroups>;
    FrictionReportGet(FrictionReport / "get") => Raw<medulla_decision_types::friction::FrictionGroupView>;
    // MEDULLA-DECISION-ENGINE-R713.3: skill telemetry ingest, measures, rank, peers and traces.
    SkillTelemetryIngest(SkillTelemetry / "ingest") => Raw<medulla_decision_types::skill_telemetry::IngestReceipt>;
    SkillTelemetryMeasures(SkillTelemetry / "measures") => Raw<medulla_decision_types::skill_telemetry::SkillMeasures>;
    SkillTelemetryRank(SkillTelemetry / "rank") => Raw<medulla_decision_types::skill_telemetry::RankAnswer>;
    SkillTelemetryPeers(SkillTelemetry / "peers") => Raw<medulla_decision_types::skill_telemetry::PeersAnswer>;
    SkillTelemetryTraces(SkillTelemetry / "traces_by_eval") => Raw<medulla_decision_types::skill_telemetry::TracesAnswer>;
    // MEDULLA-DECISION-ENGINE-R801.8: the usage ledger append and price put.
    UsageAppend(Usage / "append") => Raw<medulla_decision_types::usage::UsageAppended>;
    UsagePutPrice(Usage / "put_price") => Raw<medulla_decision_types::usage::UsagePriceStored>;
    // MEDULLA-DECISION-ENGINE-R801.7: the tenant-scoped usage read.
    UsageRead(Usage / "read") => Raw<medulla_decision_types::usage::UsageReadResult>;
    ClaimsPropose(Claims / "propose") => Raw<medulla_decision_types::claims::ClaimRecord>;
    ClaimsAdvance(Claims / "advance") => Raw<medulla_decision_types::claims::ClaimRecord>;
    ClaimsRecordEvidence(Claims / "record_evidence") => Raw<medulla_decision_types::claims::ClaimRecord>;
    ClaimsCompileArtifact(Claims / "compile_artifact") => Raw<medulla_decision_types::claims::ResearchArtifact>;
    ClaimsGet(Claims / "get") => Raw<medulla_decision_types::claims::ClaimGetResult>;
    ClaimsProvenClaims(Claims / "proven_claims") => Raw<medulla_decision_types::claims::ProvenClaimsResult>;
    LoopsClaim(Loops / "claim") => Raw<medulla_decision_types::loops::LoopView>;
    LoopsRelease(Loops / "release") => Raw<medulla_decision_types::loops::LoopView>;
    LoopsBeacon(Loops / "beacon") => Raw<medulla_decision_types::loops::LoopView>;
    LoopsFileGap(Loops / "file_gap") => Raw<medulla_decision_types::loops::LoopView>;
    LoopsOffer(Loops / "offer") => Raw<medulla_decision_types::loops::LoopView>;
    LoopsSettle(Loops / "settle") => Raw<medulla_decision_types::loops::LoopView>;
    LoopsRecordSample(Loops / "record_sample") => Raw<medulla_decision_types::loops::LoopView>;
    LoopsRatchet(Loops / "ratchet") => Raw<medulla_decision_types::loops::LoopView>;
    LoopsMine(Loops / "mine") => Raw<medulla_decision_types::loops::LoopView>;
    LoopsGet(Loops / "get") => Raw<medulla_decision_types::loops::LoopView>;
    WorkflowRegistryRegister(WorkflowRegistry / "register") => Raw<medulla_work_types::orchestration::workflows::WorkflowRegistrationEntry>;
    WorkflowRegistryRead(WorkflowRegistry / "read") => Raw<medulla_work_types::orchestration::workflows::WorkflowRegistrationEntry>;
    // MEDULLA-CQ-ORCH-R700.3: the stored workflow version a process compile wrote.
    ProcessCompile(ProcessCompile) => Json<medulla_types::process_import::ProcessCompiled>;
    // MEDULLA-CQ-ORCH-R701.3: the stored import version and the exported workflow.
    ProcessModelImport(ProcessModelImport) => Json<medulla_types::process_import::ProcessModelImported>;
    // MEDULLA-CQ-ORCH-R702.9: the stored model version a document import wrote.
    ProcessDocumentImport(ProcessDocumentImport) => Json<medulla_types::process_exchange::ProcessDocumentImported>;
    // MEDULLA-CQ-ORCH-R702.10: the document a model or workflow export wrote.
    ProcessDocumentExport(ProcessDocumentExport) => Json<medulla_types::process_exchange::ProcessDocumentExported>;
    // MEDULLA-CQ-ORCH-R703.3: the deploy answers as a sync report whose one outbound draft is the queued proposal.
    ProcessModelDeploy(ProcessModelDeploy) => Json<medulla_source_types::source_sync::SyncReport>;
    // MEDULLA-ORCH-VOCAB-R009.101.1: the harness session index write answers what it changed.
    IndexHarnessSessionBatch(IndexHarnessSessionBatch) => Json<medulla_types::harness_session::IndexHarnessSessionBatchResult>;
    // MEDULLA-ORCH-VOCAB-R009.100: one page of the correlated run-trace export.
    RunTraceExport(RunTraceExport) => Json<medulla_types::run_trace_export::RunTraceExportPageV1>;
    RunMetrics(RunMetrics) => Json<medulla_types::run_metrics::RunMetricsAnswer>;
    ProcessModelExport(ProcessModelExport) => Json<medulla_types::process_import::ProcessModelExported>;
    HarnessEditCreate(HarnessEdit / "create") => Raw<medulla_work_types::harness_edit::HarnessEditRecord>;
    HarnessEditTransition(HarnessEdit / "transition") => Raw<medulla_work_types::harness_edit::HarnessEditRecord>;
    HarnessEditGet(HarnessEdit / "get") => Raw<medulla_work_types::harness_edit::HarnessEditRecord>;
    ObservationStatePut(ObservationState / "put") => Raw<medulla_work_types::observation_state::ObservationRecord>;
    ObservationStateGet(ObservationState / "get") => Raw<medulla_work_types::observation_state::ObservationRecord>;
    ParticipantPut(Participant / "put") => Raw<medulla_work_types::participant::Participant>;
    ParticipantAddMember(Participant / "add_member") => Raw<medulla_work_types::participant::Membership>;
    ParticipantPutScope(Participant / "put_scope") => Raw<medulla_work_types::memory_scope::MemoryScope>;
    ParticipantSetObservation(Participant / "set_observation") => Raw<medulla_work_types::memory_scope::ObservationConfig>;
    ParticipantCompleteDerivation(Participant / "complete_derivation") => Raw<medulla_decision_types::memory_events::DerivationCompleted>;
    ParticipantReadGet(ParticipantRead / "get") => Raw<medulla_work_types::participant::Participant>;
    ParticipantReadList(ParticipantRead / "list") => Raw<Vec<medulla_work_types::participant::Participant>>;
    ParticipantReadAsk(ParticipantRead / "ask") => Raw<medulla_decision_types::participant_memory::ParticipantAnswer>;
    ParticipantReadCard(ParticipantRead / "card") => Raw<medulla_decision_types::participant_memory::ParticipantCard>;
    ParticipantReadStatus(ParticipantRead / "status") => Raw<Vec<medulla_decision_types::participant_memory::DerivationStatus>>;
    ParticipantReadDeriveInput(ParticipantRead / "derive_input") => Raw<medulla_decision_types::participant_memory::DeriveInput>;
    AssistantProfilePutProfile(AssistantProfile / "put_profile") => Raw<medulla_identity_types::assistant_profile::AssistantProfile>;
    AssistantProfilePutDocumentSet(AssistantProfile / "put_document_set") => Raw<medulla_identity_types::assistant_profile::DocumentSet>;
    AssistantProfileGetProfile(AssistantProfile / "get_profile") => Raw<medulla_identity_types::assistant_profile::AssistantProfile>;
    AssistantProfileGetDocumentSet(AssistantProfile / "get_document_set") => Raw<medulla_identity_types::assistant_profile::DocumentSet>;
    AssistantProfileListProfiles(AssistantProfile / "list_profiles") => Raw<Vec<medulla_identity_types::assistant_profile::AssistantProfile>>;
    AssistantProfileListDocumentSets(AssistantProfile / "list_document_sets") => Raw<Vec<medulla_identity_types::assistant_profile::DocumentSet>>;
}
