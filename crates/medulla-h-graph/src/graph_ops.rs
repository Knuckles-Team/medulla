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
use medulla_server_api::compute::compute_off_lock;
use medulla_server_api::state::{ServerState, MAX_BATCH_IDS};
use medulla_server_auth::access::check_graph_access;

use medulla_h_graph_gateway::graph_ops::*;

mod algorithms;
mod atom_context;
#[cfg(test)]
mod memory_surface_tests;
#[cfg(test)]
mod participant_memory_tests;
// MEDULLA-DECISION-ENGINE-R617.9 — served participant writes.
pub mod participant_card;
mod participant_read;
// MEDULLA-BRANCH-MERGE-R001 — served branch registry.
#[cfg(feature = "redb")]
pub mod branch;
mod broker;
mod context_facts;
#[cfg(test)]
mod context_facts_tests;
mod edges;
#[cfg(feature = "finance")]
pub mod finance_forecast_gate;
#[cfg(feature = "finance")]
mod finance_regime;
#[cfg(feature = "finance")]
mod finance_regime_gate;
#[cfg(feature = "streaming")]
mod graph_change_feed;
mod hierarchy;
mod memory;
pub mod memory_ingest;
mod nodes;
mod read_span;
#[cfg(test)]
mod read_span_tests;
mod subgraph;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod terminal;
mod union;
mod wallet_trust;

#[cfg(any(
    feature = "query",
    feature = "cypher",
    feature = "graphql",
    feature = "rdf"
))]
pub use finance_records::precheck_replicated_put;
pub use terminal::try_handle;
