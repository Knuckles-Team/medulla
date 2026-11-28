//! The boot recovery of deliveries (MEDULLA-SDLC-R019.13).
//!
//! Once, after the listeners bind, every resident graph is read for deliveries whose running
//! assignment lost its lease while the engine was down. The pass runs the same
//! `recover_deliveries_at_boot` of the delivery methods under the service actor
//! `engine:delivery-recovery`, so the boot step and a delivery's own feedback recovery are one
//! code path: the same `recover_assignments`, the same new pending assignment with its recovery
//! note and the same one-batch commit. A second pass changes nothing, because a replaced
//! assignment is already ended.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use tokio::sync::RwLock;

/// The service actor the boot recovery runs under.
pub const DELIVERY_RECOVERY_ACTOR: &str = "engine:delivery-recovery";

static CALLS: AtomicU64 = AtomicU64::new(0);

/// The service context one graph's recovery runs under; its key names the call.
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
        DELIVERY_RECOVERY_ACTOR,
        &["kg:admin"],
    )?;
    Ok(VerifiedRequestContext::from_verified_claims_with_nonce(
        service.claims().clone(),
        key,
        Some(medulla_types::contract::Nonce::minted()),
    ))
}

/// Recover the deliveries of every resident graph once. Answers how many deliveries changed; a
/// graph that refuses is logged and the others still recover.
pub async fn run_once(state: &Arc<RwLock<ServerState>>) -> u32 {
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
    let mut changed = 0;
    for graph in graphs {
        let key = format!(
            "delivery-recovery:{graph}:{}",
            CALLS.fetch_add(1, Ordering::Relaxed)
        );
        let context = match service_context(anonymous, &server, key) {
            Ok(context) => context,
            Err(error) => {
                tracing::warn!(%graph, %error, "delivery recovery has no service context");
                continue;
            }
        };
        match medulla_server_router_arms::dispatch::router::sdlc::recover_deliveries_at_boot(
            state, &graph, &context,
        )
        .await
        {
            Ok(count) => changed += count,
            Err(error) => {
                tracing::warn!(%graph, %error, "delivery recovery refused for the graph");
            }
        }
    }
    changed
}

/// Spawn the one boot pass as a background task, so it never holds the listeners up.
pub fn spawn(state: Arc<RwLock<ServerState>>) {
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        let changed = run_once(&state).await;
        tracing::info!(
            changed,
            "post-bind maintenance: delivery recovery finished in {:.3}s (R019.13)",
            started.elapsed().as_secs_f64()
        );
    });
}
