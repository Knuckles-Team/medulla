//! The query-side adapter on the served read paths (EH-396): the decoded adapter, the plan
//! rewrite and the cache salt. The adapter state itself is read by the decide handler through
//! `DecideApi::served_adapter`.
//!
//! The adapter only ever changes the QUERY vector, and only where the probe is already confined
//! to what the caller may see. A space with no active adapter -- or an adapter state that cannot
//! be read -- serves the base query unchanged.

use std::sync::Arc;

use medulla_core::graph::GraphCore;
use medulla_numeric::decision::adapter::AdapterKernel;

use crate::server::state::ServerState;

/// The active adapter of one graph, decoded for serving.
pub struct ServedAdapter {
    digest: String,
    space_digest: String,
    kernel: AdapterKernel,
}

impl ServedAdapter {
    /// Decode a checked body for serving under its content `digest`.
    pub fn for_body(
        digest: &str,
        body: &medulla_types::decision::statistical::retrieval_adapter::QueryAdapterBody,
    ) -> Result<Self, String> {
        Ok(Self {
            digest: digest.to_string(),
            space_digest: body.space_digest.clone(),
            kernel: AdapterKernel::of(body)?,
        })
    }

    /// The embedding-space digest the adapter was fitted in.
    pub fn space_digest(&self) -> &str {
        &self.space_digest
    }

    /// The content digest the result cache is salted with.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// The adapted query, or `None` for a query of another width.
    pub fn adapt(&self, query: &[f32]) -> Option<Vec<f32>> {
        self.kernel.apply(query)
    }
}

/// `query`, adapted when an adapter is active for `graph` under `tenant`.
pub async fn adapted_query(
    state: &Arc<tokio::sync::RwLock<ServerState>>,
    tenant: Option<&str>,
    (graph, core): (&str, &GraphCore),
    query: Vec<f32>,
) -> Vec<f32> {
    match resolve_served(state, tenant, graph, core).await {
        Some(adapter) => adapter.adapt(&query).unwrap_or(query),
        None => query,
    }
}

/// The adapter active for `graph` under the verified carrier tenant `tenant`, resolved by
/// the decide handler. `None` when no decide seam is installed, so the base query is served.
pub async fn resolve_served(
    state: &Arc<tokio::sync::RwLock<ServerState>>,
    tenant: Option<&str>,
    graph: &str,
    core: &GraphCore,
) -> Option<Arc<ServedAdapter>> {
    let decide = super::decide_seam(state).await?;
    decide.served_adapter(state, tenant, graph, core).await
}

/// Fold the active adapter's digest into a result-cache key payload.
pub fn salt(adapter: Option<&ServedAdapter>, payload: &mut Vec<u8>) {
    let digest = adapter.map_or("", ServedAdapter::digest);
    payload.extend_from_slice(digest.as_bytes());
}

/// The verified carrier tenant of a read, the partition adapter state is
/// served from (the same scope `DecisionLog.retrieval` activates under).
pub fn carrier_tenant(
    authority: Option<&crate::server::access::GraphReadAuthority>,
) -> Option<&str> {
    authority
        .and_then(crate::server::access::GraphReadAuthority::carrier)
        .map(crate::server::access::CarrierAuthority::tenant_scope)
}

/// The adapter a unified plan over `graph` runs with: only a plan that ranks
/// by vector looks one up.
#[cfg(feature = "query")]
pub async fn served_plan_adapter(
    state: &Arc<tokio::sync::RwLock<ServerState>>,
    graph: &str,
    authority: Option<&crate::server::access::GraphReadAuthority>,
    plan: &medulla_plan::Plan,
) -> Option<Arc<ServedAdapter>> {
    if !plan_ranks_by_vector(&plan.ops) {
        return None;
    }
    let core = Arc::clone(&state.read().await.registry.get(graph)?.core);
    resolve_served(state, carrier_tenant(authority), graph, &core).await
}

/// `plan` with every vector rank re-aimed by `adapter` (unchanged without one).
#[cfg(feature = "query")]
pub fn adapt_plan(
    plan: medulla_plan::Plan,
    adapter: Option<Arc<ServedAdapter>>,
    embedder: Option<&dyn medulla_plan::TextEmbedder>,
) -> medulla_plan::Plan {
    match adapter {
        Some(adapter) => medulla_plan::Plan::new(rewrite_ops(plan.ops, &adapter, embedder)),
        None => plan,
    }
}

/// Rewrite every vector rank through `adapter`. A `RankEmbed` is resolved to
/// its vector with the server-side `embedder` first; with none bound (or an
/// embedding error) it is left as it was, so the executor reports exactly what
/// it reported before.
#[cfg(feature = "query")]
fn rewrite_ops(
    ops: Vec<medulla_plan::Op>,
    adapter: &ServedAdapter,
    embedder: Option<&dyn medulla_plan::TextEmbedder>,
) -> Vec<medulla_plan::Op> {
    use medulla_plan::Op;
    let adapt = |query: Vec<f32>| Op::Rank {
        query: adapter.adapt(&query).unwrap_or(query),
    };
    ops.into_iter()
        .map(|op| match op {
            Op::Rank { query } => adapt(query),
            Op::RankEmbed { text } => match embedder.map(|e| e.embed(&text)) {
                Some(Ok(query)) => adapt(query),
                Some(Err(_)) | None => Op::RankEmbed { text },
            },
            Op::FuseRrf { branches, k } => Op::FuseRrf {
                branches: branches
                    .into_iter()
                    .map(|branch| rewrite_ops(branch, adapter, embedder))
                    .collect(),
                k,
            },
            other => other,
        })
        .collect()
}

/// Whether a plan ranks by vector anywhere (only then is an adapter looked up).
#[cfg(feature = "query")]
pub fn plan_ranks_by_vector(ops: &[medulla_plan::Op]) -> bool {
    use medulla_plan::Op;
    ops.iter().any(|op| match op {
        Op::Rank { .. } | Op::RankEmbed { .. } => true,
        Op::FuseRrf { branches, .. } => branches.iter().any(|b| plan_ranks_by_vector(b)),
        _ => false,
    })
}
