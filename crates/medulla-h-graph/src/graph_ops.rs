//! Graph-targeted operation handlers (node/edge CRUD, embeddings + semantic
//! search, topology/centrality/community algorithms, lifecycle/decay, ledger,
//! reasoning, and cross-graph fork/diff/subgraph-match). These borrow the graph
//! `core` (and, for cross-graph ops, the registry via `state`); heavy reads run
//! off-lock. The dispatch shell owns the cross-cutting write side-effects
//! (dirty/WAL/gauge) — handlers here only produce the `Response`.

use std::ops::ControlFlow;
use std::sync::Arc;
use tokio::sync::RwLock;

use medulla_core::isolation::AccessLevel;
use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::LedgerReadResult;
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::access::{check_graph_access, requires_write, GraphReadAuthority};
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::compute::{compute_off_lock, weight_semantic_results};
use medulla_server_api::mutation::{self, GatewayAuthzCtx, MutationCtx, MutationPlan};
use medulla_server_api::state::{
    max_response_edges, max_response_nodes, ServerState, MAX_BATCH_IDS,
};

mod algorithms;
mod broker;
mod edges;
mod gateway;
// Consumed outside this module only by the governed GraphSchema writes.
#[cfg(feature = "shacl")]
#[cfg(feature = "broker")]
mod gateway_broker;
mod gateway_graph;
#[cfg(any(feature = "graphlearn", feature = "ml-pipeline"))]
mod gateway_mining_ml;
mod hierarchy;
mod memory;
mod nodes;
mod semantic;
mod subgraph;
mod terminal;
mod union;

#[cfg(any(
    feature = "query",
    feature = "cypher",
    feature = "graphql",
    feature = "rdf"
))]
pub use gateway::mining_response_to_gateway_result as gateway_response_result;
pub use gateway::try_handle_gateway;
pub use terminal::try_handle;
