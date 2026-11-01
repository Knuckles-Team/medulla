//! Declared results of the `coordination` contract domain.

#[cfg(feature = "jobs")]
mod jobs;
mod resources;
#[cfg(feature = "statechart")]
mod statechart;
mod work_items;

#[cfg(feature = "jobs")]
pub use jobs::*;
pub use resources::*;
#[cfg(feature = "statechart")]
pub use statechart::*;
pub use work_items::*;

use medulla_decision_types::decision::statistical::retrieval::LearningRecorded;
use medulla_decision_types::decision::DecisionJobRecord;
use medulla_decision_types::{
    decision::statistical::log::DecisionLogCommitted,
    decision::statistical::log::DecisionLogCompacted, decision::statistical::log::DecisionLogEntry,
    decision::statistical::log::DecisionLogVerification,
    decision::statistical::log::OutcomeAggregate, decision::statistical::log::StoredEvaluation,
    decision::statistical::log::StoredResolution,
};
use medulla_types::delegation::KgDelegateResult;
use medulla_types::sealed_record::SealedRecordRetirement;
use medulla_types::{
    control_lease::ControlLeaseIssued, control_lease::ControlLeasePage,
    control_lease::ControlLeaseTransition, control_lease::ControlLeaseView,
};
use medulla_types::{
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
use medulla_types::{
    epistemic_operations_ext::CasWorkItemMetadataResult,
    epistemic_operations_ext::WorkItemClaimCapabilityResult,
};
use medulla_types::{
    native_control::CapacityAcquireResult, native_control::CapacityCellUpdateResult,
    native_control::CapacityMutationResult, native_control::CapacityReclaimResult,
    native_control::CapacityStatusResult, native_control::SubmitWorkItemResult,
    native_control::SubmitWorkItemsResult,
};
use medulla_types::{
    work_item_read::WorkItemOutcomeView, work_item_read::WorkItemPage, work_item_read::WorkItemView,
};
use medulla_types::{
    work_market::GapPage, work_market::GapSettled, work_market::GapTransitioned,
    work_market::GapUpserted, work_market::GapView, work_market::WorkOfferRecorded,
};

method_results! {
    visit_coordination;
    // The claimed node's id and its updated properties, or nil when nothing matched.
    ClaimNext(ClaimNext) => Raw<Option<(String, serde_json::Value)>>;
    ClaimWorkItem(ClaimWorkItem) => Raw<ClaimWorkItemResult>;
    AcquireCapacity(AcquireCapacity) => Raw<CapacityAcquireResult>;
    RenewCapacity(RenewCapacity) => Raw<CapacityMutationResult>;
    ReleaseCapacity(ReleaseCapacity) => Raw<CapacityMutationResult>;
    ReclaimExpiredCapacity(ReclaimExpiredCapacity) => Raw<CapacityReclaimResult>;
    ReconcileCapacity(ReconcileCapacity) => Raw<CapacityStatusResult>;
    CapacityStatus(CapacityStatus) => Raw<CapacityStatusResult>;
    UpdateCapacityCell(UpdateCapacityCell) => Raw<CapacityCellUpdateResult>;
    ThrottleCapacityCell(ThrottleCapacityCell) => Raw<medulla_types::native_control::CapacityThrottleResult>;
    KgDelegate(KgDelegate) => Raw<KgDelegateResult>;
    SubmitWorkItem(SubmitWorkItem) => Raw<SubmitWorkItemResult>;
    SubmitWorkItems(SubmitWorkItems) => Raw<SubmitWorkItemsResult>;
    MintWorkItemClaimCapability(MintWorkItemClaimCapability) => Raw<WorkItemClaimCapabilityResult>;
    VerifyWorkItemClaimCapability(VerifyWorkItemClaimCapability) => Raw<WorkItemClaimCapabilityResult>;
    RenewWorkItemLease(RenewWorkItemLease) => Json<WorkItemLeaseRenewal>;
    CommitWorkItemResult(CommitWorkItemResult) => Json<WorkItemTransition<WorkItemCommitStatus>>;
    CancelWorkItem(CancelWorkItem) => Json<WorkItemTransition<WorkItemCancelStatus>>;
    DeferWorkItem(DeferWorkItem) => Json<WorkItemDeferral>;
    CasWorkItemMetadata(CasWorkItemMetadata) => Raw<CasWorkItemMetadataResult>;
    // EH-219 typed WorkItem reads. `null` when no WorkItem with this id is
    // visible to the verified tenant.
    PublishCurrentPlan(PublishCurrentPlan) => Raw<medulla_decision_types::current_plan::CurrentPlanPublished>;
    PublishRunCheckpoint(PublishRunCheckpoint) => Raw<medulla_types::run_checkpoint::RunCheckpointPublished>;
    GetRunCheckpoint(GetRunCheckpoint) => Raw<Option<medulla_types::run_checkpoint::RunCheckpointRead>>;
    GetCurrentPlan(GetCurrentPlan) => Raw<Option<medulla_decision_types::current_plan::CurrentPlanSnapshot>>;
    GetSessionUsage(GetSessionUsage) => Raw<medulla_types::run_usage::SessionUsage>;
    GetRunWorkItem(GetRunWorkItem) => Raw<Option<medulla_types::work_item_read::RunWorkItemView>>;
    GetWorkItem(GetWorkItem) => Raw<Option<WorkItemView>>;
    ListWorkItems(ListWorkItems) => Raw<WorkItemPage>;
    // graph-os MEDULLA-3: `null` when the WorkItem is not visible or has no bundle.
    GetWorkItemOutcome(GetWorkItemOutcome) => Raw<Option<WorkItemOutcomeView>>;
    // graph-os MEDULLA-2 native control leases. `null` when no lease with this id
    // is visible to the verified tenant.
    IssueHostStartGrant(IssueHostStartGrant) => Json<ControlLeaseIssued>;
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
    DecisionEvalReceiptGet(DecisionEval / "receipt") => Raw<Option<medulla_decision_types::decision::DecisionEvalReceipt>>;
    DecisionEvalReceipts(DecisionEval / "receipts") => Raw<medulla_decision_types::decision::DecisionReceiptPage>;
    DecisionEvalTimeline(DecisionEval / "timeline") => Raw<medulla_decision_types::decision::DecisionReceiptTimelinePage>;
    DecisionLogCommit(DecisionLog / "commit") => Raw<DecisionLogCommitted>;
    DecisionLogEvaluate(DecisionLog / "evaluate") => Raw<StoredEvaluation>;
    DecisionLogGet(DecisionLog / "get") => Raw<Option<DecisionLogEntry>>;
    DecisionLogAggregate(DecisionLog / "aggregate") => Raw<OutcomeAggregate>;
    DecisionLogCompact(DecisionLog / "compact") => Raw<DecisionLogCompacted>;
    DecisionLogVerify(DecisionLog / "verify") => Raw<DecisionLogVerification>;
    DecisionLogResolve(DecisionLog / "resolve") => Raw<StoredResolution>;
    DecisionLogLearn(DecisionLog / "learn") => Raw<LearningRecorded>;
}
