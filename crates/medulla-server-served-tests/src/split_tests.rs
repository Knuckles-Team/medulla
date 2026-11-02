//! Modules moved here from `medulla_server::split_tests` (hard cutover, no re-exports).

#[cfg(all(
    test,
    unix,
    feature = "redb",
    feature = "tsdb",
    feature = "blob",
    feature = "shacl",
    feature = "security",
    feature = "matview"
))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_admin_boot_regression;
#[cfg(all(test, feature = "decide"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_decide_decide_guardrail_tests;
#[cfg(all(test, feature = "decide"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_decide_harness_config_tests;
#[cfg(test)]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_ingest_repository_index_tests;
#[cfg(all(test, feature = "query", feature = "modality-serving"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_ingest_web_ingest_tests;
#[cfg(all(test, feature = "jobs"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_jobs_wire_tests;
#[cfg(all(test, feature = "query", feature = "redb", feature = "ann-redb"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_query_dispatch_pipeline_tests;
#[cfg(all(test, feature = "query", feature = "ann-redb"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_query_sql_source_read_tests;
#[cfg(all(test, feature = "query"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_wire_wire_tests;
#[cfg(test)]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod wired_edge_index;
