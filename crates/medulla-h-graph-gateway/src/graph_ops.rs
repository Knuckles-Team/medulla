//! Graph-targeted operation handlers (node/edge CRUD, embeddings + semantic
//! search, topology/centrality/community algorithms, lifecycle/decay, ledger,
//! reasoning, and cross-graph fork/diff/subgraph-match). These borrow the graph
//! `core` (and, for cross-graph ops, the registry via `state`); heavy reads run
//! off-lock. The dispatch shell owns the cross-cutting write side-effects
//! (dirty/WAL/gauge) — handlers here only produce the `Response`.

use std::ops::ControlFlow;
use std::sync::Arc;
use tokio::sync::RwLock;

use medulla_graph_core::graph::GraphCore;
use medulla_isolation::isolation::AccessLevel;
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_api::compute::{compute_off_lock, weight_semantic_results};
use medulla_server_api::state::ServerState;
use medulla_server_auth::access::check_graph_access;
use medulla_server_auth::access::requires_write;
use medulla_server_mutation::{
    mutation, mutation::GatewayAuthzCtx, mutation::MutationCtx, mutation::MutationPlan,
};

#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod architecture_exchange;
pub mod argument_map;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod assistant_profile;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod harness_edit;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod process_compile;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod process_document;
pub mod retrieval_feedback;
pub mod run_failure_defect;
// MEDULLA-ORCH-VOCAB-R009.101.1: the harness session index write.
#[cfg(all(test, feature = "query"))]
pub mod discover_generation_tests;
#[cfg(all(test, feature = "query"))]
pub mod discover_seed_tests;
#[cfg(test)]
pub mod harness_edit_tests;
pub mod harness_session;
#[cfg(test)]
pub mod memory_snapshot_tests;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod observation_state;
#[cfg(test)]
pub mod observation_state_tests;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod participant_derive;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod participant_surface;
// MEDULLA-DURABLE-KERNEL-R095.1001 — served agent bus roster.
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod bus;
pub mod discover_plan;
#[cfg(feature = "query")]
pub mod discover_quality;
pub mod domain_pack;
#[cfg(feature = "finance")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod finance_accounting;
pub mod finance_alerts;
pub mod finance_factors;
#[cfg(feature = "finance")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod finance_order_limit_check;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod finance_order_proposal;
#[cfg(feature = "finance")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod finance_performance;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod finance_receipts;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod finance_records;
pub mod gateway;
#[cfg(feature = "broker")]
pub mod gateway_broker;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod gateway_graph;
#[cfg(any(feature = "graphlearn", feature = "ml-pipeline"))]
pub mod gateway_mining_ml;
pub mod memory_cursor;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod memory_snapshot;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod memory_surface;
pub mod nl_correction;
// MEDULLA-SRC-AUTH-R801: the reviewed policy source binding.
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod ops_context;
pub mod policy_source;
pub mod retention_cleanup;
pub mod schema_boundary;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod semantic;
pub mod source_sync;
pub mod workflow_registry;
#[cfg(test)]
pub mod workflow_registry_tests;

#[cfg(any(
    feature = "query",
    feature = "cypher",
    feature = "graphql",
    feature = "rdf"
))]
pub use finance_records::precheck_replicated_put;
#[cfg(any(
    feature = "query",
    feature = "cypher",
    feature = "graphql",
    feature = "rdf",
    feature = "graphlearn",
    feature = "ml-pipeline"
))]
pub use gateway::mining_response_to_gateway_result as gateway_response_result;
pub use gateway::try_handle_gateway;
