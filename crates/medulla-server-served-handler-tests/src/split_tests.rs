//! The served tests of the handler crates: every module is named for the handler crate it exercises.

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
#[cfg(all(
    test,
    unix,
    feature = "decide",
    feature = "redb",
    feature = "blob",
    feature = "shacl",
    feature = "security"
))]
mod medulla_h_decide_capability_pack_feed_tests;
#[cfg(all(test, feature = "decide", feature = "redb", feature = "security"))]
mod medulla_h_decide_correction_tests;
#[cfg(all(test, feature = "decide"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_decide_decide_guardrail_tests;
#[cfg(all(test, feature = "decide"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_decide_decision_reads_tests;
#[cfg(all(test, feature = "decide", feature = "redb", feature = "security"))]
mod medulla_h_decide_failure_ingest_tests;
#[cfg(all(test, feature = "decide", feature = "federation", feature = "redb"))]
mod medulla_h_decide_freshness_search_port_tests;
#[cfg(all(
    test,
    unix,
    feature = "decide",
    feature = "redb",
    feature = "blob",
    feature = "shacl",
    feature = "security"
))]
mod medulla_h_decide_harness_config_pack_tests;
mod medulla_h_decide_harness_config_tests;
#[cfg(all(test, feature = "decide", feature = "redb", feature = "security"))]
mod medulla_h_decide_policy_activation_tests;
#[cfg(all(test, feature = "decide", feature = "redb", feature = "blob"))]
mod medulla_h_decide_policy_evolution_tests;
#[cfg(test)]
mod medulla_h_graph_document_set_tests;
#[cfg(test)]
mod medulla_h_graph_memory_media_tests;
#[cfg(test)]
mod medulla_h_graph_participant_read_tests;
#[cfg(test)]
mod medulla_h_graph_visual_observation_tests;
#[cfg(all(test, feature = "redb", feature = "blob"))]
mod medulla_h_ingest_action_record_tests;
#[cfg(test)]
mod medulla_h_ingest_media_tag_tests;
#[cfg(all(test, feature = "redb"))]
mod medulla_h_ingest_meeting_transcript_tests;
#[cfg(all(
    test,
    unix,
    feature = "redb",
    feature = "blob",
    feature = "shacl",
    feature = "security"
))]
mod medulla_h_ingest_paper_tests;
#[cfg(test)]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_ingest_repository_index_tests;
#[cfg(test)]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_ingest_transcript_clock_tests;
#[cfg(all(test, feature = "query", feature = "modality-serving"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_ingest_web_ingest_tests;
#[cfg(all(test, unix, feature = "jobs", feature = "redb", feature = "security"))]
mod medulla_h_jobs_session_usage_tests;
#[cfg(all(test, feature = "jobs"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_jobs_wire_tests;
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
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_query_column_vector_tests;
#[cfg(all(test, feature = "query", feature = "redb", feature = "ann-redb"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_query_dispatch_pipeline_tests;
#[cfg(all(
    test,
    unix,
    feature = "redb",
    feature = "blob",
    feature = "shacl",
    feature = "security",
    feature = "sparql",
    feature = "nl-query"
))]
mod medulla_h_query_nl_sparql_tenant_tests;
#[cfg(all(test, feature = "query", feature = "redb", feature = "security"))]
mod medulla_h_query_retrieval_round_two_tenant_tests;
#[cfg(all(test, feature = "query", feature = "ann-redb"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_query_sql_source_read_tests;
#[cfg(all(test, feature = "query", feature = "redb", feature = "ann-redb"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
mod medulla_h_retrieve_promotion_ops_tests;
