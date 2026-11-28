//! The per-request context every graph-operation handler receives.

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_graph_core::graph::GraphCore;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_api::state::ServerState;

#[derive(Clone, Copy)]
pub struct GraphOpsContext<'a> {
    pub state: &'a Arc<RwLock<ServerState>>,
    pub req_id: u64,
    pub graph_name: &'a str,
    pub read_authority: &'a GraphReadAuthority,
    pub core: &'a Arc<GraphCore>,
    pub raw_core: &'a Arc<GraphCore>,
    pub raw_ledger_len: u64,
}
