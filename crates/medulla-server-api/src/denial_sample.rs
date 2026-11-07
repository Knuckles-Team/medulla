//! The durable denial sample.
//!
//! Every authorization denial already counts in metrics. The denial paths
//! also OFFER a sample to the isolation layer (`IsolationLayer::offer_denial`) -- an in-memory, bounded, rate-limited sampler,
//! cheap on the hot path -- and the engine periodically writes what it kept
//! into the identity store's hash-chained audit trail in ONE durable write.
#![cfg(feature = "security")]

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::state::ServerState;

/// How often kept samples are written.
const FLUSH_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// Write every kept sample into the identity audit trail.
pub(crate) async fn flush(state: &Arc<RwLock<ServerState>>) {
    let (samples, dropped) = state.read().await.isolation.drain_denials();
    if samples.is_empty() && dropped == 0 {
        return;
    }
    let mut guard = crate::state::timed_write(state).await;
    if let Err(error) = guard.isolation.try_record_denials(samples, dropped) {
        tracing::warn!(%error, "denial sample could not be written; dropped");
    }
    guard.publish_identity_view();
}

/// The periodic writer, one per engine. It also publishes the loaded
/// identity store to the SQL projection once at startup.
pub fn spawn_flusher(state: Arc<RwLock<ServerState>>) {
    tokio::spawn(async move {
        state.read().await.publish_identity_view();
        let mut ticker = tokio::time::interval(FLUSH_EVERY);
        loop {
            ticker.tick().await;
            flush(&state).await;
        }
    });
}
