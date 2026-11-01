//! Persistence and raft items moved down to their kernel and store users.

/// Stable serving principal for the engine's own kernel-owned durable writes.
///
/// This identity is shared by the server, embedded, and Raft/redb-only builds;
/// keeping it in the kernel prevents a standalone embedded build from acquiring a
/// dependency on crate::server.
pub const ENGINE_LEDGER_PRINCIPAL: &str =
    "principal:sha256:41290b0e412ac542f312d4312a7a299e771eec66e3ffbbf7edb6369576875fb2";

/// Replay entry of the data-mining write-back arms (MEDULLA-CONTRACT-R056.8).
/// The mining handlers live above the kernel; the server installs them here.
pub type ReplayHook = fn(&medulla_core::graph::GraphCore, &medulla_types::protocol::Method);

static MINING_REPLAY: std::sync::OnceLock<ReplayHook> = std::sync::OnceLock::new();

/// Install the mining write-back replay. The first installation wins.
pub fn install_mining_replay(hook: ReplayHook) {
    let _ = MINING_REPLAY.set(hook);
}

/// Replay one mining write-back `method` against `core` through the installed
/// hook; with none installed the method takes the generic durable-apply arm,
/// as in a build without the mining handlers.
pub fn replay_mining(core: &medulla_core::graph::GraphCore, method: &medulla_types::protocol::Method) {
    match MINING_REPLAY.get() {
        Some(hook) => hook(core, method),
        None => medulla_core::durable_apply::apply(core, method),
    }
}

/// Return a unique process-local temporary directory path and clear any stale
/// path from a previous interrupted test run.
///
/// The process-local sequence prevents collisions even when concurrent calls
/// observe the same system-clock tick. Callers retain their prefix and tag.
#[cfg(any(test, feature = "test-support"))]
pub fn temp_dir(prefix: &str, tag: &str) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "{prefix}-{tag}-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Durable-presence probe the resident-projection maintenance sweeps consult before
/// evicting a node from memory.
pub trait DurableNodePresence: Send + Sync {
    /// Whether each of `node_ids` is present in the authoritative store for the
    /// sanitized graph key `graph_fname`, in the order given.
    fn durable_node_presence(
        &self,
        graph_fname: &str,
        node_ids: &[String],
    ) -> Result<Vec<bool>, String>;
}

/// The server state as seen by the resident-projection maintenance in `crate::persist`.
pub trait ResidentProjectionHost {
    /// Every registered graph as `(name, resident core)`.
    fn resident_graphs(&self) -> Vec<(String, std::sync::Arc<medulla_core::graph::GraphCore>)>;

    /// The durable-presence probe of the configured persistence tier, or `None`
    /// when no persistence is configured.
    fn durable_presence(&self) -> Option<std::sync::Arc<dyn DurableNodePresence>>;
}
