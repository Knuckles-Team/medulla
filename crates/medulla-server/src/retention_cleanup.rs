//! The retention cleanup tick (MEDULLA-DURABLE-KERNEL-R700.3).
//!
//! `server_startup::retention_cleanup_tick` calls [`run_tick`] every
//! `MEDULLA_RETENTION_CLEANUP_SECS` seconds. With no `MEDULLA_RETENTION_CLEANUP`
//! policy the tick does nothing. Otherwise it runs one bounded step per resident
//! graph by re-entering the served `RetentionCleanupRun` under the service actor
//! `engine:retention-cleanup`, so the sweep and the served method are the one code
//! path: the same `cleanup_graph`, the same one-`BatchUpdate` commit and the same
//! report. Each graph resumes after the cursor its last step returned.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use medulla_h_graph::graph_ops::retention_cleanup::{configured_policy, RETENTION_ACTOR};
use medulla_mutation_types::retention_cleanup::{
    RetentionCleanupReport, RetentionCleanupRunRequest,
};
use medulla_protocol_types::protocol::{Method, Request, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use tokio::sync::RwLock;

/// Whether a valid `MEDULLA_RETENTION_CLEANUP` policy is configured.
pub fn is_configured() -> bool {
    configured_policy().is_some()
}

/// The sweep interval in seconds, within [60, 86400].
pub fn sweep_interval_secs() -> u64 {
    medulla_h_graph::graph_ops::retention_cleanup::sweep_interval_secs()
}

/// The cursor each graph's next step resumes after.
static CURSORS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

fn cursor_of(graph: &str) -> Option<String> {
    CURSORS.lock().ok().and_then(|map| map.get(graph).cloned())
}

fn remember(graph: &str, next: Option<String>) {
    if let Ok(mut map) = CURSORS.lock() {
        match next {
            Some(cursor) => {
                map.insert(graph.to_string(), cursor);
            }
            None => {
                map.remove(graph);
            }
        }
    }
}

/// The service context one step runs under; its key names the graph and instant.
fn service_context(
    anonymous: bool,
    server: &medulla_server_api::server_context::ServerContext,
    key: String,
) -> Result<VerifiedRequestContext, String> {
    if anonymous {
        return Ok(VerifiedRequestContext::anonymous_local(Some(key)));
    }
    let service = VerifiedRequestContext::authenticated_fixed_service_actor(
        server,
        RETENTION_ACTOR,
        &["retention:run"],
    )?;
    Ok(VerifiedRequestContext::from_verified_claims_with_nonce(
        service.claims().clone(),
        key,
        Some(medulla_types::contract::Nonce::minted()),
    ))
}

/// Run one step over `graph`; answers how many nodes it removed.
async fn step(
    state: &Arc<RwLock<ServerState>>,
    anonymous: bool,
    server: &medulla_server_api::server_context::ServerContext,
    graph: &str,
    now_ms: u64,
) -> Result<u64, String> {
    let context = service_context(
        anonymous,
        server,
        format!("retention-cleanup:{graph}:{now_ms}"),
    )?;
    let request = Request {
        id: 0,
        graph: graph.to_string(),
        auth_token: String::new(),
        agent_id: Some(RETENTION_ACTOR.to_string()),
        method: Method::RetentionCleanupRun {
            request: RetentionCleanupRunRequest {
                graph: graph.to_string(),
                dry_run: false,
                cursor: cursor_of(graph),
            },
        },
    };
    let response =
        medulla_server_api::api::reentry::dispatch_verified_request(state, request, context).await;
    if let Some(refusal) = response.refusal_text() {
        return Err(refusal);
    }
    let Some(ResultPayload::Raw(bytes)) = response.result else {
        return Err("the cleanup step answered an unexpected payload".to_string());
    };
    let report: RetentionCleanupReport = rmp_serde::from_slice(&bytes)
        .map_err(|error| format!("the cleanup report is unreadable: {error}"))?;
    remember(graph, report.next_cursor.clone());
    if report.scanned == 0 {
        // The scans reached their end: start over on the next sweep.
        remember(graph, None);
    }
    Ok(report.removed)
}

/// Run one cleanup sweep: one step per resident graph. Answers the nodes removed.
pub async fn run_tick(state: &Arc<RwLock<ServerState>>, now_ms: u64) -> u64 {
    if configured_policy().is_none() {
        return 0;
    }
    let (graphs, anonymous, server) = {
        let current = state.read().await;
        let graphs: Vec<String> = current
            .registry
            .all_entries()
            .iter()
            .map(|entry| entry.name.clone())
            .collect();
        (graphs, current.local_anonymous, current.context.clone())
    };
    let mut removed = 0;
    for graph in graphs {
        match step(state, anonymous, &server, &graph, now_ms).await {
            Ok(count) => removed += count,
            Err(error) => {
                tracing::warn!(%graph, %error, "retention cleanup step failed; retrying next sweep");
            }
        }
    }
    removed
}
