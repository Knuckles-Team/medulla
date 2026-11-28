//! Modules moved here from `medulla_server::split_tests` (hard cutover, no re-exports).

#[cfg(all(
    test,
    unix,
    feature = "redb",
    feature = "blob",
    feature = "shacl",
    feature = "security"
))]
mod agent_run_served_tests;
#[cfg(all(
    test,
    unix,
    feature = "query",
    feature = "redb",
    feature = "ann-redb",
    feature = "blob",
    feature = "shacl",
    feature = "security"
))]
mod bookmark_served_tests;
#[cfg(all(
    test,
    unix,
    feature = "redb",
    feature = "blob",
    feature = "shacl",
    feature = "security"
))]
mod build_slot_lease_served_tests;
#[cfg(all(test, feature = "query"))]
mod business_ops_served_tests;
mod cluster_hierarchy_served_tests;
#[cfg(all(test, feature = "datascience", feature = "query"))]
mod clustering_kernel;
#[cfg(all(
    test,
    unix,
    feature = "redb",
    feature = "blob",
    feature = "shacl",
    feature = "security"
))]
mod deploy_record_served_tests;
#[cfg(all(
    test,
    unix,
    feature = "finance",
    feature = "redb",
    feature = "blob",
    feature = "shacl",
    feature = "security"
))]
mod finance_performance_served_tests;
mod graph_algorithm_scope_tests;
mod harness_edit_served_tests;
#[cfg(all(test, feature = "query", feature = "streaming"))]
mod incremental_views;
#[cfg(all(
    test,
    unix,
    feature = "redb",
    feature = "blob",
    feature = "shacl",
    feature = "security"
))]
mod ingest_materialize_service_level_tests;
#[cfg(all(test, feature = "promql"))]
mod medulla_server_obs_telemetry_snapshot_tests;
#[cfg(all(test, feature = "query"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_wire_wire_tests;
mod neighborhood_tests;
#[cfg(all(test, unix, feature = "redb", feature = "security"))]
mod observability;
#[cfg(all(
    test,
    unix,
    feature = "redb",
    feature = "blob",
    feature = "shacl",
    feature = "security",
    feature = "query"
))]
mod stored_query_workspace_tests;
mod tenant_separation_tests;
#[cfg(test)]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
#[cfg(feature = "query")]
mod wired_edge_index;
