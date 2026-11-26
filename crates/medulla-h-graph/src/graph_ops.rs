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
use medulla_protocol_types::protocol::LedgerReadResult;
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_api::compute::{compute_off_lock, weight_semantic_results};
use medulla_server_api::state::{ServerState, MAX_BATCH_IDS};
use medulla_server_auth::access::check_graph_access;
use medulla_server_auth::access::requires_write;
use medulla_server_mutation::{
    mutation, mutation::GatewayAuthzCtx, mutation::MutationCtx, mutation::MutationPlan,
};

mod algorithms;
pub(crate) mod architecture_exchange;
mod argument_map;
mod retrieval_feedback;
mod run_failure_defect;
pub mod assistant_profile;
mod atom_context;
mod harness_edit;
mod process_compile;
mod process_document;
#[cfg(test)]
mod harness_edit_tests;
mod observation_state;
#[cfg(test)]
mod memory_surface_tests;
#[cfg(test)]
mod memory_snapshot_tests;
#[cfg(all(test, feature = "query"))]
mod discover_generation_tests;
#[cfg(all(test, feature = "query"))]
mod discover_seed_tests;
#[cfg(test)]
mod observation_state_tests;
#[cfg(test)]
mod participant_memory_tests;
// MEDULLA-DECISION-ENGINE-R617.9 — served participant writes.
pub mod participant_derive;
pub mod participant_surface;
// MEDULLA-BRANCH-MERGE-R001 — served branch registry.
#[cfg(feature = "redb")]
pub mod branch;
mod broker;
// MEDULLA-DURABLE-KERNEL-R095.1001 — served agent bus roster.
pub mod bus;
mod context_facts;
#[cfg(test)]
mod context_facts_tests;
mod discover_plan;
#[cfg(feature = "query")]
mod discover_quality;
pub mod domain_pack;
mod edges;
#[cfg(feature = "finance")]
mod finance_accounting;
#[cfg(feature = "finance")]
mod finance_performance;
mod finance_alerts;
mod finance_factors;
#[cfg(feature = "finance")]
mod finance_order_limit_check;
mod finance_order_proposal;
#[cfg(feature = "finance")]
pub mod finance_forecast_gate;
mod finance_records;
#[cfg(feature = "finance")]
mod finance_regime;
#[cfg(feature = "finance")]
mod finance_regime_gate;
mod gateway;
#[cfg(feature = "broker")]
mod gateway_broker;
mod gateway_graph;
#[cfg(any(feature = "graphlearn", feature = "ml-pipeline"))]
mod gateway_mining_ml;
#[cfg(feature = "streaming")]
mod graph_change_feed;
mod hierarchy;
mod memory;
mod memory_cursor;
pub mod memory_surface;
pub mod memory_ingest;
pub mod memory_snapshot;
mod nl_correction;
mod nodes;
// MEDULLA-SRC-AUTH-R801: the reviewed policy source binding.
pub mod policy_source;
mod read_span;
#[cfg(test)]
mod read_span_tests;
pub mod retention_cleanup;
mod schema_boundary;
mod semantic;
mod source_sync;
mod subgraph;
mod terminal;
mod union;
mod wallet_trust;
mod workflow_registry;
#[cfg(test)]
mod workflow_registry_tests;

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
pub use terminal::try_handle;
