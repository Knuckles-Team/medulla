//! Declared results of the `cluster` contract domain.

mod lifecycle;
mod registry;
mod sharding;
mod topology;

pub use lifecycle::*;
use medulla_result_types::compute_result::distributed::DistResult;
pub use registry::*;
pub use topology::*;

use super::Dynamic;

method_results! {
    visit_cluster;
    CreateGraph(CreateGraph) => Json<GraphCreated>;
    DeleteGraph(DeleteGraph) => Json<GraphDeleted>;
    CreateBranch(CreateBranch) => Json<medulla_mutation_types::branch::Branch>;
    BranchWrite(BranchWrite) => Json<medulla_mutation_types::branch::BranchWriteReceipt>;
    BranchMerge(BranchMerge) => Json<medulla_mutation_types::branch::BranchMergeReceipt>;
    BranchDiff(BranchDiff) => Json<medulla_mutation_types::versioning::KgDiff>;
    BranchReadAt(BranchReadAt) => Json<medulla_mutation_types::versioning::BranchSnapshot>;
    ReadTree(ReadTree) => Json<medulla_source_types::source_changeset::ReadTreeReceipt>;
    ReadPathHistory(ReadPathHistory) => Json<medulla_source_types::source_changeset::ReadPathHistoryReceipt>;
    SetPathGrant(SetPathGrant) => Json<medulla_source_types::source_path_grant::PathGrant>;
    ListPathGrants(ListPathGrants) => Json<medulla_source_types::source_path_grant::PathGrantList>;
    BisectStep(BisectStep) => Json<medulla_source_types::source_bisect::BisectStepReceipt>;
    ReadBlame(ReadBlame) => Json<medulla_source_types::source_changeset::ReadBlameReceipt>;
    SubmitChangeset(SubmitChangeset) => Json<medulla_source_types::source_changeset::SubmitChangesetReceipt>;
    RevertChangeset(RevertChangeset) => Json<medulla_source_types::source_changeset::SubmitChangesetReceipt>;
    SubmitSymbolEdit(SubmitSymbolEdit) => Json<medulla_source_types::source_changeset::SubmitChangesetReceipt>;
    ImportGitHistory(ImportGitHistory) => Json<medulla_source_types::source_changeset::ImportGitHistoryReceipt>;
    // MEDULLA-DURABLE-KERNEL-R877.2: the stored bundle, and what an import wrote.
    BundleExport(BundleExport) => Json<medulla_types::knowledge_bundle::BundleExportReceipt>;
    BundleImport(BundleImport) => Json<medulla_types::knowledge_bundle::BundleImportReceipt>;
    GitExportPack(GitExportPack) => Json<medulla_source_types::source_changeset::GitExportPackReceipt>;
    GitRefsList(GitRefsList) => Json<Vec<medulla_source_types::source_changeset::GitRef>>;
    ListGraphs(ListGraphs) => Json<Vec<GraphListing>>;
    Reshard(Reshard) => Json<ShardReshardReport>;
    CatalogAssign(CatalogAssign) => Bool<bool>;
    CatalogReassign(CatalogReassign) => Bool<bool>;
    CatalogRemove(CatalogRemove) => Bool<bool>;
    CatalogList(CatalogList) => Json<CatalogListing>;
    RebalancePlan(RebalancePlan) => Json<RebalancePlanReport>;
    RebalanceExecute(RebalanceExecute) => Json<RebalanceExecution>;
    PlacementRoute(PlacementRoute) => Raw<PlacementRouteWire>;
    RaftAddLearner(RaftAddLearner) => Bool<bool>;
    RaftChangeMembership(RaftChangeMembership) => Bool<bool>;
    ClusterMembers(ClusterMembers) => Json<ClusterDiscoverySnapshot>;
    // The acknowledgement of the `__commons__` server-row write it performs.
    RegisterServer(RegisterServer) => Text<String>;
    ListRegisteredServers(ListRegisteredServers) => Raw<RegisteredServerListPage>;
    FleetCatalogRecordDiscovery(FleetCatalog / "record_discovery") => Raw<medulla_catalog_types::fleet_catalog::FleetWriteReceipt>;
    FleetCatalogSetOverride(FleetCatalog / "set_override") => Raw<medulla_catalog_types::fleet_catalog::FleetWriteReceipt>;
    FleetCatalogClearOverride(FleetCatalog / "clear_override") => Raw<medulla_catalog_types::fleet_catalog::FleetWriteReceipt>;
    FleetCatalogList(FleetCatalog / "list") => Raw<medulla_catalog_types::fleet_catalog::FleetCatalogPage>;
    FleetCatalogLookupRows(FleetCatalog / "lookup") => Raw<medulla_catalog_types::fleet_catalog::FleetCatalogLookup>;
    PlacementAssign(PlacementAdmin / "assign") => Json<PlacementEpoch>;
    PlacementMove(PlacementAdmin / "move") => Json<PlacementMoveResult>;
    PlacementAbortMove(PlacementAdmin / "abort_move") => Bool<bool>;
    Ping(Ping) => Text<String>;
    EngineCapabilities(EngineCapabilities) => Raw<medulla_catalog_types::engine_capabilities::EngineCapabilities>;
    Health(Health) => Json<HealthReport>;
    IngestWorkerStatus(IngestWorkerStatus) => Json<medulla_types::ingest_lanes::IngestWorkerStatusReport>;
    Shutdown(Shutdown) => Text<String>;
    CancelRequest(CancelRequest) => Bool<bool>;
    RegisterForeignSource(RegisterForeignSource) => Text<String>;
    RegisterGraphView(RegisterGraphView) => Text<String>;
    ListForeignSources(ListForeignSources) => Json<Vec<medulla_catalog_types::foreign_source_catalog::ForeignSourceView>>;
    GetForeignSource(GetForeignSource) => Json<medulla_catalog_types::foreign_source_catalog::ForeignSourceView>;
    ProbeForeignSource(ProbeForeignSource) => Json<medulla_catalog_types::foreign_source_catalog::ForeignSourceProbe>;
    ShareForeignSource(ShareForeignSource) => Text<String>;
    UnshareForeignSource(UnshareForeignSource) => Text<String>;
    ForeignSourceDetach(ForeignSourceDetach) => Text<String>;
    ForeignSourceParity(ForeignSourceParity) => Json<medulla_catalog_types::foreign_source_catalog::ForeignSourceParityAnswer>;
    RegisterUdf(RegisterUdf) => Text<String>;
    CreateMatView(CreateMatView) => Count<u64>;
    #[cfg(feature = "compute-dist")]
    GetMatView(GetMatView) => Raw<DistResult>;
    RefreshMatView(RefreshMatView) => Count<u64>;
    PlanMatViewDefine(PlanMatViewDefine) => Count<u64>;
    // Rows of the view's caller-authored plan, served from the result cache or the
    // incremental circuit as the MessagePack they were materialized to.
    PlanMatViewGet(PlanMatViewGet) => Raw<Dynamic> dynamic QueryRows;
    PlanMatViewRefresh(PlanMatViewRefresh) => Count<u64>;
    PlanMatViewDrop(PlanMatViewDrop) => Bool<bool>;
}
