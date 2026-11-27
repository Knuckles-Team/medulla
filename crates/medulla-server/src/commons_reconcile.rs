//! The commons reconcile sweep (MEDULLA-IDENTITY-R023.13.7).
//!
//! A commons share copies the node into `__commons__` before it commits the node's
//! share keys, so a crash between the two writes can leave an orphan copy. One tick
//! of [`run_tick`] reads a page of the `__commons__` graph's `commons:` nodes and
//! removes every copy whose source node no longer holds `_shared_scope = commons`
//! and `_commons_copy = <this copy>`. The tick never touches a source node and
//! never writes a share key, so it cannot undo a share in flight: a copy younger
//! than [`RECONCILE_GRACE_MS`] is left for a later tick.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use medulla_isolation::isolation::share::{COMMONS_COPY_KEY, SCOPE_COMMONS, SHARED_SCOPE_KEY};
use medulla_protocol_types::protocol::{Method, Request, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use tokio::sync::RwLock;

/// The service actor the pass runs under.
pub const RECONCILE_ACTOR: &str = "engine:commons-reconcile";

/// The graph that holds the commons copies.
const COMMONS_GRAPH: &str = "__commons__";

/// The id prefix of a commons copy.
const COPY_PREFIX: &str = "commons:";

/// The most copies one tick reads.
const PAGE_SIZE: usize = 256;

/// A copy younger than this is never removed: a promotion writes the copy before
/// the node's keys.
pub const RECONCILE_GRACE_MS: u64 = 60_000;

/// The copy key that names the graph the copy was promoted from.
pub const SOURCE_GRAPH_KEY: &str = "_commons_source_graph";

/// The copy key that holds the instant the share path wrote it.
pub const CREATED_AT_KEY: &str = "_created_at_ms";

/// The cursor the next tick resumes after.
static CURSORS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// Distinguishes the calls of one tick.
static CALLS: AtomicU64 = AtomicU64::new(0);

fn cursor() -> Option<String> {
    CURSORS
        .lock()
        .ok()
        .and_then(|map| map.get(COMMONS_GRAPH).cloned())
}

fn remember(next: Option<String>) {
    if let Ok(mut map) = CURSORS.lock() {
        match next {
            Some(cursor) => {
                map.insert(COMMONS_GRAPH.to_string(), cursor);
            }
            None => {
                map.remove(COMMONS_GRAPH);
            }
        }
    }
}

/// The service context one call runs under; its key names the call.
pub(crate) fn service_context(
    anonymous: bool,
    server: &medulla_server_api::server_context::ServerContext,
    actor: &str,
    key: String,
) -> Result<VerifiedRequestContext, String> {
    if anonymous {
        return Ok(VerifiedRequestContext::anonymous_local(Some(key)));
    }
    let service =
        VerifiedRequestContext::authenticated_fixed_service_actor(server, actor, &["kg:admin"])?;
    Ok(VerifiedRequestContext::from_verified_claims_with_nonce(
        service.claims().clone(),
        key,
        Some(medulla_types::contract::Nonce::minted()),
    ))
}

type Properties = serde_json::Map<String, serde_json::Value>;

/// One served call under the service actor.
async fn call(
    state: &Arc<RwLock<ServerState>>,
    anonymous: bool,
    server: &medulla_server_api::server_context::ServerContext,
    graph: &str,
    now_ms: u64,
    method: Method,
) -> Result<medulla_protocol_types::protocol::Response, String> {
    let context = service_context(
        anonymous,
        server,
        RECONCILE_ACTOR,
        format!(
            "commons-reconcile:{graph}:{now_ms}:{}",
            CALLS.fetch_add(1, Ordering::Relaxed)
        ),
    )?;
    let request = Request {
        id: 0,
        graph: graph.to_string(),
        auth_token: String::new(),
        agent_id: Some(RECONCILE_ACTOR.to_string()),
        method,
        trace_id: None,
    };
    let response =
        medulla_server_api::api::reentry::dispatch_verified_request(state, request, context).await;
    match response.refusal_text() {
        Some(refusal) => Err(refusal),
        None => Ok(response),
    }
}

/// The committed properties of a source node; `None` when it is absent.
async fn source_properties(
    state: &Arc<RwLock<ServerState>>,
    anonymous: bool,
    server: &medulla_server_api::server_context::ServerContext,
    graph: &str,
    node_id: &str,
    now_ms: u64,
) -> Result<Option<Properties>, String> {
    let response = call(
        state,
        anonymous,
        server,
        graph,
        now_ms,
        Method::GetNodeProperties {
            node_id: node_id.to_string(),
        },
    )
    .await?;
    Ok(match response.result {
        Some(ResultPayload::Raw(bytes)) => {
            medulla_types::msgpack::decode_property_object(&bytes).ok()
        }
        Some(ResultPayload::Json(serde_json::Value::Object(object))) => Some(object),
        _ => None,
    })
}

/// Whether the source node no longer vouches for the copy.
fn orphaned(copy_id: &str, source: Option<&Properties>) -> bool {
    let Some(source) = source else {
        return true;
    };
    let text = |key: &str| source.get(key).and_then(serde_json::Value::as_str);
    text(SHARED_SCOPE_KEY) != Some(SCOPE_COMMONS) || text(COMMONS_COPY_KEY) != Some(copy_id)
}

/// Run one reconcile tick at `now_ms`; answers the copies removed.
pub async fn run_tick_at(state: &Arc<RwLock<ServerState>>, now_ms: u64) -> u64 {
    let (resident, anonymous, server): (BTreeSet<String>, bool, _) = {
        let current = state.read().await;
        let names = current
            .registry
            .all_entries()
            .iter()
            .map(|entry| entry.name.clone())
            .collect();
        (names, current.local_anonymous, current.context.clone())
    };
    if !resident.contains(COMMONS_GRAPH) {
        return 0;
    }
    let page = match call(
        state,
        anonymous,
        &server,
        COMMONS_GRAPH,
        now_ms,
        Method::GetNodesByLabel {
            label: String::new(),
            after: cursor(),
            limit: PAGE_SIZE,
        },
    )
    .await
    {
        Ok(response) => match response.result {
            Some(ResultPayload::NodeList(nodes)) => nodes,
            _ => Vec::new(),
        },
        Err(error) => {
            tracing::warn!(%error, "commons reconcile: the page read failed; retrying next sweep");
            return 0;
        }
    };
    remember(if page.len() < PAGE_SIZE {
        None
    } else {
        page.last().map(|(id, _)| id.clone())
    });
    let mut removed = 0;
    let mut skipped = 0u64;
    for (copy_id, properties) in page {
        let Some(node_id) = copy_id.strip_prefix(COPY_PREFIX) else {
            continue;
        };
        let source_graph = properties
            .get(SOURCE_GRAPH_KEY)
            .and_then(serde_json::Value::as_str);
        let created = properties
            .get(CREATED_AT_KEY)
            .and_then(serde_json::Value::as_u64);
        let (Some(source_graph), Some(created)) = (source_graph, created) else {
            skipped += 1;
            continue;
        };
        if !resident.contains(source_graph) || created.saturating_add(RECONCILE_GRACE_MS) > now_ms {
            skipped += 1;
            continue;
        }
        let source = match source_properties(
            state,
            anonymous,
            &server,
            source_graph,
            node_id,
            now_ms,
        )
        .await
        {
            Ok(source) => source,
            Err(error) => {
                tracing::warn!(%copy_id, %error, "commons reconcile: source read refused; copy kept");
                skipped += 1;
                continue;
            }
        };
        if !orphaned(&copy_id, source.as_ref()) {
            continue;
        }
        let removal = call(
            state,
            anonymous,
            &server,
            COMMONS_GRAPH,
            now_ms,
            Method::RemoveNode {
                node_id: copy_id.clone(),
            },
        )
        .await;
        match removal {
            Ok(_) => removed += 1,
            Err(error) => {
                tracing::warn!(%copy_id, %error, "commons reconcile: removal refused; retrying next sweep");
            }
        }
    }
    if skipped > 0 {
        tracing::debug!(skipped, "commons reconcile: copies skipped");
    }
    removed
}

/// Run one reconcile tick now; answers the copies removed.
pub async fn run_tick(state: &Arc<RwLock<ServerState>>) -> u64 {
    run_tick_at(
        state,
        medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
    )
    .await
}
