//! Pure redb durable-row machinery (CONCEPT:MEDULLA-KG.storage.kg-kg / KG-2.195 / KG-2.216).
//!
//! This is the SERVER-INDEPENDENT half of the redb durable tier: the on-disk
//! table layout, the `Method → redb rows` apply, the group-commit, and the
//! full checkpoint/load read-back. It has NO Tokio and NO `ServerState`
//! dependency, so it compiles under `--features redb` ALONE (no `server`).
//!
//! Two callers share it — ONE durable format, never duplicated:
//!   * the out-of-process server's `medulla_server::persistence::redb_backend::RedbBackend`
//!     (gated on `server`), which wraps these in its off-reactor group-commit
//!     writer thread + the `PersistenceBackend` async trait; and
//!   * the in-process [`crate::embedded::EmbeddedEngine`] (gated on `embedded`),
//!     which commits through them DIRECTLY (the caller is the writer — durable,
//!     commit-before-return, no Tokio runtime).
//!
//! The redb `Database` and every table key/value shape here are byte-identical to
//! what the server writes, so a graph written by the embedded API reopens in the
//! server and vice-versa.
//!
//! ## Tables (all keyed by graph prefix)
//!   * `nodes`          `(graph, id)            -> node properties msgpack`
//!   * `edges`          `(graph, src, tgt, ord) -> edge properties msgpack`
//!   * `ledger`         `(graph, seq)           -> ledger line`
//!   * `semantic_store` `graph                  -> semantic store blob (msgpack)`
//!   * `graph_meta`     `graph                  -> identity + keyed schema authority`

pub mod enrichment_top_up;
pub mod store_prelude;
use store_prelude::*;

/// Durable rows are outside the native RPC frame validator and may be supplied
/// by a corrupted, restored, or otherwise untrusted database file. Keep one
/// format-wide ceiling aligned with the native protocol's hard request budget,
/// then structurally preflight every MessagePack row before serde can honor an
/// attacker-controlled collection size hint.
const MAX_DURABLE_MSGPACK_BYTES: usize = 384 * 1024 * 1024;
const MAX_DURABLE_STORED_BYTES: usize = MAX_DURABLE_MSGPACK_BYTES + 1024;
const MAX_DURABLE_MSGPACK_ITEMS: usize = 4_000_000;
pub const INITIAL_GRAPH_VERSION: u64 = 0;

/// Return whether any optional exact-text filter rejects its corresponding
/// stored value.  The resource and development-lane status projections use
/// the same predicate over different request/record DTOs; keeping the
/// comparison here removes that duplicated policy without coupling domains.
pub fn any_optional_text_filter_mismatch<'a, I>(filters: I) -> bool
where
    I: IntoIterator<Item = (Option<&'a str>, &'a str)>,
{
    filters
        .into_iter()
        .any(|(expected, actual)| expected.is_some_and(|expected| expected != actual))
}

/// One test-only graph-scoped control batch, shared by reservation and
/// WorkItem-capability fixtures. Both exercise the same caller-to-shard
/// binding, but retain their own actor, attempt time, and stable identity.
#[cfg(any(test, feature = "test-support"))]
pub struct ControlFixtureBatch {
    pub identity: medulla_mutation_types::mutation_scope::MutationScopeIdentity,
    pub batch_id: String,
    pub actor: String,
    pub request_id: u64,
    pub idempotency_key: String,
    pub expected_version: u64,
    pub method: Method,
    pub created_at_ms: u64,
}

#[cfg(any(test, feature = "test-support"))]
pub fn control_fixture_batch(input: ControlFixtureBatch) -> MutationBatch {
    let mut batch = MutationBatch {
        schema_version: MUTATION_BATCH_VERSION,
        batch_id: input.batch_id,
        envelope: fixture_operation_envelope(
            &input.identity,
            &input.actor,
            input.request_id,
            &input.idempotency_key,
        ),
        identity: input.identity,
        placement_epoch: 0,
        version_expectation: VersionExpectation::Graph(input.expected_version),
        fencing_token: None,
        authoritative_state: None,
        operations: vec![MutationOperation {
            ordinal: 0,
            surface: MutationSurface::Job,
            domain: DurabilityDomain::ControlPlane,
            method: input.method,
        }],
        outbox: Vec::new(),
        created_at_ms: input.created_at_ms,
    };
    batch
        .reseal_envelope(medulla_types::contract::Digest256::from_bytes([1_u8; 32]))
        .expect("a fixture batch reseals its envelope over its final body");
    batch
}

pub mod commit_timing;
pub mod store_graph_names;
pub mod store_read;
pub mod store_rows;
pub use store_read::*;
pub use store_rows::*;
pub use store_graph_names::*;
#[cfg(any(test, feature = "test-support"))]
pub use store_types::{decode_durable, DurableCrypto};
pub mod sealed_rows;
pub mod store_types;
pub mod unreadable;

pub use commit_timing::CommitPhaseTimer;
// The exact performance harness is a separate binary crate and must reach the
// sole public item in `store_rows`; all remaining helpers in that module stay
// private or crate-private.
pub use store_types::*;
pub use unreadable::{NodeUnreadable, UnsealFailure};


pub mod enrichment_budget;
/// The graph shard as a kernel-owned store (RF-RULING-004 step 8).
#[cfg(feature = "redb")]
pub mod shard;

pub mod audit;
#[cfg(feature = "security")]
pub mod operation_audit;

// The decomposition keeps the physical file as one `redb_store` API surface.
// Keep the root's imports and its server/embedded callers on that surface
// explicitly: child modules use `super::*`, while persistence code reaches the
// durable machinery through `crate::redb_store`, never through a second store.
#[cfg(feature = "security")]
pub use audit::{
    append_audit_entry, prove_inclusion, provenance_anchor_commit, provenance_leaf_hashes,
    verify_audit, AuditTailCache, ProvenanceAnchorCache,
};
#[cfg(feature = "security")]
pub use operation_audit::{
    audit_writer_unavailable, operation_audit_append, operation_audit_read, OperationAuditEvent,
};
pub use shard::{Shard, ShardWrite};
// ── Cross-shard 2PC durable rows (CONCEPT:MEDULLA-KG.storage.lane-n-increment) — pure, server-INDEPENDENT ──
// Shared store helpers (mirroring NODES/EDGES/purge_graph_rows): the `Cmd` arms in
// `redb_backend`'s off-reactor writer thread call straight into these.


/// Build a unique process-local `.redb` fixture path for a kernel-owned-store
/// test: `<prefix>-<tag>-<pid>-<nanos>.redb` under the OS temp directory.
/// Unlike [`crate::kernel::persistence::temp_dir`], this hands back a *file* path meant to
/// go straight to `redb::Database::create`/`Shard::open`, not a directory the
/// fixture owns end-to-end — so there is no matching stale-path cleanup here;
/// each caller is responsible for its own fixture's lifecycle, exactly as it
/// was before this helper had a single home. The four kernel-owned-store test
/// modules that build these paths (mutation-batch replay here, the keyset-page
/// dump tests, the shard bootstrap/graft tests, and the online-reshard tests)
/// used to hand-roll this same construction independently, varying only the
/// prefix; they now call through this one definition instead.
#[cfg(any(test, feature = "test-support"))]
pub fn temp_path(prefix: &str, tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "{prefix}-{tag}-{}-{}.redb",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
