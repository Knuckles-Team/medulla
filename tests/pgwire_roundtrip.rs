//! Postgres wire round-trip integration test (CONCEPT:AU-KG.query.raw-python).
//!
//! Starts the real pgwire listener over an in-process `ServerState`, connects with
//! the real `tokio-postgres` client, and proves the shim end-to-end:
//!   * `SELECT id FROM nodes …` over a seeded graph returns the expected rows
//!     (reusing the medulla-query DataFusion path — the SAME code `Method::Sql` runs),
//!   * an `INSERT INTO nodes …` (routed through the GraphTxn write path) is visible
//!     to a subsequent `SELECT` on the same connection.
//!
//! It also proves the durability barrier (CONCEPT:MEDULLA-KG.query.concept-10): a pgwire
//! INSERT awaits `record_durable` before completion, verified by reading the row
//! back from a fresh backend on the same directory.
//!
//! Only compiled with `--features pgwire` (the listener + the medulla-query SQL path);
//! the authoritative durability test additionally needs `--features redb`.

#![cfg(feature = "pgwire")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_core::registry::GraphRegistry;
use medulla_identity_types::acl::{AgentIdentity, AgentRole};
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::state::ServerState;
use medulla_wire::pgwire;

fn sql_test_persist_dir() -> String {
    test_support::sql_test_persist_dir("pgwire")
}

/// Build a minimal `ServerState` with one seeded node so a wire SELECT has rows to
/// return. `__commons__` is pre-created by the registry.
fn state_with(persistence: Option<Arc<dyn PersistenceBackend>>) -> test_support::SharedState {
    state_with_dir(persistence, sql_test_persist_dir())
}

fn state_with_dir(
    persistence: Option<Arc<dyn PersistenceBackend>>,
    persist_dir: String,
) -> test_support::SharedState {
    test_support::seeded_wire_state("test", "tester", Some(persist_dir), persistence)
}

/// The cache-only default state (no durable tier) the original round-trip tests use.
/// A real tempdir-backed `RedbBackend` (mirrors `common::tempdir_persistence()`,
/// `medulla_server::mod::tests::test_state`, `bolt_wire::tests::durable_state`, and every
/// other `tests/*.rs` fixture that needs one): the authoritative-commit flip means
/// EVERY graph-node write now routes through the universal cross-modal MutationBatch
/// kernel (`commit_cross_modal_txn`), which fails closed ("cross-modal mutation
/// requires durable persistence") against a `persistence: None` state. A keyed
/// backend additionally satisfies the multi-op commit's transaction-recovery-plan
/// seal, which requires `MEDULLA_ENCRYPTION_KEY` at `open()` (see
/// `redb_backend::tests::cm_dir`'s identical requirement) — provisioned once, before
/// the first backend opens.
fn default_persistence() -> Option<test_support::SharedPersistence> {
    test_support::durable_persistence("pgwire-roundtrip-recovery-key")
}

fn seeded_state() -> test_support::SharedState {
    state_with(default_persistence())
}

/// Reopen a `RedbBackend` at `dir` with a short bounded retry. `shutdown()` joins the
/// writer thread, which drops that thread's sole STRONG `Arc<Database>` (the shard
/// only ever keeps a `Weak`, see `redb_backend::Shard::db`'s doc) so the exclusive
/// per-process file lock releases once the thread has actually finished unwinding —
/// a small window after `shutdown()`/`join()` returns where the very next `open()`
/// can still race the OS releasing the lock. Mirrors the identical retry
/// `advanced_crossmodal_roundtrip.rs`'s `encryption_at_rest_wrong_key_fails_eg394`
/// needed for the same race.
#[cfg(feature = "redb")]
fn reopen_redb_with_retry(
    dir: &str,
    cache: usize,
) -> medulla_persistence::redb_backend::RedbBackend {
    use medulla_persistence::redb_backend::RedbBackend;
    let mut last_err = None;
    for attempt in 0..50 {
        match RedbBackend::open(dir.to_string(), cache) {
            Ok(backend) => return backend,
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(20 * (attempt + 1)));
            }
        }
    }
    panic!("reopen redb backend at {dir} after bounded retry: {last_err:?}");
}

#[cfg(feature = "redb")]
fn interval_backend(label: &str) -> (String, Arc<dyn PersistenceBackend>) {
    use medulla_persistence::redb_backend::RedbBackend;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "medulla-pgwire-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let dir_s = dir.to_string_lossy().into_owned();
    let backend: Arc<dyn PersistenceBackend> =
        Arc::new(RedbBackend::open(dir_s.clone(), 64).expect("open redb backend"));
    (dir_s, backend)
}

#[cfg(feature = "redb")]
async fn reopened_node(
    dir: &str,
    node_id: &str,
) -> (
    medulla_persistence::redb_backend::RedbBackend,
    Option<Vec<u8>>,
) {
    let reopened = reopen_redb_with_retry(dir, 64);
    let stored = reopened
        .read_node("__commons__", node_id)
        .await
        .expect("read_node");
    (reopened, stored)
}

/// Bind the listener on an ephemeral port with mandatory SCRAM authentication.
/// Returns the chosen `127.0.0.1:<port>` address.
async fn spawn_listener(state: Arc<RwLock<ServerState>>) -> String {
    spawn_listener_mode(state, pgwire::PgWireAuthMode::Scram).await
}

/// Bind + serve with an EXPLICIT auth mode (CONCEPT:MEDULLA-KG.query.concept-13). Used by the auth
/// tests to pin SCRAM deterministically (no process-global env toggle).
async fn spawn_listener_mode(
    state: Arc<RwLock<ServerState>>,
    mode: pgwire::PgWireAuthMode,
) -> String {
    provision_text_embedder_once();
    test_support::spawn_pgwire_listener(state, mode).await
}

/// Provision `MEDULLA_UQL_TEXT_EMBEDDER=hash` for this whole test process, before ANY
/// pgwire listener exists (GOC-70 rule 2).
///
/// The server resolves that variable exactly once per process -- `uql_text_embedder`
/// is a `OnceLock` and `bind_sql_text_embedder` a `Once`, both latched by the FIRST
/// pgwire query of ANY session (`bind_startup_from_client`). A single test setting
/// the variable in its own body therefore raced every sibling test's first query:
/// whichever session queried first latched "no embedder" for the process, and
/// `wire_eg_embed_is_reachable_over_a_pgwire_only_session` failed under 2-core
/// affinity. Every listener in this binary is spawned through `spawn_listener_mode`
/// or `spawn_listener_abortable`, and no query can precede a listener, so doing the
/// mutation here -- once, with `Once` blocking concurrent callers until it is done --
/// makes the embedder deterministically bound before any reader can latch it. No
/// other test in this binary depends on the embedder being unbound.
fn provision_text_embedder_once() {
    static TEXT_EMBEDDER_ENV: std::sync::Once = std::sync::Once::new();
    TEXT_EMBEDDER_ENV.call_once(|| std::env::set_var("MEDULLA_UQL_TEXT_EMBEDDER", "hash"));
}

/// Like [`spawn_listener`], but hands back the listener task's `JoinHandle` too. A
/// durability test that reopens the SAME on-disk persistence directory mid-test needs
/// this: `shutdown()` alone does not release redb's advisory file lock — the lock is
/// held for as long as ANY `Arc<RedbBackend>` clone survives (see
/// `redb_backend::tests::many_recreate_cycles_keep_inmemory_writes_visible`'s identical
/// note), and the listener task captured its OWN clone via `state.persistence` and
/// runs forever (its `JoinHandle` is normally discarded). The caller must `.abort()`
/// this handle and await it BEFORE reopening, so the task's captured `state` (and thus
/// its `persistence` clone) is actually dropped.
async fn spawn_listener_abortable(
    state: Arc<RwLock<ServerState>>,
) -> (String, tokio::task::JoinHandle<()>) {
    provision_text_embedder_once();
    let addr_s = test_support::ephemeral_listener_addr().await;
    let serve_addr = addr_s.clone();
    let handle = tokio::spawn(async move {
        let _ = pgwire::serve_with_auth(&serve_addr, state, pgwire::PgWireAuthMode::Scram).await;
    });
    test_support::wait_for_listener_ready(&addr_s).await;
    (addr_s, handle)
}

/// Connect a real tokio-postgres client with the fixture's derived SCRAM password.
async fn connect(addr: &str) -> tokio_postgres::Client {
    test_support::connect_pgwire(addr, "test", "tester", "__commons__")
        .await
        .expect("pgwire connect")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_select_returns_seeded_rows() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    let rows = client
        .simple_query("SELECT id FROM nodes WHERE rank >= 2 ORDER BY id")
        .await
        .expect("simple_query SELECT");

    // simple_query yields a mix of RowDescription/Row/CommandComplete messages;
    // pull the data rows.
    let ids = simple_ids(rows);
    assert_eq!(ids, vec!["n2".to_string(), "n3".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_rejects_retired_graph_compatibility_sql_and_extensions() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;
    let sql = "SELECT * FROM cypher('__commons__', $$ MATCH (n) RETURN n $$) AS (n agtype)";

    client
        .simple_query(sql)
        .await
        .expect_err("simple protocol must reject the retired graph compatibility surface");

    match client.prepare(sql).await {
        Ok(statement) => {
            client.query(&statement, &[]).await.expect_err(
                "extended protocol must reject the retired graph compatibility surface",
            );
        }
        Err(_) => {
            // Rejecting during Parse/Describe is also a valid fail-closed outcome.
        }
    }

    for extension in ["age", "pg_age"] {
        let result = client
            .simple_query(&format!("CREATE EXTENSION {extension}"))
            .await;
        assert!(
            result.is_err(),
            "CREATE EXTENSION {extension} must reject a retired extension"
        );
    }

    let rows = client
        .simple_query("SELECT id FROM nodes WHERE rank >= 2 ORDER BY id")
        .await
        .expect("native SQL remains available after rejected compatibility requests");
    assert_eq!(simple_ids(rows), vec!["n2".to_string(), "n3".to_string()]);
}

/// BUG-CX-084: `medulla_embed(text)`'s process-wide embedder (design §9 phase 2) used to be
/// bound ONLY at the top of the native RPC `handle_sql` entry point — the pgwire
/// `do_query`/extended-`do_query` handlers never call `handle_sql` (they run the
/// shared `WireSession::execute` core directly), so a `psql`/tokio-postgres client
/// speaking ONLY pgwire could never reach a bound embedder: `medulla_embed(...)` always
/// fell through to its typed "no server-side text embedder is bound" error, even with
/// `MEDULLA_UQL_TEXT_EMBEDDER` configured. This drives a real tokio-postgres client at a
/// real pgwire listener — the actual reachability path the bug report names — and
/// proves the SQL resolves rather than erroring closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_eg_embed_is_reachable_over_a_pgwire_only_session() {
    // `MEDULLA_UQL_TEXT_EMBEDDER=hash` is provisioned process-wide by
    // `provision_text_embedder_once` inside `spawn_listener`, before this or any other
    // session's first query can latch the embedder (see that function's doc).
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    let rows = client
        .simple_query("SELECT medulla_embed('leaky pump') AS v")
        .await
        .unwrap_or_else(|e| {
            panic!(
                "medulla_embed(...) must be reachable over a pgwire-only session \
                 (BUG-CX-084), got: {e:?}"
            )
        });
    let data_rows = rows
        .iter()
        .filter(|m| matches!(m, tokio_postgres::SimpleQueryMessage::Row(_)))
        .count();
    assert_eq!(data_rows, 1, "a bound embedder must return exactly one row");
}

// spec: MEDULLA-DURABLE-KERNEL-R860.1
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_insert_then_select_round_trip() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    // INSERT a new node through the GraphTxn write path (simple-query protocol —
    // the surface the first increment supports; extended Parse/Bind is a follow-up).
    let insert = client
        .simple_query(
            "INSERT INTO nodes (id, type, rank, _visibility, _owner) VALUES ('n9', 'Agent', 9, 'public', 'tester')",
        )
        .await
        .expect("INSERT");
    let affected = command_count(&insert);
    assert_eq!(affected, 1, "one row inserted");

    // It must now be visible to a SELECT on the same connection.
    let rows = client
        .simple_query("SELECT id FROM nodes WHERE rank = 9")
        .await
        .expect("SELECT after insert");
    let ids = simple_ids(rows);
    assert_eq!(ids, vec!["n9".to_string()]);

    // The same row through the extended protocol (Parse/Bind/Execute).
    let stmt = client
        .prepare("SELECT id FROM nodes WHERE rank = $1")
        .await
        .expect("prepare after insert");
    let rows = client
        .query(&stmt, &[&9_i64])
        .await
        .expect("extended SELECT after insert");
    let ids: Vec<String> = rows.iter().map(|r| r.get::<_, String>(0)).collect();
    assert_eq!(ids, vec!["n9".to_string()]);
}

// ───────────────────────────────────────────────────────────────────────────
// Extended / prepared protocol (CONCEPT:MEDULLA-KG.query.describe) — real driver path.
// `tokio-postgres`'s `prepare`/`query`/`execute` use the extended protocol
// (Parse/Bind/Describe/Execute/Sync), which is what psycopg3/asyncpg/JDBC/sqlx/
// SQLAlchemy use by DEFAULT. These prove a real driver's prepared+parameterized
// round-trip works against the shim, not just raw `psql`.
// ───────────────────────────────────────────────────────────────────────────

/// Extended protocol: `client.prepare(...)` + a parameterized `query` with a
/// typed param (`WHERE rank > $1`) returns the matching rows. Proves Parse/Bind/
/// Describe/Execute + parameter binding through a real driver.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extended_prepared_parameterized_select() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    // prepare() issues Parse + Describe(statement) + Sync; query() issues Bind +
    // Execute + Sync — the full extended flow.
    let stmt = client
        .prepare("SELECT id FROM nodes WHERE rank > $1 ORDER BY id")
        .await
        .expect("prepare");
    let rows = client
        .query(&stmt, &[&1i64])
        .await
        .expect("parameterized query");
    let ids: Vec<String> = rows.iter().map(|r| r.get::<_, String>(0)).collect();
    assert_eq!(ids, vec!["n2".to_string(), "n3".to_string()]);

    // A different bound value reuses the SAME prepared statement (new portal).
    let rows2 = client.query(&stmt, &[&2i64]).await.expect("second bind");
    let ids2: Vec<String> = rows2.iter().map(|r| r.get::<_, String>(0)).collect();
    assert_eq!(ids2, vec!["n3".to_string()]);
}

/// Extended protocol DML: a parameterized `UPDATE … WHERE id = $1` applies, and a
/// follow-up SELECT shows the change. Proves a parameterized write over the real
/// driver path routes through the GraphTxn CAS write path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extended_parameterized_update_then_select() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    let affected = client
        .execute("UPDATE nodes SET rank = $1 WHERE id = $2", &[&42i64, &"n1"])
        .await
        .expect("parameterized UPDATE");
    assert_eq!(affected, 1, "one row updated");

    let rows = client
        .query("SELECT id FROM nodes WHERE rank = $1", &[&42i64])
        .await
        .expect("SELECT after update");
    let ids: Vec<String> = rows.iter().map(|r| r.get::<_, String>(0)).collect();
    assert_eq!(ids, vec!["n1".to_string()]);
}

/// Extended protocol DML: a parameterized `DELETE … WHERE id = $1` removes the
/// node; a follow-up SELECT shows it gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extended_parameterized_delete_then_select() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    let affected = client
        .execute("DELETE FROM nodes WHERE id = $1", &[&"n2"])
        .await
        .expect("parameterized DELETE");
    assert_eq!(affected, 1, "one row deleted");

    let rows = client
        .query("SELECT id FROM nodes WHERE id = $1", &[&"n2"])
        .await
        .expect("SELECT after delete");
    assert_eq!(rows.len(), 0, "deleted node must be gone");
}

/// Multi-row INSERT (CONCEPT:MEDULLA-KG.query.follow-up): `INSERT … VALUES (…),(…),(…)` adds three
/// nodes in one statement; a COUNT confirms all landed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_row_insert_then_count() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    let affected = client
        .execute(
            "INSERT INTO nodes (id, type, rank, _visibility, _owner) VALUES \
             ('m1','Agent',10,'public','tester'),('m2','Agent',11,'public','tester'),\
             ('m3','Tool',12,'public','tester')",
            &[],
        )
        .await
        .expect("multi-row INSERT");
    assert_eq!(affected, 3, "three rows inserted");

    let rows = client
        .query("SELECT count(*) AS c FROM nodes WHERE rank >= 10", &[])
        .await
        .expect("COUNT after multi-insert");
    let c: i64 = rows[0].get("c");
    assert_eq!(c, 3, "all three multi-insert rows present");
}

/// `INSERT … RETURNING id` (CONCEPT:MEDULLA-KG.query.follow-up): the write returns the affected row
/// as a result set, not just a CommandComplete tag.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn insert_returning_row() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    let rows = client
        .query(
            "INSERT INTO nodes (id, type, rank, _visibility, _owner) VALUES ('r1','Agent',55,'public', 'tester') RETURNING id",
            &[],
        )
        .await
        .expect("INSERT RETURNING");
    assert_eq!(rows.len(), 1, "RETURNING yields one row");
    assert_eq!(rows[0].get::<_, String>("id"), "r1".to_string());

    // UPDATE … RETURNING surfaces the post-write value.
    let urows = client
        .query(
            "UPDATE nodes SET rank = $1 WHERE id = $2 RETURNING id, rank",
            &[&77i64, &"r1"],
        )
        .await
        .expect("UPDATE RETURNING");
    assert_eq!(urows.len(), 1);
    assert_eq!(urows[0].get::<_, String>("id"), "r1".to_string());
    assert_eq!(urows[0].get::<_, i64>("rank"), 77i64);
}

/// A recording `PersistenceBackend` that captures every `record_durable` call.
#[derive(Default)]
struct RecordingBackend {
    recorded: std::sync::Mutex<Vec<(String, String)>>,
    durable: std::sync::atomic::AtomicUsize,
}

impl RecordingBackend {
    /// `(graph_fname, node_id)` pairs captured via the durable commit seam.
    fn recorded_pairs(&self) -> Vec<(String, String)> {
        self.recorded.lock().unwrap().clone()
    }
    fn durable_calls(&self) -> usize {
        self.durable.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl PersistenceBackend for RecordingBackend {
    async fn load_all(&self, _state: &Arc<RwLock<ServerState>>) -> Result<usize, String> {
        Ok(0)
    }
    async fn record_durable(
        &self,
        graph_fname: &str,
        method: &medulla_protocol_types::protocol::Method,
    ) -> Result<(), String> {
        self.durable
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let medulla_protocol_types::protocol::Method::AddNode { node_id, .. } = method {
            self.recorded
                .lock()
                .unwrap()
                .push((graph_fname.to_string(), node_id.clone()));
        }
        Ok(())
    }
    // Wire graph-node writes now route through the universal cross-modal
    // MutationBatch kernel (`medulla_h_txn::txn::commit_cross_modal_txn`), not
    // a bare `record_durable` call — the trait's default `commit_mutation_batch_crossmodal`
    // fails closed ("persistence backend does not support atomic cross-modal
    // MutationBatch commits"), which would defeat this backend's whole purpose
    // (proving the durable seam is AWAITED before ack). Override it to record the
    // SAME (graph, node_id) pairs `record_durable` used to, off the batch's staged
    // `AddNode` operations, and hand back a minimal committed record — this is the
    // authoritative-commit-flip seam this mock must speak now.
    async fn commit_mutation_batch_crossmodal(
        &self,
        args: medulla_server_api::api::persistence::CrossModalCommitArgs<'_>,
    ) -> Result<medulla_protocol_types::mutation_batch::MutationBatchCommit, String> {
        self.durable
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        for method in args.methods {
            if let medulla_protocol_types::protocol::Method::AddNode { node_id, .. } = method {
                self.recorded
                    .lock()
                    .unwrap()
                    .push((args.graph_fname.to_string(), node_id.clone()));
            }
        }
        // MutationBatch v1 (`1d83cf4b`) added the self-checking `identity`
        // envelope copies and the exact `committed_version` transition. Derive
        // both the way the landed production commit does
        // (`redb_store::write_mutation_batch_commit_rows`): the identity is the
        // batch's own identity (`validate_identity` requires exact equality at
        // every level), and the committed version is the OCC expectation's
        // `expected -> expected + 1` transition. A wire graph write is always
        // `VersionExpectation::Graph`, and production fails closed on anything
        // else, so this mock does too rather than stubbing a default.
        let committed_version = match args.batch.version_expectation {
            medulla_protocol_types::mutation_batch::VersionExpectation::Graph(expected) => {
                medulla_mutation_types::mutation_scope::CommittedVersion::checked_graph(expected)?
            }
            other => {
                return Err(format!(
                    "graph MutationBatch requires a graph version expectation, got {other:?}"
                ))
            }
        };
        Ok(
            medulla_protocol_types::mutation_batch::MutationBatchCommit {
                record: medulla_protocol_types::mutation_batch::MutationBatchRecord {
                    batch: args.batch.clone(),
                    identity: args.batch.identity.clone(),
                    status: medulla_protocol_types::mutation_batch::MutationBatchStatus::Committed,
                    committed_version,
                    result_msgpack: args.result_msgpack.map(|b| b.to_vec()),
                    committed_at_ms: args.committed_at_ms,
                },
                identity: args.batch.identity.clone(),
                replayed: false,
            },
        )
    }
    fn shutdown(&self) {}
}

/// A pgwire INSERT awaits the durable backend before reporting completion.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_insert_awaits_durable_backend() {
    let backend = Arc::new(RecordingBackend::default());
    let state = state_with(Some(backend.clone()));
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    let insert = client
        .simple_query("INSERT INTO nodes (id, type, rank) VALUES ('w1', 'Agent', 4)")
        .await
        .expect("INSERT");
    let affected = command_count(&insert);
    assert_eq!(affected, 1);

    assert_eq!(
        backend.recorded_pairs(),
        vec![("__commons__".to_string(), "w1".to_string())],
        "the durable commit must receive the pgwire INSERT"
    );
    assert_eq!(
        backend.durable_calls(),
        1,
        "record_durable must be awaited before completion"
    );
}

/// Authoritative durability (CONCEPT:MEDULLA-KG.query.concept-10): a pgwire INSERT is
/// commit-before-ack — `record_durable` is awaited before the
/// CommandComplete is sent. We prove the wire write is durable WITHOUT any checkpoint
/// by reading the row back from a SEPARATE redb backend reopened on the same dir: the
/// row is only there because the INSERT's await observed a durable commit, exactly
/// like a normal `Method::AddNode` write. The row is read only after the
/// group-commit barrier has fired for the awaited writer.
#[cfg(feature = "redb")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_insert_authoritative_is_durable_without_checkpoint() {
    let (dir_s, backend) = interval_backend("durable");
    let state = state_with_dir(Some(backend.clone()), dir_s.clone());
    let (addr, listener) = spawn_listener_abortable(state).await;
    let client = connect(&addr).await;

    // CommandComplete is returned only after the durable group commit.
    let insert = client
        .simple_query("INSERT INTO nodes (id, type, rank) VALUES ('d1', 'Agent', 7)")
        .await
        .expect("INSERT");
    let affected = command_count(&insert);
    assert_eq!(affected, 1);

    // The wire write was acked ⇒ (commit-before-ack) it is on disk. Prove it is
    // durable independent of any checkpoint by reopening a FRESH redb backend on the
    // SAME dir and point-reading the node — no bulk registry export was involved.
    // redb's advisory file lock is held for as long as ANY `Arc<RedbBackend>` clone
    // survives, not just the writer thread `shutdown()` stops — the listener task
    // (and its per-connection handler for `client`) each captured their own clone via
    // `state.persistence`, so those must actually be torn down first: close the
    // client connection, abort + await the listener task, THEN shut down our own
    // local clone (the retry loop below absorbs the async race of the per-connection
    // handler noticing the closed socket).
    drop(client);
    listener.abort();
    let _ = listener.await;
    backend.shutdown();
    drop(backend);
    let (reopened, stored) = reopened_node(&dir_s, "d1").await;
    assert!(
        stored.is_some(),
        "pgwire authoritative INSERT must be durable in redb WITHOUT a checkpoint"
    );
    // The stored blob round-trips the inserted properties.
    let props: serde_json::Value =
        rmp_serde::from_slice(&stored.unwrap()).expect("decode stored node");
    assert_eq!(props.get("type").and_then(|v| v.as_str()), Some("Agent"));
    assert_eq!(props.get("rank").and_then(|v| v.as_i64()), Some(7));

    reopened.shutdown();
    let _ = std::fs::remove_dir_all(&dir_s);
}

/// Extended-protocol durable-on-ack (CONCEPT:MEDULLA-KG.query.describe + KG-2.198): a parameterized
/// `UPDATE … WHERE id = $1` issued over the extended protocol
/// mode is commit-before-ack — `record_durable` (a `CompareAndSetNodeFields` method)
/// is AWAITED before the client's `execute()` returns. We prove the wire write is
/// durable WITHOUT any checkpoint by reopening a FRESH redb backend on the same dir
/// and reading the updated property back. This is the headline guarantee that a real
/// driver's prepared write never acks before it lands on disk.
#[cfg(feature = "redb")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extended_update_authoritative_is_durable_without_checkpoint() {
    let (dir_s, backend) = interval_backend("extended-durable");
    let state = state_with_dir(Some(backend.clone()), dir_s.clone());
    let (addr, listener) = spawn_listener_abortable(state).await;
    let client = connect(&addr).await;

    // Seed `n1` durably via a wire INSERT (also commit-before-ack), then UPDATE it
    // over the extended/prepared protocol with a bound parameter.
    client
        .execute(
            "INSERT INTO nodes (id, type, rank, _visibility, _owner) VALUES ('n1','Agent',1,'public', 'tester')",
            &[],
        )
        .await
        .expect("seed INSERT");
    let affected = client
        .execute("UPDATE nodes SET rank = $1 WHERE id = $2", &[&99i64, &"n1"])
        .await
        .expect("extended UPDATE");
    assert_eq!(affected, 1);

    // The UPDATE was acked ⇒ (commit-before-ack) the CAS is on disk. Reopen a fresh
    // backend on the same dir and confirm the updated value WITHOUT any checkpoint.
    // See the identical teardown note in `wire_insert_authoritative_is_durable_without_checkpoint`:
    // redb's file lock survives until every `Arc<RedbBackend>` clone (including the
    // listener task's and its per-connection handler's) is actually dropped.
    drop(client);
    listener.abort();
    let _ = listener.await;
    backend.shutdown();
    drop(backend);
    let (reopened, stored) = reopened_node(&dir_s, "n1").await;
    assert!(
        stored.is_some(),
        "extended-protocol authoritative UPDATE must be durable in redb WITHOUT a checkpoint"
    );
    let props: serde_json::Value =
        rmp_serde::from_slice(&stored.unwrap()).expect("decode stored node");
    assert_eq!(
        props.get("rank").and_then(|v| v.as_i64()),
        Some(99),
        "the UPDATE's new value must be the durably-committed one"
    );

    reopened.shutdown();
    let _ = std::fs::remove_dir_all(&dir_s);
}

// ───────────────────────────────────────────────────────────────────────────
// Synthetic catalog (CONCEPT:MEDULLA-KG.query.datafusion) — a real driver introspects on connect.
// A tokio-postgres client lists tables via information_schema, reflects the
// `nodes` columns, calls version()/current_schema(), then SELECTs — all succeed.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_introspection_then_select() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    // 1. List tables via information_schema (what SQLAlchemy get_table_names issues).
    let rows = client
        .query(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_name IN ('nodes','edges') ORDER BY table_name",
            &[],
        )
        .await
        .expect("information_schema.tables");
    let tables: Vec<String> = rows.iter().map(|r| r.get::<_, String>(0)).collect();
    assert_eq!(tables, vec!["edges".to_string(), "nodes".to_string()]);

    // 2. Reflect the `nodes` columns (what SQLAlchemy get_columns issues).
    let rows = client
        .query(
            "SELECT column_name FROM information_schema.columns \
             WHERE table_name = 'nodes' ORDER BY column_name",
            &[],
        )
        .await
        .expect("information_schema.columns");
    let cols: Vec<String> = rows.iter().map(|r| r.get::<_, String>(0)).collect();
    for must in ["id", "rank", "type"] {
        assert!(
            cols.contains(&must.to_string()),
            "reflect missing {must}: {cols:?}"
        );
    }

    // 3. pg_catalog reflect: list relations under public via pg_class/pg_namespace.
    // Filter to the graph projections (`nodes`/`edges`) — the user-table SQL store is
    // a process-global singleton, so a sibling test's user table can otherwise appear
    // here; this mirrors step 1's `information_schema` query, which also filters.
    let rows = client
        .query(
            "SELECT c.relname FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'public' AND c.relkind = 'r' \
             AND c.relname IN ('nodes','edges') ORDER BY c.relname",
            &[],
        )
        .await
        .expect("pg_catalog.pg_class");
    let rels: Vec<String> = rows.iter().map(|r| r.get::<_, String>(0)).collect();
    assert_eq!(rels, vec!["edges".to_string(), "nodes".to_string()]);

    // 4. Catalog scalar functions a driver probes on connect.
    let v = client
        .query("SELECT version()", &[])
        .await
        .expect("version()");
    assert!(v[0].get::<_, String>(0).starts_with("PostgreSQL"));
    let s = client
        .query("SELECT current_schema()", &[])
        .await
        .expect("current_schema()");
    assert_eq!(s[0].get::<_, String>(0), "public");

    // 5. After introspection, a normal SELECT still works.
    let rows = client
        .query("SELECT id FROM nodes WHERE rank >= 2 ORDER BY id", &[])
        .await
        .expect("post-introspection SELECT");
    let ids: Vec<String> = rows.iter().map(|r| r.get::<_, String>(0)).collect();
    assert_eq!(ids, vec!["n2".to_string(), "n3".to_string()]);
}

// ───────────────────────────────────────────────────────────────────────────
// pg-wire SCRAM auth bridged to the engine identity (CONCEPT:MEDULLA-KG.query.concept-13).
// A SCRAM login succeeds with the derived password, is REJECTED with the wrong
// one, and post-login queries run under the mapped engine AgentIdentity/ACL.
// ───────────────────────────────────────────────────────────────────────────

use medulla_wire::pgwire::derive_pg_password;

/// A SCRAM-auth state: a known engine secret + a registered identity so the
/// IsolationLayer has rules (the ACL is enforced once any identity exists). The
/// `worker` agent owns `agent:worker` and is a member of no team; `__commons__`
/// stays open to all authenticated agents.
fn scram_state(secret: &str) -> Arc<RwLock<ServerState>> {
    let mut registry = GraphRegistry::new();
    test_support::seed_wire_nodes(&registry, "worker");
    // A private per-agent graph owned by `worker`, plus a peer's private graph.
    registry
        .create_graph(
            "agent:worker",
            medulla_protocol_types::protocol::GraphType::Agent,
            Some("worker".to_string()),
        )
        .unwrap();
    registry
        .create_graph(
            "agent:peer",
            medulla_protocol_types::protocol::GraphType::Agent,
            Some("peer".to_string()),
        )
        .unwrap();

    let mut isolation = test_support::wire_isolation(&["worker", "peer"]);
    // `__commons__` stays reachable by every authenticated agent via the
    // `commons-user` role (there is no more pre-RBAC "Commons is open to all"
    // fall-through under `feature = "security"`).
    // Each agent additionally owns its own private `agent:*` graph — granted
    // explicitly (mirrors `medulla_server::mod::tests::multi_tenant_state`'s `owner-{w}`
    // pattern) so `scram_identity_drives_acl` can prove `worker` reaches its own
    // graph while still being denied `peer`'s.
    #[cfg(feature = "security")]
    {
        use medulla_identity_types::acl::{Grant, GrantEffect, RbacAction, ResourceSelector, Role};
        for (owner, graph) in [("worker", "agent:worker"), ("peer", "agent:peer")] {
            let role = format!("owner-{owner}");
            isolation.add_role(Role::new(role.clone()));
            for action in [RbacAction::Read, RbacAction::Write] {
                isolation.add_grant(Grant {
                    role: role.clone(),
                    resource: ResourceSelector::Graph(graph.to_string()),
                    action,
                    effect: GrantEffect::Allow,
                });
            }
        }
    }
    isolation.register_agent(AgentIdentity {
        agent_id: "worker".to_string(),
        role: AgentRole::Agent,
        teams: vec![],
        roles: vec!["commons-user".to_string(), "owner-worker".to_string()],
    });
    isolation.register_agent(AgentIdentity {
        agent_id: "peer".to_string(),
        role: AgentRole::Agent,
        teams: vec![],
        roles: vec!["commons-user".to_string(), "owner-peer".to_string()],
    });

    test_support::state_with_registry(
        secret,
        isolation,
        registry,
        Some(sql_test_persist_dir()),
        None,
    )
}

/// Connect with explicit user + password (SCRAM negotiated by tokio-postgres).
async fn connect_scram(
    addr: &str,
    user: &str,
    password: &str,
    dbname: &str,
) -> Result<tokio_postgres::Client, tokio_postgres::Error> {
    test_support::connect_pgwire_with_password(addr, password, user, dbname).await
}

async fn scram_client(secret: &str, addr: &str, user: &str) -> tokio_postgres::Client {
    let password = derive_pg_password(secret, user);
    connect_scram(addr, user, &password, "__commons__")
        .await
        .expect("SCRAM login")
}

/// SCRAM login SUCCEEDS with the derived password and a post-login query runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scram_login_succeeds_with_correct_password() {
    let secret = "dummy-pg-test-secret"; // sanitizer:ignore
    let state = scram_state(secret);
    let addr = spawn_listener_mode(state, pgwire::PgWireAuthMode::Scram).await;

    let client = scram_client(secret, &addr, "worker").await;

    // A query after login runs (against the open __commons__). `rank` is inferred
    // as a flat typed column off the seeded property blobs (the same convention
    // `wire_select_returns_seeded_rows`/`extended_prepared_parameterized_select`
    // rely on elsewhere in this file) — no JSON deep-index accessor is needed. The
    // `props->>'k'` operator form is only wired for the narrow graph-node
    // UPDATE/DELETE selector decoder (`medulla_sql::sql::classify`), not the general
    // DataFusion-planned SELECT path this query runs on, so using it here panicked
    // with "Operator ->> is not yet supported".
    let rows = client
        .query("SELECT id FROM nodes WHERE rank = 1", &[])
        .await
        .expect("post-login SELECT");
    let ids: Vec<String> = rows.iter().map(|r| r.get::<_, String>(0)).collect();
    assert_eq!(ids, vec!["n1".to_string()]);
}

/// SCRAM login is REJECTED with the wrong password.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scram_login_rejected_with_wrong_password() {
    let secret = "dummy-pg-test-secret"; // sanitizer:ignore
    let state = scram_state(secret);
    let addr = spawn_listener_mode(state, pgwire::PgWireAuthMode::Scram).await;

    let err = connect_scram(&addr, "worker", "not-the-derived-password", "__commons__")
        .await
        .expect_err("wrong password must be rejected");
    // The SASL failure surfaces as a db error; inspect the full source chain so the
    // assertion catches the auth-failure SQLSTATE/message regardless of wrapping.
    let chain = error_chain(&err);
    assert!(
        chain.contains("password")
            || chain.contains("auth")
            || chain.contains("sasl")
            || chain.contains("28p01")
            || chain.contains("28000"),
        "expected an auth failure, got chain: {chain}"
    );
}

/// The post-login identity drives the ACL: `worker` reaches its OWN private graph
/// (`agent:worker`) but is DENIED a peer's private graph (`agent:peer`). Proves the
/// pg user → engine AgentIdentity mapping flows into `IsolationLayer::check_access`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scram_identity_drives_acl() {
    let secret = "dummy-pg-test-secret"; // sanitizer:ignore
    let state = scram_state(secret);
    let addr = spawn_listener_mode(state, pgwire::PgWireAuthMode::Scram).await;

    let client = scram_client(secret, &addr, "worker").await;

    // Owner reaches its own private graph.
    client
        .simple_query("SET graph = 'agent:worker'")
        .await
        .expect("SET graph to own");
    client
        .query("SELECT id FROM nodes", &[])
        .await
        .expect("read own private graph");

    // The same connection is DENIED a peer's private graph.
    client
        .simple_query("SET graph = 'agent:peer'")
        .await
        .expect("SET graph to peer (the SET itself is local)");
    let denied = client.query("SELECT id FROM nodes", &[]).await;
    let err = denied.expect_err("peer graph read must be denied");
    let chain = error_chain(&err);
    assert!(
        chain.contains("permission denied") || chain.contains("42501"),
        "expected an ACL denial, got chain: {chain}"
    );
}

/// Render a tokio-postgres error's full source chain lowercased, so an assertion
/// can match a SQLSTATE/message that the top-level `Display` hides behind a generic
/// "db error" wrapper.
fn error_chain(err: &tokio_postgres::Error) -> String {
    use std::error::Error;
    let mut s = err.to_string();
    let mut src = err.source();
    while let Some(e) = src {
        s.push_str(" | ");
        s.push_str(&e.to_string());
        src = e.source();
    }
    s.to_lowercase()
}

// ───────────────────────────────────────────────────────────────────────────
// Mixed-store wire transactions (CONCEPT:MEDULLA-KG.compute.kg-transaction-is-pinned) — BEGIN/COMMIT/ROLLBACK over the
// simple-query protocol, on ONE persistent connection (== one EngineBackend).
// ───────────────────────────────────────────────────────────────────────────

/// Pull the first-column values from a `simple_query` result as strings.
fn simple_ids(msgs: Vec<tokio_postgres::SimpleQueryMessage>) -> Vec<String> {
    test_support::simple_ids(msgs)
}

fn command_count(msgs: &[tokio_postgres::SimpleQueryMessage]) -> i64 {
    msgs.iter()
        .find_map(|m| match m {
            // tokio-postgres yields the affected-row count as u64; this helper's
            // callers compare against signed literals, so narrow once here.
            tokio_postgres::SimpleQueryMessage::CommandComplete(n) => Some(*n as i64),
            _ => None,
        })
        .expect("INSERT CommandComplete")
}

/// A unique user-table name (the SQL table store is a process-global file, so a
/// fixed name could collide with a prior run / a sibling test).
fn unique_table() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("kv_{nanos}")
}

/// ROLLBACK discards a buffered node INSERT, and a SELECT inside the txn sees its
/// own uncommitted write first (read-your-own-writes). Proves the `T` in-transaction
/// status (the txn is live and buffering) and clean discard on ROLLBACK.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_txn_rollback_discards_and_ryow() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    client.simple_query("BEGIN").await.expect("BEGIN");
    client
        .simple_query(
            "INSERT INTO nodes (id, type, rank, _visibility, _owner) VALUES ('tx1', 'Agent', 42, 'public', 'tester')",
        )
        .await
        .expect("INSERT inside txn");
    // Read-your-own-writes: the uncommitted insert IS visible inside the txn.
    let seen = simple_ids(
        client
            .simple_query("SELECT id FROM nodes WHERE rank = 42")
            .await
            .expect("SELECT inside txn"),
    );
    assert_eq!(
        seen,
        vec!["tx1".to_string()],
        "read-your-own-writes: buffered insert visible inside the txn"
    );

    client.simple_query("ROLLBACK").await.expect("ROLLBACK");

    // After ROLLBACK the buffered insert is gone.
    let after = simple_ids(
        client
            .simple_query("SELECT id FROM nodes WHERE rank = 42")
            .await
            .expect("SELECT after ROLLBACK"),
    );
    assert!(
        after.is_empty(),
        "ROLLBACK discarded the buffered insert, got {after:?}"
    );
}

/// COMMIT applies a buffered node INSERT so it survives the transaction and is seen
/// by a post-COMMIT SELECT.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_txn_commit_persists_node_insert() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    client.simple_query("BEGIN").await.expect("BEGIN");
    client
        .simple_query(
            "INSERT INTO nodes (id, type, rank, _visibility, _owner) VALUES ('tx2', 'Agent', 43, 'public', 'tester')",
        )
        .await
        .expect("INSERT inside txn");
    client.simple_query("COMMIT").await.expect("COMMIT");

    let after = simple_ids(
        client
            .simple_query("SELECT id FROM nodes WHERE rank = 43")
            .await
            .expect("SELECT after COMMIT"),
    );
    assert_eq!(
        after,
        vec!["tx2".to_string()],
        "COMMIT applied the buffered node insert"
    );
}

/// A mixed transaction commits BOTH graph-node ops AND user-table ops atomically
/// (sequenced across the two stores).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_txn_mixed_node_and_table_commit() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;
    let table = unique_table();

    client.simple_query("BEGIN").await.expect("BEGIN");
    client
        .simple_query(&format!("CREATE TABLE {table} (k TEXT, v BIGINT)"))
        .await
        .expect("CREATE TABLE inside txn");
    client
        .simple_query(&format!("INSERT INTO {table} (k, v) VALUES ('a', 1)"))
        .await
        .expect("INSERT table inside txn");
    client
        .simple_query(
            "INSERT INTO nodes (id, type, rank, _visibility, _owner) VALUES ('mx1', 'Agent', 44, 'public', 'tester')",
        )
        .await
        .expect("INSERT node inside txn");
    client.simple_query("COMMIT").await.expect("COMMIT");

    // Both stores reflect the committed transaction.
    let node = simple_ids(
        client
            .simple_query("SELECT id FROM nodes WHERE rank = 44")
            .await
            .expect("SELECT node after COMMIT"),
    );
    assert_eq!(node, vec!["mx1".to_string()], "node op committed");

    let table_rows = simple_ids(
        client
            .simple_query(&format!("SELECT k FROM {table}"))
            .await
            .expect("SELECT table after COMMIT"),
    );
    assert_eq!(table_rows, vec!["a".to_string()], "table op committed");

    // Clean up: the SQL table store is a process-global singleton, so drop the table
    // to avoid leaking it into other tests / future runs.
    let _ = client.simple_query(&format!("DROP TABLE {table}")).await;
}

/// An error inside an open transaction latches it into the aborted state: every
/// later statement except COMMIT/ROLLBACK is rejected with SQLSTATE 25P02 (the `E`
/// failed-transaction status), and ROLLBACK returns the connection to a usable idle
/// state (`I`) where writes work again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_txn_error_blocks_until_rollback_25p02() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    client.simple_query("BEGIN").await.expect("BEGIN");
    // A statement that errors inside the txn (unknown table) → aborts the txn.
    let bad = client.simple_query("SELECT id FROM no_such_table").await;
    assert!(bad.is_err(), "the bad statement must error");

    // Every subsequent non-COMMIT/ROLLBACK statement is now rejected with 25P02.
    let blocked = client
        .simple_query("INSERT INTO nodes (id, type, rank) VALUES ('nope', 'Agent', 45)")
        .await
        .expect_err("statement after an in-txn error must be rejected");
    let chain = error_chain(&blocked);
    assert!(
        chain.contains("25p02") || chain.contains("aborted"),
        "expected 25P02 aborted-transaction error, got: {chain}"
    );

    // ROLLBACK ends the block and restores a usable connection.
    client.simple_query("ROLLBACK").await.expect("ROLLBACK");
    client
        .simple_query(
            "INSERT INTO nodes (id, type, rank, _visibility, _owner) VALUES ('ok', 'Agent', 46, 'public', 'tester')",
        )
        .await
        .expect("writes work again after ROLLBACK");
    let ok = simple_ids(
        client
            .simple_query("SELECT id FROM nodes WHERE rank = 46")
            .await
            .expect("SELECT after recovery"),
    );
    assert_eq!(
        ok,
        vec!["ok".to_string()],
        "connection usable after ROLLBACK"
    );
    // The aborted statement never landed.
    let nope = simple_ids(
        client
            .simple_query("SELECT id FROM nodes WHERE rank = 45")
            .await
            .expect("SELECT for the blocked row"),
    );
    assert!(nope.is_empty(), "the blocked insert never applied");
}

/// SET graph is rejected while a transaction is open (a txn stays within one graph /
/// redb shard, CONCEPT:MEDULLA-KG.sharding.semantic-embedding-store-backed).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_txn_set_graph_rejected_while_open() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    client.simple_query("BEGIN").await.expect("BEGIN");
    let denied = client.simple_query("SET graph = 'other'").await;
    let err = denied.expect_err("SET graph inside a txn must be rejected");
    let chain = error_chain(&err);
    assert!(
        chain.contains("transaction"),
        "expected a 'within one graph / transaction' rejection, got: {chain}"
    );
    client.simple_query("ROLLBACK").await.expect("ROLLBACK");
}

// ───────────────────────────────────────────────────────────────────────────
// FEATURE-GAP SEAM SPECS (ignored executable specs) — the audit's cross-modal
// write→read seams that are NOT yet built. Each is written to the INTENDED
// contract and `#[ignore]`d with the precise missing seam, so it flips green when
// the seam lands. They COMPILE against the real tokio-postgres client path.
// ───────────────────────────────────────────────────────────────────────────

/// SEAM (CONCEPT:MEDULLA-KG.txn.isolation-ryow-begin-set): inside an open transaction, staged writes are visible to a
/// same-connection *SQL* read (read-your-own-writes — proven by
/// `wire_txn_rollback_discards_and_ryow`) AND to an in-txn cross-modal `UQL …` read over
/// the PGWIRE TEXT protocol — the entrypoint this PR adds onto the committed MEDULLA-359 RPC
/// seam. The UQL is genuinely cross-modal: its `MATCH … WHERE rank = 99` reads the staged
/// UPDATE (graph modality, RYOW) and its `RANK BY ~[…]` reads the staged embedding (vector
/// modality, RYOW) — both overlaid onto the committed snapshot by the shared `run_unified`
/// overlay. (`RANK BY` retains only rows carrying an embedding — the documented medulla-plan
/// hybrid-metadata-filter invariant — so the row the UPDATE touches is ALSO given a staged
/// embedding, which is what makes the filter→rank return it.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_txn_update_then_cross_modal_read() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    client.simple_query("BEGIN").await.expect("BEGIN");

    // Stage a graph write (UPDATE) AND a vector write (SET EMBEDDING) inside the txn —
    // neither committed yet. The cross-modal UQL below must read BOTH.
    client
        .execute("UPDATE nodes SET rank = $1 WHERE id = $2", &[&99i64, &"n1"])
        .await
        .expect("staged in-txn UPDATE");
    client
        .simple_query("SET EMBEDDING FOR 'n1' = '[1.0, 0.0, 0.0]'")
        .await
        .expect("staged in-txn embedding");

    // GREEN today: a plain-SQL RYOW read sees the staged write (buffered write path).
    let sql_ryow = simple_ids(
        client
            .simple_query("SELECT id FROM nodes WHERE rank = 99")
            .await
            .expect("plain-SQL RYOW inside txn"),
    );
    assert_eq!(
        sql_ryow,
        vec!["n1".to_string()],
        "plain SQL RYOW works today"
    );

    // THE SEAM: a CROSS-MODAL / UQL read issued over the wire ALSO reflects the staged
    // writes — a unified filter→rank over the staged row returns `n1` (RYOW across the
    // graph AND vector modalities before COMMIT).
    let unified = simple_ids(
        client
            .simple_query("UQL MATCH (:Agent) WHERE rank = 99 |> RANK BY ~[1.0,0.0,0.0] |> LIMIT 5")
            .await
            .expect("in-txn cross-modal/UQL read over the wire"),
    );
    assert_eq!(
        unified,
        vec!["n1".to_string()],
        "the cross-modal read must see the same-txn staged UPDATE + embedding (read-your-own-writes)"
    );

    client.simple_query("COMMIT").await.expect("COMMIT");
}

/// SEAM (CONCEPT:MEDULLA-KG.txn.isolation-ryow-begin-set): a single transaction that stages FIVE modalities over the
/// PGWIRE TEXT protocol — a graph node (`INSERT INTO nodes`), a vector embedding
/// (`SET EMBEDDING FOR`), a timeseries point (`INSERT INTO series`), an OWL/SPARQL axiom
/// (`SPARQL UPDATE INSERT DATA`), and a `SPARQL CONSTRUCT` — then COMMITs them atomically
/// through the shared RPC cross-modal commit (all modalities in ONE redb
/// `WriteTransaction`), and reads the committed result with a cross-modal UQL join over
/// the graph + vector + timeseries legs. This is the pgwire entrypoint for the MEDULLA-360/361/
/// 362 staging seam + MEDULLA-363 planner reach the RPC transport already proved. (The final
/// read uses `MATCH (:Sensor) |> RANK BY …`. The string-type↔IRI-class bridge for a
/// `REASON <iri>` over the committed axiom is now BUILT — MEDULLA-375/376 — and proved over the
/// wire by `wire_reason_iri_bridges_string_typed_node` below under the reasoner-exec build
/// layer; the in-txn tsdb-RYOW overlay is likewise built, MEDULLA-374, see `docs/north_star.md`.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_txn_mixed_five_modality_commit() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    client.simple_query("BEGIN").await.expect("BEGIN");

    // (1) graph node.
    client
        .simple_query(
            "INSERT INTO nodes (id, type, rank, _visibility, _owner) VALUES ('mm1', 'Sensor', 7, 'public', 'tester')",
        )
        .await
        .expect("stage graph node");
    // (2) vector embedding for the new node (a wire verb the seam must add).
    client
        .simple_query("SET EMBEDDING FOR 'mm1' = '[1.0, 0.0, 0.0]'")
        .await
        .expect("stage vector embedding");
    // (3) timeseries point (tsdb staging the seam must add to txn.rs).
    client
        .simple_query("INSERT INTO series (id, ts, value) VALUES ('mm1', 1000, 42.0)")
        .await
        .expect("stage timeseries point");
    // (4) OWL/SPARQL axiom UPDATE (OWL-update-in-txn the seam must add).
    client
        .simple_query(
            "SPARQL UPDATE INSERT DATA { <http://ex/Sensor> \
             <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex/Device> }",
        )
        .await
        .expect("stage OWL axiom update");
    // (5) CONSTRUCT-materialized triple (CONSTRUCT-in-txn the seam must add).
    client
        .simple_query(
            "SPARQL CONSTRUCT { ?s <http://ex/kind> <http://ex/Device> } \
             WHERE { ?s a <http://ex/Sensor> }",
        )
        .await
        .expect("stage CONSTRUCT materialization");

    client.simple_query("COMMIT").await.expect("COMMIT");

    // A cross-modal UQL join reads the just-committed txn: the new Sensor node (graph)
    // ranked by its committed embedding (vector). (The in-txn tsdb-RYOW overlay — a
    // `TsScan` reading the txn's OWN staged points — is now built, MEDULLA-374; the staged
    // measurement above still commits atomically here.)
    let joined = simple_ids(
        client
            .simple_query("UQL MATCH (:Sensor) |> RANK BY ~[1.0,0.0,0.0] |> LIMIT 5")
            .await
            .expect("cross-modal UQL join over the committed five-modality txn"),
    );
    assert_eq!(
        joined,
        vec!["mm1".to_string()],
        "the staged modalities join in one UQL read over the committed txn"
    );
}

/// SEAM (CONCEPT:MEDULLA-KG.txn.isolation-ryow-begin-set) — isolation + RYOW: a `BEGIN; SET EMBEDDING; INSERT INTO series;
/// <UQL join over graph+vector+tsdb>; COMMIT` reads its OWN writes inside the txn, while a
/// SECOND connection sees NONE of them until COMMIT (the cross-modal writes are staged, not
/// committed). After COMMIT the second connection reads the committed cross-modal state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_txn_crossmodal_ryow_isolated_until_commit() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let writer = connect(&addr).await;
    let reader = connect(&addr).await;

    // The UQL cross-modal join used by both connections: a Widget node filtered on the
    // graph modality and ranked by the query embedding (graph + vector legs). The
    // timeseries WRITE is exercised below; the in-txn tsdb-RYOW READ overlay (a `TsScan`
    // reading the txn's own staged points) is now built (MEDULLA-374) and proved by the
    // hand-built `medulla_plan::tsdb_scan_tests::tsdb_in_txn_ryow_staged_overlay`.
    let uql = "UQL MATCH (:Widget) WHERE rank = 5 |> RANK BY ~[1.0,0.0,0.0] |> LIMIT 5";

    writer.simple_query("BEGIN").await.expect("BEGIN");
    // Stage a graph node, its embedding, and a measurement — all inside the txn.
    writer
        .simple_query(
            "INSERT INTO nodes (id, type, rank, _visibility, _owner) VALUES ('vv1', 'Widget', 5, 'public', 'tester')",
        )
        .await
        .expect("stage graph node");
    writer
        .simple_query("SET EMBEDDING FOR 'vv1' = '[1.0, 0.0, 0.0]'")
        .await
        .expect("stage embedding");
    writer
        .simple_query("INSERT INTO series (id, ts, value) VALUES ('vv1', 1000, 9.0)")
        .await
        .expect("stage measurement");

    // RYOW: the writer's OWN in-txn UQL join sees its staged node + embedding.
    let in_txn = simple_ids(
        writer
            .simple_query(uql)
            .await
            .expect("in-txn cross-modal UQL"),
    );
    assert_eq!(
        in_txn,
        vec!["vv1".to_string()],
        "the writer reads its own staged cross-modal writes (RYOW)"
    );

    // Isolation: a SECOND connection (no open txn) sees NONE of the staged writes.
    let before = simple_ids(
        reader
            .simple_query(uql)
            .await
            .expect("off-txn cross-modal UQL"),
    );
    assert!(
        before.is_empty(),
        "a second connection must see none of the uncommitted cross-modal writes, got {before:?}"
    );

    writer.simple_query("COMMIT").await.expect("COMMIT");

    // After COMMIT the committed cross-modal state is visible to the second connection.
    let after = simple_ids(
        reader
            .simple_query(uql)
            .await
            .expect("post-commit cross-modal UQL"),
    );
    assert_eq!(
        after,
        vec!["vv1".to_string()],
        "after COMMIT the committed node + embedding are visible off-txn"
    );
}

/// SEAM (CONCEPT:MEDULLA-KG.query.reason-iri-parses-angle / MEDULLA-376) — `REASON <iri>` + the string-type↔IRI-class bridge over
/// the PGWIRE wire. Off-txn (each statement auto-commits as one atomic cross-modal write):
/// INSERT a bare-string-typed `Sensor` node, then stage the OWL axiom `<http://ex/Sensor>
/// ⊑ <http://ex/Device>` via `SPARQL UPDATE`. A `UQL MATCH (:Sensor) |> REASON
/// <http://ex/Device>` read then returns the Sensor node — the bare string `type`
/// `"Sensor"` is BRIDGED to the class IRI `<http://ex/Sensor>` (the target's namespace +
/// the local name) and reasoned into `<http://ex/Device>` through the committed subclass
/// axiom. This is the ORIGINAL intended `REASON <iri>` read the five-modality spec had to
/// avoid before the bridge existed. It needs the reasoner-exec build layer (`owl-plan` →
/// `medulla-plan/owl`, which compiles the `Op::Reason` executor the UQL clause lowers to); it
/// is in the `full` build (so the `cargo-test-full` / `clippy --features full` gates cover
/// it) but not the narrower `owl` feature alone.
#[cfg(feature = "owl-plan")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_reason_iri_bridges_string_typed_node() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    // A string-typed Sensor node (auto-commits as one atomic statement).
    client
        .simple_query(
            "INSERT INTO nodes (id, type, rank, _visibility, _owner) VALUES ('sen1', 'Sensor', 3, 'public', 'tester')",
        )
        .await
        .expect("insert string-typed Sensor node");
    // The OWL axiom Sensor ⊑ Device (SPARQL UPDATE auto-commits, lowered to graph triples
    // the reasoner reconstructs as TBox).
    //
    // FIXED (A18 -- was the KNOWN GAP this test used to document): the two axiom class
    // nodes (`<http://ex/Sensor>`, `<http://ex/Device>`) land with an EMPTY property
    // blob (`lower_triples`/`medulla_rdf::update::insert_triple` only populate ordinary
    // properties for a subject/object that also carries a literal-valued triple) --
    // untagged and unowned in the ABox sense. Row-level default-deny RLS
    // (`IsolationLayer::can_see_row`) exists to protect ABox rows, but an OWL axiom is
    // SCHEMA (TBox), not a row about anyone -- applying row-level default-deny to it was
    // a category error, and its consequence was exactly this: `REASON <http://ex/Device>`
    // never saw the axiom for the pgwire "tester" actor. `medulla_rdf::update::insert_triple`
    // (and `mapping::lower_triples` for bulk RDF load) now stamps BOTH endpoints of a
    // recognized RDFS/OWL schema predicate (`rdfs:subClassOf` here) with
    // `medulla_graph_core::rls::RLS_SCHEMA_KEY`, and `can_see_row` treats a schema-marked row
    // as visible -- narrowly: ONLY rows the RDF/OWL lowering structurally identified as
    // schema, never merely because they are untagged. Graph-level ACL
    // (`check_graph_access`, exercised in the SECOND connection below) is completely
    // unchanged and still gates whether an actor reaches row filtering at all.
    client
        .simple_query(
            "SPARQL UPDATE INSERT DATA { <http://ex/Sensor> \
             <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://ex/Device> }",
        )
        .await
        .expect("stage OWL subclass axiom");

    // REASON <http://ex/Device> over the wire: MATCH (:Sensor) sources the string-typed
    // node; the mid-plan REASON filters to the IRI class's members, and the bridge makes
    // the bare `"Sensor"` a member of `<http://ex/Device>`.
    let ids = simple_ids(
        client
            .simple_query("UQL MATCH (:Sensor) |> REASON <http://ex/Device> |> LIMIT 5")
            .await
            .expect("REASON <iri> read over the wire"),
    );
    assert_eq!(
        ids,
        vec!["sen1".to_string()],
        "REASON <http://ex/Device> includes the string-typed Sensor via the string→IRI bridge"
    );

    // ── A18, the OTHER direction: an actor with NO graph access sees NOTHING, TBox
    // included. `stranger` completes SCRAM login (the shared secret alone proves
    // possession; login never consults `IsolationLayer`), but is never
    // `register_agent`-ed, so `IsolationLayer::check_access` denies it before the
    // read even reaches row-level filtering -- proving the TBox exemption did not
    // widen the GRAPH-level gate this fix leaves untouched.
    let stranger_pw = pgwire::derive_pg_password("test", "stranger");
    let stranger = connect_scram(&addr, "stranger", &stranger_pw, "__commons__")
        .await
        .expect(
            "SCRAM login succeeds for an unregistered user (login proves only the shared secret)",
        );
    let denied = stranger
        .simple_query("UQL MATCH (:Sensor) |> REASON <http://ex/Device> |> LIMIT 5")
        .await;
    let err = denied.expect_err("an unregistered actor must be denied __commons__ graph access");
    let chain = error_chain(&err);
    assert!(
        chain.contains("permission denied") || chain.contains("42501"),
        "expected a graph-level ACL denial, got chain: {chain}"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// DIFFERENTIAL WIRE-SURFACE ORACLE (CONCEPT:MEDULLA-KG.query.pgwire-leg) — the pgwire leg of the
// query-surface harness. The in-process UQL-vs-builder-vs-oracle legs live in
// `crates/medulla-plan/tests/differential_oracle.rs`; this leg REUSES the live pgwire
// listener above to prove the SQL-wire and UQL-wire surfaces agree on the SAME query,
// so a caller reaches the identical result whichever door they speak.
// ───────────────────────────────────────────────────────────────────────────

/// The SAME relational filter expressed as plain SQL and as UQL over the pgwire text
/// protocol returns the IDENTICAL id set. `type='Agent' AND rank>1` (SQL) and
/// `MATCH (:Agent) WHERE rank > 1` (UQL) must both select exactly `n2` — one wire, two
/// surfaces, one answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_sql_and_uql_agree_on_same_filter() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    let mut sql = simple_ids(
        client
            .simple_query("SELECT id FROM nodes WHERE type = 'Agent' AND rank > 1")
            .await
            .expect("SQL filter"),
    );
    sql.sort();
    let mut uql = simple_ids(
        client
            .simple_query("UQL MATCH (:Agent) WHERE rank > 1")
            .await
            .expect("UQL filter"),
    );
    uql.sort();

    assert_eq!(
        sql, uql,
        "the SQL and UQL wire surfaces must return the identical set for the same filter"
    );
    assert_eq!(
        sql,
        vec!["n2".to_string()],
        "only n2 is an Agent with rank > 1"
    );
}

/// The UQL wire surface is DETERMINISTIC: the same cross-modal `MATCH → RANK → LIMIT`
/// issued twice over the wire yields the byte-identical ordered id list (the wire leg of
/// the harness's determinism / snapshot-isolation property).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_uql_is_deterministic() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    // Give the two Agent nodes embeddings so the RANK leg has something to order.
    client
        .simple_query("SET EMBEDDING FOR 'n1' = '[0.2, 0.9, 0.0]'")
        .await
        .expect("embed n1");
    client
        .simple_query("SET EMBEDDING FOR 'n2' = '[0.98, 0.2, 0.0]'")
        .await
        .expect("embed n2");

    let uql = "UQL MATCH (:Agent) |> RANK BY ~[1.0,0.0,0.0] |> LIMIT 5";
    let a = simple_ids(client.simple_query(uql).await.expect("UQL run 1"));
    let b = simple_ids(client.simple_query(uql).await.expect("UQL run 2"));
    assert_eq!(a, b, "the same UQL over the wire must be deterministic");
    assert_eq!(
        a,
        vec!["n2".to_string(), "n1".to_string()],
        "ranked n2 > n1"
    );
}

/// Two sessions begin at one snapshot and update the same row: the second
/// commit refuses with SQLSTATE 40001 and the code `TXN_CONFLICT`, and the
/// first writer's value stays. Sessions on disjoint rows both commit.
// spec: MEDULLA-ENGINE-PARITY-R002.2
// spec: MEDULLA-ENGINE-PARITY-R002.3
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn occ_conflict_over_wire() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let a = connect(&addr).await;
    let b = connect(&addr).await;
    let table = unique_table();

    a.simple_query(&format!(
        "CREATE TABLE {table} (id BIGINT PRIMARY KEY, bal BIGINT NOT NULL)"
    ))
    .await
    .expect("CREATE TABLE");
    a.simple_query(&format!(
        "INSERT INTO {table} (id, bal) VALUES (1, 10), (2, 20)"
    ))
    .await
    .expect("seed rows");

    a.simple_query("BEGIN").await.expect("A BEGIN");
    b.simple_query("BEGIN").await.expect("B BEGIN");
    b.simple_query(&format!("UPDATE {table} SET bal = 12 WHERE id = 1"))
        .await
        .expect("B update");
    b.simple_query("COMMIT").await.expect("B COMMIT");

    a.simple_query(&format!("UPDATE {table} SET bal = 11 WHERE id = 1"))
        .await
        .expect("A update is buffered");
    let refused = a
        .simple_query("COMMIT")
        .await
        .expect_err("A's commit conflicts with B's");
    let chain = error_chain(&refused);
    // The SQLSTATE travels in the error's code field, not in its display text.
    assert_eq!(
        refused
            .as_db_error()
            .expect("a database error")
            .code()
            .code(),
        "40001",
        "expected SQLSTATE 40001: {chain}"
    );
    assert!(
        chain.contains("txn_conflict"),
        "expected TXN_CONFLICT: {chain}"
    );
    let _ = a.simple_query("ROLLBACK").await;

    let value = simple_ids(
        a.simple_query(&format!("SELECT bal FROM {table} WHERE id = 1"))
            .await
            .expect("read back"),
    );
    assert_eq!(value, vec!["12".to_string()], "B's value stays");

    // Disjoint rows from one snapshot both commit.
    a.simple_query("BEGIN").await.expect("A BEGIN 2");
    b.simple_query("BEGIN").await.expect("B BEGIN 2");
    b.simple_query(&format!("UPDATE {table} SET bal = 21 WHERE id = 2"))
        .await
        .expect("B update row 2");
    b.simple_query("COMMIT").await.expect("B COMMIT 2");
    a.simple_query(&format!("UPDATE {table} SET bal = 13 WHERE id = 1"))
        .await
        .expect("A update row 1");
    a.simple_query("COMMIT")
        .await
        .expect("disjoint commit succeeds");
}

/// Define one plan view over `__commons__` through the served dispatch path.
#[cfg(feature = "matview")]
async fn define_plan_view(
    state: &test_support::SharedState,
    id: u64,
    name: &str,
    ops: Vec<medulla_query_types::wire::Op>,
) {
    let reply = test_support::dispatch(
        state,
        test_support::commons_request(
            "test",
            id,
            medulla_protocol_types::protocol::Method::PlanMatViewDefine {
                name: name.to_string(),
                graph: "__commons__".to_string(),
                plan: medulla_query_types::wire::Plan::new(ops),
            },
        ),
    )
    .await;
    assert!(
        reply.error.is_none(),
        "define {name} failed: {:?}, detail: {:?}",
        reply.error,
        reply.error_detail
    );
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R005.5
#[cfg(feature = "matview")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matview_status_catalog_reports_mode_and_reason() {
    use medulla_query_types::wire::{Op, Pred};

    // Plan views are unscoped, so only a `System` caller may define them.
    let state = test_support::seeded_system_wire_state(
        "test",
        common::TEST_AGENT,
        Some(sql_test_persist_dir()),
        default_persistence(),
    );
    // The plan filters on `year`, a property only the document nodes carry; a view over a
    // property no node holds has no column to read.
    {
        let guard = state.read().await;
        let core = guard.registry.get("__commons__").unwrap().core.clone();
        let properties = serde_json::json!({
            "type": "Doc",
            "year": 2010,
            "_visibility": "public",
            "_owner": common::TEST_AGENT,
        });
        core.add_node(
            "doc-1".to_string(),
            rmp_serde::to_vec_named(&properties).expect("encode doc node"),
        );
    }
    let scan = || Op::Scan {
        label: "Doc".into(),
    };
    define_plan_view(
        &state,
        2,
        "r005_5_supported",
        vec![
            scan(),
            Op::Filter {
                preds: vec![Pred::GtNum {
                    prop: "year".into(),
                    n: 2000.0,
                }],
            },
        ],
    )
    .await;
    define_plan_view(
        &state,
        3,
        "r005_5_traverse",
        vec![
            scan(),
            Op::Traverse {
                rel: "CITES".into(),
                min: 1,
                max: 2,
            },
        ],
    )
    .await;

    let addr = spawn_listener(state).await;
    let client = test_support::connect_pgwire(&addr, "test", common::TEST_AGENT, "__commons__")
        .await
        .expect("pgwire connect");
    let rows = client
        .query(
            "SELECT name, mode, reason_code FROM medulla_catalog.matviews \
             WHERE name LIKE 'r005_5_%' ORDER BY name",
            &[],
        )
        .await
        .expect("select medulla_catalog.matviews");
    let seen: Vec<(String, String, Option<String>)> = rows
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect();
    assert_eq!(
        seen,
        vec![
            (
                "r005_5_supported".to_string(),
                "incremental".to_string(),
                None
            ),
            (
                "r005_5_traverse".to_string(),
                "recompute".to_string(),
                Some("MATVIEW_NOT_INCREMENTAL".to_string())
            ),
        ]
    );
}

/// 32 sessions each update their own row range of one table: all 32 commits
/// succeed, none returns SQLSTATE 40001, and every row holds its own value.
// spec: MEDULLA-ENGINE-PARITY-R004.3
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn disjoint_sessions_commit_in_parallel() {
    const SESSIONS: i64 = 32;
    const ROWS_PER_SESSION: i64 = 4;
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let setup = connect(&addr).await;
    let table = unique_table();
    setup
        .simple_query(&format!(
            "CREATE TABLE {table} (id BIGINT PRIMARY KEY, owner BIGINT NOT NULL, bal BIGINT NOT NULL)"
        ))
        .await
        .expect("CREATE TABLE");
    let values: Vec<String> = (0..SESSIONS * ROWS_PER_SESSION)
        .map(|id| format!("({id}, {}, 0)", id / ROWS_PER_SESSION))
        .collect();
    setup
        .simple_query(&format!(
            "INSERT INTO {table} (id, owner, bal) VALUES {}",
            values.join(", ")
        ))
        .await
        .expect("seed rows");

    let mut sessions = Vec::new();
    for _ in 0..SESSIONS {
        sessions.push(connect(&addr).await);
    }
    // Every session pins the same snapshot before any of them commits.
    for session in &sessions {
        session.simple_query("BEGIN").await.expect("BEGIN");
    }
    let mut tasks = Vec::new();
    for (owner, session) in sessions.into_iter().enumerate() {
        let table = table.clone();
        tasks.push(tokio::spawn(async move {
            session
                .simple_query(&format!(
                    "UPDATE {table} SET bal = {} WHERE owner = {owner}",
                    owner as i64 + 1
                ))
                .await
                .map_err(|e| error_chain(&e))?;
            session
                .simple_query("COMMIT")
                .await
                .map_err(|e| error_chain(&e))
        }));
    }
    for task in tasks {
        let outcome = task.await.expect("session task");
        assert!(
            outcome.is_ok(),
            "a disjoint commit must succeed, got {outcome:?}"
        );
    }
    let rows = setup
        .simple_query(&format!(
            "SELECT owner, bal FROM {table} WHERE bal <> owner + 1"
        ))
        .await
        .expect("verify");
    assert!(
        simple_ids(rows).is_empty(),
        "every row holds its own session's value"
    );
}

// ── Wire logins (MEDULLA-ENGINE-PARITY-R007.2, R007.3) ─────────────────────────

const LOGIN_SECRET: &str = "wire-login-test-secret"; // sanitizer:ignore
const LOGIN_PASSWORD: &str = "wire-login-pass"; // sanitizer:ignore
const LOGIN_SALT: [u8; 16] = [9u8; 16];

/// A state whose `__commons__` graph holds one private node per principal.
fn login_state(principals: &[&str]) -> Arc<RwLock<ServerState>> {
    let registry = GraphRegistry::new();
    let core = registry.get("__commons__").unwrap().core.clone();
    for principal in principals {
        let properties = serde_json::json!({
            "type": "Note",
            "_visibility": "private",
            "_owner": principal,
        });
        core.add_node(
            format!("note-{principal}"),
            rmp_serde::to_vec_named(&properties).expect("encode node"),
        );
    }
    test_support::state_with_registry(
        LOGIN_SECRET,
        test_support::wire_isolation(principals),
        registry,
        Some(sql_test_persist_dir()),
        None,
    )
}

/// Put `record` into the identity store the listener reads its logins from
/// (MEDULLA-ENGINE-PARITY-R007.4).
async fn register_wire_login(
    state: &test_support::SharedState,
    record: medulla_identity_types::identity::WireLoginRecord,
) {
    state.write().await.isolation.seed_wire_login(record);
}

async fn register_password_login(
    state: &test_support::SharedState,
    login: &str,
    principal: &str,
    tenant: &str,
) {
    let record = medulla_identity_types::identity::WireLoginRecord::new(
        login,
        medulla_identity_types::identity::WireLoginKind::Password,
        &pgwire::password_verifier(LOGIN_PASSWORD, &LOGIN_SALT),
        principal,
        tenant,
        1,
    );
    register_wire_login(state, record).await;
}

async fn wire_note_ids(client: &tokio_postgres::Client) -> Vec<String> {
    let mut ids: Vec<String> = client
        .query("SELECT id FROM nodes WHERE type = 'Note'", &[])
        .await
        .expect("wire read")
        .iter()
        .map(|row| row.get::<_, String>(0))
        .collect();
    ids.sort();
    ids
}

/// The same read through `Method::Uql`, signed as `principal`.
async fn native_note_ids(
    state: &test_support::SharedState,
    id: u64,
    principal: &str,
) -> Vec<String> {
    let request = common::signed_request_as(
        LOGIN_SECRET,
        id,
        "__commons__",
        principal,
        medulla_protocol_types::protocol::Method::Uql {
            text: "MATCH (:Note)".to_string(),
            params: Default::default(),
            prepare: false,
            foreign_executions: Default::default(),
        },
    );
    let response = test_support::dispatch(state, request).await;
    assert!(
        response.error.is_none(),
        "native read: {:?}",
        response.error
    );
    let mut ids = test_support::uql_ids(&response);
    ids.sort();
    ids
}

// spec: MEDULLA-ENGINE-PARITY-R007.2
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_rls_equals_native_rls() {
    let principals = ["rls-a", "rls-b", "rls-c"];
    let state = login_state(&principals);
    let addr = spawn_listener_mode(state.clone(), pgwire::PgWireAuthMode::Scram).await;
    for (n, principal) in principals.iter().enumerate() {
        let login = format!("bi-{principal}");
        register_password_login(&state, &login, principal, common::TEST_TENANT).await;
        let client = connect_scram(&addr, &login, LOGIN_PASSWORD, "__commons__")
            .await
            .expect("wire login");
        let over_wire = wire_note_ids(&client).await;
        let native = native_note_ids(&state, 100 + n as u64, principal).await;
        assert_eq!(
            over_wire, native,
            "{principal}: the wire read and Method::Uql return the same rows"
        );
        assert!(
            over_wire.contains(&format!("note-{principal}")),
            "{principal} sees its own row"
        );
        assert!(
            over_wire.len() < principals.len(),
            "{principal} does not see every principal's private row"
        );
    }
}

// spec: MEDULLA-ENGINE-PARITY-R007.2
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_login_of_another_tenant_refuses_plan_scope_unverified() {
    let state = login_state(&["tenant-x"]);
    let addr = spawn_listener_mode(state.clone(), pgwire::PgWireAuthMode::Scram).await;
    register_password_login(&state, "bi-foreign", "tenant-x", "some-other-tenant").await;
    let client = connect_scram(&addr, "bi-foreign", LOGIN_PASSWORD, "__commons__")
        .await
        .expect("SCRAM succeeds; the scope check runs on the first query");
    let err = client
        .query("SELECT id FROM nodes", &[])
        .await
        .expect_err("a login whose tenant is unverified reads nothing");
    assert!(
        error_chain(&err).contains("plan_scope_unverified"),
        "got {}",
        error_chain(&err)
    );
    // An unknown login refuses at startup with SQLSTATE 28000.
    let unknown = connect_scram(&addr, "no-such-login", LOGIN_PASSWORD, "__commons__")
        .await
        .expect_err("unknown login refuses");
    let chain = error_chain(&unknown);
    assert!(
        chain.contains("28000") || chain.contains("28p01"),
        "got {chain}"
    );
}

// spec: MEDULLA-ENGINE-PARITY-R007.3
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_maps_to_principal() {
    const TOKEN: &str = "wire-login-token-1"; // sanitizer:ignore
    let state = login_state(&["login-a"]);
    let addr = spawn_listener_mode(state.clone(), pgwire::PgWireAuthMode::Scram).await;
    register_password_login(&state, "bi-login-a", "login-a", common::TEST_TENANT).await;
    register_wire_login(
        &state,
        medulla_identity_types::identity::WireLoginRecord::new(
            "bi-token-a",
            medulla_identity_types::identity::WireLoginKind::Token,
            &medulla_identity_types::identity::sha256_verifier(TOKEN.as_bytes()),
            "login-a",
            common::TEST_TENANT,
            1,
        ),
    )
    .await;
    for round in 0..1_000 {
        let by_password = connect_scram(&addr, "bi-login-a", LOGIN_PASSWORD, "__commons__")
            .await
            .unwrap_or_else(|e| panic!("password login {round}: {e}"));
        let by_token = connect_scram(&addr, "bi-token-a", TOKEN, "__commons__")
            .await
            .unwrap_or_else(|e| panic!("token login {round}: {e}"));
        for client in [&by_password, &by_token] {
            assert_eq!(
                wire_note_ids(client).await,
                vec!["note-login-a".to_string()],
                "login {round}"
            );
        }
    }
}

/// Generated mTLS material: a CA, a server leaf for 127.0.0.1 and a client leaf, all
/// created at test time. Only the PEM paths of the server side are handed to the
/// listener; the client side stays in memory.
struct MtlsFixture {
    _directory: tempfile::TempDir,
    server_cert: std::path::PathBuf,
    server_key: std::path::PathBuf,
    client_ca: std::path::PathBuf,
    client_leaf_der: Vec<u8>,
    connector: tokio_postgres_rustls::MakeRustlsConnect,
}

fn mtls_fixture() -> MtlsFixture {
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
        SanType,
    };
    use std::os::unix::fs::PermissionsExt;

    let ca_key = KeyPair::generate().expect("CA key");
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "wire-login-test-ca");
    let ca = ca_params.self_signed(&ca_key).expect("CA certificate");

    let server_key = KeyPair::generate().expect("server key");
    let mut server_params = CertificateParams::new(Vec::<String>::new()).expect("server params");
    server_params.subject_alt_names = vec![SanType::IpAddress("127.0.0.1".parse().unwrap())];
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server = server_params
        .signed_by(&server_key, &ca, &ca_key)
        .expect("server certificate");

    let client_key = KeyPair::generate().expect("client key");
    let mut client_params = CertificateParams::new(Vec::<String>::new()).expect("client params");
    client_params
        .distinguished_name
        .push(DnType::CommonName, "bi-cert-a");
    client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let client = client_params
        .signed_by(&client_key, &ca, &ca_key)
        .expect("client certificate");

    let directory = tempfile::tempdir().expect("TLS directory");
    let write = |name: &str, pem: String, mode: u32| {
        let path = directory.path().join(name);
        std::fs::write(&path, pem).expect("write PEM");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("restrict PEM");
        path
    };
    let server_cert = write("server.pem", server.pem(), 0o600);
    let server_key_path = write("server.key", server_key.serialize_pem(), 0o600);
    let client_ca = write("client-ca.pem", ca.pem(), 0o600);

    let mut roots = rustls::RootCertStore::empty();
    roots.add(ca.der().clone()).expect("trust the test CA");
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_root_certificates(roots)
        .with_client_auth_cert(
            vec![client.der().clone()],
            rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
                client_key.serialize_der(),
            )),
        )
        .expect("client identity");
    MtlsFixture {
        server_cert,
        server_key: server_key_path,
        client_ca,
        client_leaf_der: client.der().as_ref().to_vec(),
        connector: tokio_postgres_rustls::MakeRustlsConnect::new(config),
        _directory: directory,
    }
}

/// Bind an mTLS pgwire listener with explicit TLS material (no environment).
async fn spawn_mtls_listener(state: Arc<RwLock<ServerState>>, fixture: &MtlsFixture) -> String {
    provision_text_embedder_once();
    let addr = test_support::ephemeral_listener_addr().await;
    let secret = state.read().await.auth_secret.clone();
    let prepared = pgwire::prepare_startup_policy_with_tls(
        &addr,
        secret,
        pgwire::PgWireAuthMode::Scram,
        Some(pgwire::PgWireTlsConfig {
            cert_path: fixture.server_cert.to_string_lossy().into_owned(),
            key_path: fixture.server_key.to_string_lossy().into_owned(),
            client_ca_path: Some(fixture.client_ca.to_string_lossy().into_owned()),
        }),
    )
    .await
    .expect("prepare the mTLS listener");
    tokio::spawn(async move {
        let _ = pgwire::serve_prepared(state, prepared).await;
    });
    test_support::wait_for_listener_ready(&addr).await;
    addr
}

async fn connect_mtls(
    addr: &str,
    connector: &tokio_postgres_rustls::MakeRustlsConnect,
    user: &str,
) -> Result<tokio_postgres::Client, tokio_postgres::Error> {
    let port = addr.rsplit(':').next().unwrap();
    let conn_str =
        format!("host=127.0.0.1 port={port} user={user} dbname=__commons__ sslmode=require");
    let (client, connection) = tokio_postgres::connect(&conn_str, connector.clone()).await?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(client)
}

// spec: MEDULLA-ENGINE-PARITY-R007.3
/// The Certificate leg of the login proof: 1,000 certificate logins over an mTLS
/// listener each map to the record's principal, and a certificate whose fingerprint
/// is not the record's refuses with SQLSTATE 28000.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn certificate_login_maps_to_principal() {
    let state = login_state(&["login-a"]);
    let fixture = mtls_fixture();
    let addr = spawn_mtls_listener(state.clone(), &fixture).await;
    let register = |login: &str, der: &[u8]| {
        medulla_identity_types::identity::WireLoginRecord::new(
            login,
            medulla_identity_types::identity::WireLoginKind::Certificate,
            &medulla_identity_types::identity::sha256_verifier(der),
            "login-a",
            common::TEST_TENANT,
            1,
        )
    };
    register_wire_login(&state, register("bi-cert-a", &fixture.client_leaf_der)).await;
    register_wire_login(
        &state,
        register("bi-cert-other", b"not the presented certificate"),
    )
    .await;
    for round in 0..1_000 {
        let client = connect_mtls(&addr, &fixture.connector, "bi-cert-a")
            .await
            .unwrap_or_else(|e| panic!("certificate login {round}: {}", error_chain(&e)));
        assert_eq!(
            wire_note_ids(&client).await,
            vec!["note-login-a".to_string()],
            "certificate login {round}"
        );
    }
    let mismatch = connect_mtls(&addr, &fixture.connector, "bi-cert-other")
        .await
        .expect_err("a fingerprint that is not the record's refuses");
    let chain = error_chain(&mismatch);
    assert!(
        chain.contains("28000") || chain.contains("invalid_login"),
        "got {chain}"
    );
}

/// A statement form outside the compatibility matrix is refused with SQLSTATE `0A000`
/// and the form's name, on both the simple and the extended protocol, and the
/// connection stays usable afterwards.
// spec: MEDULLA-ENGINE-PARITY-R009.2
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refused_form_returns_0a000() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    for (sql, form) in [
        (
            "CREATE TRIGGER t BEFORE INSERT ON nodes FOR EACH ROW EXECUTE FUNCTION f()",
            "create_trigger",
        ),
        ("CREATE POLICY p ON nodes USING (true)", "create_policy"),
    ] {
        let simple = client
            .simple_query(sql)
            .await
            .expect_err("a refused form must not run");
        let db = simple.as_db_error().expect("a database error");
        assert_eq!(
            db.code(),
            &tokio_postgres::error::SqlState::FEATURE_NOT_SUPPORTED
        );
        assert!(db.message().contains(form), "{}", db.message());

        let extended = match client.prepare(sql).await {
            Ok(statement) => client
                .execute(&statement, &[])
                .await
                .expect_err("a refused form must not run"),
            Err(error) => error,
        };
        assert_eq!(
            extended.code(),
            Some(&tokio_postgres::error::SqlState::FEATURE_NOT_SUPPORTED),
            "{extended:?}"
        );
    }

    let rows = client
        .simple_query("SELECT id FROM nodes WHERE rank >= 2 ORDER BY id")
        .await
        .expect("the connection still serves supported forms");
    assert_eq!(simple_ids(rows), vec!["n2".to_string(), "n3".to_string()]);
}

/// One frontend frame: the tag byte, an `int32` length that counts itself, the body.
fn raw_frame(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut frame = vec![tag];
    frame.extend_from_slice(&(body.len() as i32 + 4).to_be_bytes());
    frame.extend_from_slice(body);
    frame
}

/// The `(field code, text)` pairs of an `ErrorResponse` body.
fn error_fields(body: &[u8]) -> std::collections::HashMap<u8, String> {
    let mut fields = std::collections::HashMap::new();
    let mut rest = body;
    while let Some((&code, tail)) = rest.split_first() {
        if code == 0 {
            break;
        }
        let end = tail.iter().position(|b| *b == 0).expect("field terminator");
        fields.insert(code, String::from_utf8_lossy(&tail[..end]).into_owned());
        rest = &tail[end + 1..];
    }
    fields
}

/// The raw frames the proxy captured, shared with its relay task.
type CapturedFrames = Arc<std::sync::Mutex<Option<Vec<(u8, Vec<u8>)>>>>;

/// A TCP relay between a client library and the listener that can put a raw frontend
/// frame on the server side at a frame boundary and capture the server's answer up to
/// its `ReadyForQuery`, so a test can send a message no client library emits. The
/// client never sees that exchange, so its own session stays in step.
struct RawFrameProxy {
    addr: String,
    to_server: Arc<tokio::sync::Mutex<tokio::net::tcp::OwnedWriteHalf>>,
    capture: CapturedFrames,
    answered: Arc<tokio::sync::Notify>,
}

impl RawFrameProxy {
    async fn start(server_addr: &str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the relay");
        let addr = listener.local_addr().expect("relay address").to_string();
        let server = tokio::net::TcpStream::connect(server_addr)
            .await
            .expect("connect to the listener");
        let (from_server, to_server) = server.into_split();
        let to_server = Arc::new(tokio::sync::Mutex::new(to_server));
        let capture: CapturedFrames = Arc::default();
        let answered = Arc::new(tokio::sync::Notify::new());
        tokio::spawn(Self::relay(
            listener,
            from_server,
            to_server.clone(),
            capture.clone(),
            answered.clone(),
        ));
        Self {
            addr,
            to_server,
            capture,
            answered,
        }
    }

    /// Accept the one client, copy its bytes to the listener, and relay the listener's
    /// frames back, holding back the frames of a captured exchange.
    async fn relay(
        listener: tokio::net::TcpListener,
        mut from_server: tokio::net::tcp::OwnedReadHalf,
        to_server: Arc<tokio::sync::Mutex<tokio::net::tcp::OwnedWriteHalf>>,
        capture: CapturedFrames,
        answered: Arc<tokio::sync::Notify>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (client, _) = listener.accept().await.expect("accept the client");
        let (mut from_client, mut to_client) = client.into_split();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            loop {
                let read = from_client.read(&mut buf).await.unwrap_or(0);
                if read == 0 {
                    break;
                }
                if to_server
                    .lock()
                    .await
                    .write_all(&buf[..read])
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        loop {
            let mut tag = [0u8; 1];
            let mut length = [0u8; 4];
            if from_server.read_exact(&mut tag).await.is_err()
                || from_server.read_exact(&mut length).await.is_err()
            {
                break;
            }
            let mut body = vec![0u8; (i32::from_be_bytes(length) - 4) as usize];
            if from_server.read_exact(&mut body).await.is_err() {
                break;
            }
            let held_back = {
                let mut guard = capture.lock().unwrap();
                match guard.as_mut() {
                    Some(frames) => {
                        frames.push((tag[0], body.clone()));
                        true
                    }
                    None => false,
                }
            };
            if held_back {
                if tag[0] == b'Z' {
                    answered.notify_one();
                }
                continue;
            }
            let mut frame = tag.to_vec();
            frame.extend_from_slice(&length);
            frame.extend_from_slice(&body);
            if to_client.write_all(&frame).await.is_err() {
                break;
            }
        }
    }

    /// Send `frame` to the listener and return every frame it answers with, through
    /// the first `ReadyForQuery`. The client must be idle.
    async fn exchange(&self, frame: Vec<u8>) -> Vec<(u8, Vec<u8>)> {
        use tokio::io::AsyncWriteExt;
        *self.capture.lock().unwrap() = Some(Vec::new());
        self.to_server
            .lock()
            .await
            .write_all(&frame)
            .await
            .expect("write the frame");
        tokio::time::timeout(std::time::Duration::from_secs(30), self.answered.notified())
            .await
            .expect("the listener answers the frame");
        self.capture
            .lock()
            .unwrap()
            .take()
            .expect("captured answer")
    }
}

/// A `FunctionCall` and a `CopyBoth` frame are refused with SQLSTATE `0A000` naming the
/// message, and a refused statement form is refused the same way; the connection keeps
/// serving supported statements after each.
// spec: MEDULLA-ENGINE-PARITY-R006.2
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_message_refuses_0a000() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let proxy = RawFrameProxy::start(&addr).await;
    let client = test_support::connect_pgwire(&proxy.addr, "test", "tester", "__commons__")
        .await
        .expect("connect through the relay");

    // FunctionCall: object id, no format codes, no arguments, text result.
    let function_call = raw_frame(b'F', &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let copy_both = raw_frame(b'W', &[]);
    for (frame, name) in [(function_call, "FunctionCall"), (copy_both, "CopyBoth")] {
        let answer = proxy.exchange(frame).await;
        let kinds: Vec<u8> = answer.iter().map(|(tag, _)| *tag).collect();
        assert_eq!(kinds, vec![b'E', b'Z'], "{name}: an error, then ready");
        let fields = error_fields(&answer[0].1);
        assert_eq!(
            fields.get(&b'C').map(String::as_str),
            Some("0A000"),
            "{name}"
        );
        let message = fields.get(&b'M').expect("a message");
        assert!(message.contains("FEATURE_NOT_SUPPORTED"), "{message}");
        assert!(message.contains(name), "{message}");

        let rows = client
            .simple_query("SELECT id FROM nodes WHERE rank >= 2 ORDER BY id")
            .await
            .expect("the connection still serves supported forms");
        assert_eq!(simple_ids(rows), vec!["n2".to_string(), "n3".to_string()]);
    }

    let refused = client
        .simple_query("CREATE POLICY p ON nodes USING (true)")
        .await
        .expect_err("a refused form must not run");
    assert_eq!(
        refused.as_db_error().expect("a database error").code(),
        &tokio_postgres::error::SqlState::FEATURE_NOT_SUPPORTED
    );
}

/// Every text cell of a simple-query answer joined with newlines (an `EXPLAIN` plan is
/// one row per plan line).
fn simple_text(msgs: &[tokio_postgres::SimpleQueryMessage]) -> String {
    msgs.iter()
        .filter_map(|message| match message {
            tokio_postgres::SimpleQueryMessage::Row(row) => Some(
                (0..row.len())
                    .filter_map(|index| row.get(index))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A partial index created over the wire serves the point query whose `WHERE` carries
/// the index predicate: the rows inside and outside the predicate answer correctly,
/// `EXPLAIN` names the index, and a query that does not carry the predicate scans.
// spec: MEDULLA-ENGINE-PARITY-R010.3
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_partial_index_over_wire_is_used() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;
    let table = unique_table();
    let index = format!("{table}_active_email");

    client
        .simple_query(&format!(
            "CREATE TABLE {table} (id BIGINT PRIMARY KEY, email TEXT NOT NULL, status TEXT NOT NULL)"
        ))
        .await
        .expect("CREATE TABLE");
    client
        .simple_query(&format!(
            "INSERT INTO {table} (id, email, status) VALUES (1, 'a@x', 'active'), (2, 'b@x', 'closed'), (3, 'c@x', 'active')"
        ))
        .await
        .expect("seed rows before the index");
    client
        .simple_query(&format!(
            "CREATE INDEX {index} ON {table} (email) WHERE status = 'active'"
        ))
        .await
        .expect("CREATE INDEX over the wire");
    // Rows inserted after the index exists, inside and outside its predicate.
    client
        .simple_query(&format!(
            "INSERT INTO {table} (id, email, status) VALUES (4, 'd@x', 'active'), (5, 'e@x', 'closed')"
        ))
        .await
        .expect("insert after the index");

    let point = format!("SELECT id FROM {table} WHERE status = 'active' AND email = 'd@x'");
    let plan = client
        .simple_query(&format!("EXPLAIN {point}"))
        .await
        .expect("EXPLAIN");
    let text = simple_text(&plan);
    assert!(
        text.contains(&format!("IndexScanExec: index={index}")),
        "{text}"
    );
    let rows = client.simple_query(&point).await.expect("point query");
    assert_eq!(simple_ids(rows), vec!["4".to_string()]);

    let inside = format!("SELECT id FROM {table} WHERE status = 'active' ORDER BY id");
    let rows = client.simple_query(&inside).await.expect("inside rows");
    assert_eq!(
        simple_ids(rows),
        vec!["1".to_string(), "3".to_string(), "4".to_string()]
    );

    // A query that does not carry the predicate scans, and still finds closed rows.
    let outside = format!("SELECT id FROM {table} WHERE status = 'closed' ORDER BY id");
    let plan = client
        .simple_query(&format!("EXPLAIN {outside}"))
        .await
        .expect("EXPLAIN outside");
    assert!(!simple_text(&plan).contains("IndexScanExec"));
    let rows = client.simple_query(&outside).await.expect("outside rows");
    assert_eq!(simple_ids(rows), vec!["2".to_string(), "5".to_string()]);
}

/// 10,000 rows from 8 concurrent sessions into a serial table: every id is distinct,
/// ids increase within each session, and the sequence reserved its blocks with at
/// most `ceil(10000 / cache)` plus one write per session.
// spec: MEDULLA-ENGINE-PARITY-R013.3
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn serial_column_uses_sequence() {
    const SESSIONS: usize = 8;
    const ROWS: usize = 10_000;
    const STATEMENT_ROWS: usize = 125;
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let admin = connect(&addr).await;
    let table = unique_table();
    admin
        .simple_query(&format!(
            "CREATE TABLE {table} (id SERIAL PRIMARY KEY, session BIGINT NOT NULL, ord BIGINT NOT NULL)"
        ))
        .await
        .expect("CREATE TABLE with a serial column");

    // Each row carries its session and its position in that session's insert
    // order, so the assigned ids can be read back and compared in that order.
    let per_session = ROWS / SESSIONS;
    let mut sessions = Vec::new();
    for session in 0..SESSIONS {
        let addr = addr.clone();
        let table = table.clone();
        sessions.push(tokio::spawn(async move {
            let client = connect(&addr).await;
            for statement in 0..per_session / STATEMENT_ROWS {
                let values = (0..STATEMENT_ROWS)
                    .map(|row| format!("({session}, {})", statement * STATEMENT_ROWS + row))
                    .collect::<Vec<_>>()
                    .join(", ");
                client
                    .simple_query(&format!(
                        "INSERT INTO {table} (session, ord) VALUES {values}"
                    ))
                    .await
                    .expect("serial INSERT");
            }
        }));
    }
    for session in sessions {
        session.await.expect("session task");
    }
    let stored = admin
        .simple_query(&format!("SELECT session, ord, id FROM {table}"))
        .await
        .expect("read back the serial ids");
    let mut by_session: std::collections::BTreeMap<i64, Vec<(i64, i64)>> = Default::default();
    for message in stored {
        if let tokio_postgres::SimpleQueryMessage::Row(row) = message {
            let field = |index: usize| -> i64 {
                row.get(index)
                    .expect("column")
                    .parse()
                    .expect("integer column")
            };
            by_session
                .entry(field(0))
                .or_default()
                .push((field(1), field(2)));
        }
    }
    assert_eq!(by_session.len(), SESSIONS);
    let mut every_id = Vec::with_capacity(ROWS);
    for (_, mut rows) in by_session {
        rows.sort_unstable();
        assert_eq!(rows.len(), per_session);
        assert!(
            rows.windows(2).all(|pair| pair[0].1 < pair[1].1),
            "ids must increase within a session"
        );
        every_id.extend(rows.into_iter().map(|(_, id)| id));
    }
    every_id.sort_unstable();
    every_id.dedup();
    assert_eq!(every_id.len(), ROWS, "every id is distinct");

    // `setval` below the reserved high mark refuses and names the mark, which
    // counts the reservation writes: one per block of `cache` values.
    let rewind = admin
        .simple_query(&format!("SELECT setval('{table}_id_seq', 1)"))
        .await
        .expect_err("setval below the high mark refuses");
    let message = rewind.as_db_error().map(|db| db.message()).unwrap_or("");
    assert!(message.contains("SEQUENCE_REWIND"), "{message}");
    let high_mark: u64 = message
        .split("high mark ")
        .nth(1)
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|digits| digits.parse().ok())
        .unwrap_or_else(|| panic!("the refusal names the high mark: {message}"));
    let cache = medulla_storage::SERIAL_SEQUENCE_CACHE;
    let reservations = (high_mark - medulla_storage::SERIAL_SEQUENCE_START) / cache;
    let bound = (ROWS as u64).div_ceil(cache) + SESSIONS as u64;
    assert!(
        reservations <= bound,
        "{reservations} reservation writes over the bound {bound}"
    );
}

/// The SQLSTATE and message of a refused simple statement.
async fn refusal_of(client: &tokio_postgres::Client, sql: &str) -> (String, String) {
    let error = client
        .simple_query(sql)
        .await
        .expect_err("the statement must be refused");
    let db = error.as_db_error().expect("a database error");
    (db.code().code().to_string(), db.message().to_string())
}

async fn count_rows(client: &tokio_postgres::Client, table: &str) -> usize {
    let rows = client
        .simple_query(&format!("SELECT id FROM {table}"))
        .await
        .expect("count rows");
    simple_ids(rows).len()
}

/// Each constraint kind refuses with its SQLSTATE and names the constraint, a
/// deferred foreign key refuses at `COMMIT`, and a refused statement leaves the
/// table unchanged.
// spec: MEDULLA-ENGINE-PARITY-R011.2
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn constraint_violations_return_sqlstate() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;
    let parents = unique_table();
    let children = unique_table();

    client
        .simple_query(&format!("CREATE TABLE {parents} (id BIGINT PRIMARY KEY)"))
        .await
        .expect("CREATE parents");
    client
        .simple_query(&format!(
            "CREATE TABLE {children} (id BIGINT PRIMARY KEY, parent_id BIGINT, bal BIGINT NOT NULL, \
             CONSTRAINT {children}_bal_nonneg CHECK (bal >= 0), \
             CONSTRAINT {children}_parent_fk FOREIGN KEY (parent_id) REFERENCES {parents} (id))"
        ))
        .await
        .expect("CREATE children");
    client
        .simple_query(&format!("INSERT INTO {parents} (id) VALUES (1)"))
        .await
        .expect("seed parent");
    client
        .simple_query(&format!(
            "INSERT INTO {children} (id, parent_id, bal) VALUES (1, 1, 10)"
        ))
        .await
        .expect("seed child");

    // 23505: a duplicate primary key.
    let (code, _) = refusal_of(
        &client,
        &format!("INSERT INTO {children} (id, parent_id, bal) VALUES (1, 1, 5)"),
    )
    .await;
    assert_eq!(code, "23505");
    // 23503: a parent that does not exist, naming the constraint.
    let (code, message) = refusal_of(
        &client,
        &format!("INSERT INTO {children} (id, parent_id, bal) VALUES (2, 99, 5)"),
    )
    .await;
    assert_eq!(code, "23503");
    assert!(
        message.contains(&format!("{children}_parent_fk")),
        "{message}"
    );
    // 23514: a failing CHECK, naming the constraint.
    let (code, message) = refusal_of(
        &client,
        &format!("INSERT INTO {children} (id, parent_id, bal) VALUES (3, 1, -1)"),
    )
    .await;
    assert_eq!(code, "23514");
    assert!(
        message.contains(&format!("{children}_bal_nonneg")),
        "{message}"
    );
    // 23502: a NULL in a NOT NULL column.
    let (code, _) = refusal_of(
        &client,
        &format!("INSERT INTO {children} (id, parent_id) VALUES (4, 1)"),
    )
    .await;
    assert_eq!(code, "23502");
    // UPDATE and DELETE are checked too.
    let (code, _) = refusal_of(
        &client,
        &format!("UPDATE {children} SET bal = -5 WHERE id = 1"),
    )
    .await;
    assert_eq!(code, "23514");
    let (code, _) = refusal_of(&client, &format!("DELETE FROM {parents} WHERE id = 1")).await;
    assert_eq!(code, "23503");
    // Every refused statement left the tables unchanged.
    assert_eq!(count_rows(&client, &children).await, 1);
    assert_eq!(count_rows(&client, &parents).await, 1);

    // A deferred foreign key refuses at COMMIT and writes nothing.
    let deferred = unique_table();
    client
        .simple_query(&format!(
            "CREATE TABLE {deferred} (id BIGINT PRIMARY KEY, parent_id BIGINT, \
             CONSTRAINT {deferred}_parent_fk FOREIGN KEY (parent_id) REFERENCES {parents} (id) \
             DEFERRABLE INITIALLY DEFERRED)"
        ))
        .await
        .expect("CREATE deferred");
    client.simple_query("BEGIN").await.expect("BEGIN");
    client
        .simple_query(&format!(
            "INSERT INTO {deferred} (id, parent_id) VALUES (1, 77)"
        ))
        .await
        .expect("a deferred key is not checked per statement");
    let (code, message) = refusal_of(&client, "COMMIT").await;
    assert_eq!(code, "23503");
    assert!(
        message.contains(&format!("{deferred}_parent_fk")),
        "{message}"
    );
    let _ = client.simple_query("ROLLBACK").await;
    assert_eq!(count_rows(&client, &deferred).await, 0);
}

// spec: MEDULLA-ENGINE-PARITY-R012.2
/// `CREATE TRIGGER` and `DROP TRIGGER` buffer like the other catalog DDL
/// inside an explicit pgwire transaction: `BEGIN; CREATE TRIGGER; ROLLBACK`
/// leaves no trigger, `BEGIN; CREATE TRIGGER; COMMIT` installs it, and
/// `BEGIN; DROP TRIGGER; ROLLBACK` keeps it.
#[cfg(feature = "statechart")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_txn_create_and_drop_trigger_follow_the_transaction() {
    let machine = stamp_before_insert_machine();
    // The machine lives in the engine's own statechart store, which is where
    // the served `CREATE TRIGGER` reads it.
    let persist_dir = sql_test_persist_dir();
    let machine_id = medulla_h_jobs::statechart::statechart_store(Some(&persist_dir))
        .expect("statechart store")
        .define(&machine)
        .expect("define the trigger machine");
    let state = state_with_dir(default_persistence(), persist_dir);
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;
    let table = unique_table();
    client
        .simple_query(&format!("CREATE TABLE {table} (id BIGINT, status TEXT)"))
        .await
        .expect("CREATE TABLE");

    let status_after_insert = |id: i64| {
        let client = &client;
        let table = table.clone();
        async move {
            client
                .simple_query(&format!(
                    "INSERT INTO {table} (id, status) VALUES ({id}, 'closed')"
                ))
                .await
                .expect("INSERT");
            let rows = client
                .simple_query(&format!("SELECT status FROM {table} WHERE id = {id}"))
                .await
                .expect("SELECT");
            rows.into_iter()
                .find_map(|message| match message {
                    tokio_postgres::SimpleQueryMessage::Row(row) => row.get(0).map(str::to_string),
                    _ => None,
                })
                .expect("one row")
        }
    };
    let create =
        format!("CREATE TRIGGER stamp BEFORE INSERT ON {table} EXECUTE MACHINE {machine_id}");

    // Rolled back: the trigger never installs.
    client.simple_query("BEGIN").await.expect("BEGIN");
    client.simple_query(&create).await.expect("CREATE TRIGGER");
    client.simple_query("ROLLBACK").await.expect("ROLLBACK");
    assert_eq!(status_after_insert(1).await, "closed");

    // Committed: the trigger installs and stamps the next insert.
    client.simple_query("BEGIN").await.expect("BEGIN");
    client.simple_query(&create).await.expect("CREATE TRIGGER");
    client.simple_query("COMMIT").await.expect("COMMIT");
    assert_eq!(status_after_insert(2).await, "open");

    // A rolled back DROP keeps the trigger; a committed one removes it.
    let drop = format!("DROP TRIGGER stamp ON {table}");
    client.simple_query("BEGIN").await.expect("BEGIN");
    client.simple_query(&drop).await.expect("DROP TRIGGER");
    client.simple_query("ROLLBACK").await.expect("ROLLBACK");
    assert_eq!(status_after_insert(3).await, "open");
    client.simple_query("BEGIN").await.expect("BEGIN");
    client.simple_query(&drop).await.expect("DROP TRIGGER");
    client.simple_query("COMMIT").await.expect("COMMIT");
    assert_eq!(status_after_insert(4).await, "closed");

    let _ = client.simple_query(&format!("DROP TABLE {table}")).await;
}

// spec: MEDULLA-DURABLE-KERNEL-R035.4
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explain_and_explain_analyze_return_plans_and_refuse_writes() {
    use tokio_postgres::SimpleQueryMessage;

    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    // Plain EXPLAIN and EXPLAIN ANALYZE of a read answer plan rows.
    for statement in [
        "EXPLAIN SELECT id FROM nodes WHERE rank >= 2",
        "EXPLAIN ANALYZE SELECT id FROM nodes WHERE rank >= 2",
    ] {
        let messages = client
            .simple_query(statement)
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
        let plan_rows = messages
            .iter()
            .filter(|message| matches!(message, SimpleQueryMessage::Row(_)))
            .count();
        assert!(plan_rows > 0, "{statement} returned no plan rows");
    }

    // EXPLAIN ANALYZE of a write is refused as not supported, and writes nothing.
    let refusal = client
        .simple_query("EXPLAIN ANALYZE INSERT INTO nodes (id) VALUES ('n-explain')")
        .await
        .expect_err("EXPLAIN ANALYZE of a write must be refused");
    assert_eq!(
        refusal.code(),
        Some(&tokio_postgres::error::SqlState::FEATURE_NOT_SUPPORTED),
        "{refusal}"
    );
    let rows = client
        .simple_query("SELECT id FROM nodes WHERE id = 'n-explain'")
        .await
        .expect("read after the refused write");
    assert!(simple_ids(rows).is_empty(), "the refused write took effect");
}

/// Send a raw `CancelRequest` (length 16, code 80877102, pid, secret) on a fresh
/// connection and wait for the server to close it.
async fn send_raw_cancel_request(addr: &str, pid: i32, secret: i32) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect for the cancel request");
    let mut frame = Vec::with_capacity(16);
    frame.extend_from_slice(&16i32.to_be_bytes());
    frame.extend_from_slice(&80_877_102i32.to_be_bytes());
    frame.extend_from_slice(&pid.to_be_bytes());
    frame.extend_from_slice(&secret.to_be_bytes());
    stream.write_all(&frame).await.expect("send the cancel request");
    let mut rest = Vec::new();
    let _ = stream.read_to_end(&mut rest).await;
}

// spec: MEDULLA-DURABLE-KERNEL-R035.5
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_request_stops_a_long_query_and_wrong_secret_is_ignored() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = Arc::new(connect(&addr).await);

    // A cancel with the connection's own key stops a long statement with 57014.
    let long = "SELECT count(*) FROM generate_series(1, 5000000000)";
    let running = {
        let client = Arc::clone(&client);
        tokio::spawn(async move { client.simple_query(long).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let cancelled_at = std::time::Instant::now();
    client
        .cancel_token()
        .cancel_query(tokio_postgres::NoTls)
        .await
        .expect("send the cancel request");
    let error = running
        .await
        .expect("the query task joins")
        .expect_err("a cancelled statement fails");
    let elapsed = cancelled_at.elapsed();
    assert_eq!(
        error.code(),
        Some(&tokio_postgres::error::SqlState::QUERY_CANCELED),
        "{error:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "the cancel took {elapsed:?}"
    );

    // The connection keeps serving after a cancel.
    let rows = client
        .simple_query("SELECT id FROM nodes WHERE rank >= 2 ORDER BY id")
        .await
        .expect("the connection serves after a cancel");
    assert_eq!(simple_ids(rows), vec!["n2".to_string(), "n3".to_string()]);

    // A wrong key matches no registered connection: it is ignored and the
    // statement completes.
    let bounded = "SELECT count(*) AS n FROM generate_series(1, 20000000)";
    let running = {
        let client = Arc::clone(&client);
        tokio::spawn(async move { client.simple_query(bounded).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    send_raw_cancel_request(&addr, 100_000, 0x5eed_0bad).await;
    let messages = running
        .await
        .expect("the query task joins")
        .expect("a wrong-key cancel leaves the statement running");
    let count = messages
        .into_iter()
        .find_map(|message| match message {
            tokio_postgres::SimpleQueryMessage::Row(row) => row.get(0).map(str::to_string),
            _ => None,
        })
        .expect("one count row");
    assert_eq!(count, "20000000");
}

// spec: MEDULLA-DURABLE-KERNEL-R035.6
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pooler_transaction_mode_isolates_clients() {
    let secret = "dummy-pg-test-secret"; // sanitizer:ignore
    let state = scram_state(secret);
    let addr = spawn_listener_mode(state, pgwire::PgWireAuthMode::Scram).await;
    // One backend connection, as a transaction-mode pooler holds it.
    let backend = scram_client(secret, &addr, "worker").await;
    let commons_ids = vec!["n1".to_string(), "n2".to_string(), "n3".to_string()];

    // Client A: a prepared statement and a graph binding of its own.
    let prepared = backend
        .prepare("SELECT id FROM nodes WHERE rank > $1 ORDER BY id")
        .await
        .expect("client A prepares");
    let rows = backend
        .query(&prepared, &[&1i64])
        .await
        .expect("client A runs its statement");
    assert_eq!(rows.len(), 2);
    backend
        .simple_query("SET graph = 'agent:worker'")
        .await
        .expect("client A binds its own graph");
    let rows = backend
        .simple_query("SELECT id FROM nodes ORDER BY id")
        .await
        .expect("client A reads its graph");
    assert!(simple_ids(rows).is_empty(), "agent:worker holds no nodes");

    // DISCARD ALL inside an open transaction refuses with 25001.
    backend.simple_query("BEGIN").await.expect("BEGIN");
    let refused = backend
        .simple_query("DISCARD ALL")
        .await
        .expect_err("DISCARD ALL inside a transaction refuses");
    assert_eq!(
        refused.code(),
        Some(&tokio_postgres::error::SqlState::ACTIVE_SQL_TRANSACTION),
        "{refused:?}"
    );
    backend.simple_query("ROLLBACK").await.expect("ROLLBACK");

    // The pooler resets the connection between clients.
    let started = std::time::Instant::now();
    backend
        .simple_query("DISCARD ALL")
        .await
        .expect("DISCARD ALL outside a transaction");
    assert!(started.elapsed() < std::time::Duration::from_secs(1));

    // Client B: client A's statement is gone, and the startup graph is bound again.
    let leaked = backend
        .query(&prepared, &[&1i64])
        .await
        .expect_err("client A's prepared statement does not survive DISCARD ALL");
    assert_eq!(
        leaked.code(),
        Some(&tokio_postgres::error::SqlState::INVALID_SQL_STATEMENT_NAME),
        "{leaked:?}"
    );
    let rows = backend
        .simple_query("SELECT id FROM nodes ORDER BY id")
        .await
        .expect("client B reads the startup graph");
    assert_eq!(simple_ids(rows), commons_ids);

    // Advisory lock functions are not served, so they refuse.
    backend
        .simple_query("SELECT pg_advisory_lock(1)")
        .await
        .expect_err("advisory locks are not served");
}

/// Single-row inserts into a table with a primary key, a foreign key and a check, from
/// 16 concurrent sessions: every valid row lands, every violation refuses with its
/// SQLSTATE, and a primary key raced by every session is won by exactly one.
// spec: MEDULLA-ENGINE-PARITY-R011.3
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn constraints_hold_under_concurrent_sessions() {
    const SESSIONS: i64 = 16;
    const VALID_PER_SESSION: i64 = 20;
    const RACED_ID: i64 = 1_000_000;
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let admin = connect(&addr).await;
    let parents = unique_table();
    let children = unique_table();
    admin
        .simple_query(&format!("CREATE TABLE {parents} (id BIGINT PRIMARY KEY)"))
        .await
        .expect("CREATE parents");
    admin
        .simple_query(&format!(
            "CREATE TABLE {children} (id BIGINT PRIMARY KEY, parent_id BIGINT, bal BIGINT NOT NULL, \
             CONSTRAINT {children}_bal_nonneg CHECK (bal >= 0), \
             CONSTRAINT {children}_parent_fk FOREIGN KEY (parent_id) REFERENCES {parents} (id))"
        ))
        .await
        .expect("CREATE children");
    admin
        .simple_query(&format!("INSERT INTO {parents} (id) VALUES (1)"))
        .await
        .expect("seed parent");

    let mut sessions = Vec::new();
    for session in 0..SESSIONS {
        let addr = addr.clone();
        let children = children.clone();
        sessions.push(tokio::spawn(async move {
            let client = connect(&addr).await;
            let mut refusals: Vec<String> = Vec::new();
            let mut raced_won = false;
            for row in 0..VALID_PER_SESSION {
                let id = session * 1_000 + row;
                client
                    .simple_query(&format!(
                        "INSERT INTO {children} (id, parent_id, bal) VALUES ({id}, 1, {row})"
                    ))
                    .await
                    .expect("a valid row lands");
            }
            let violations = [
                // A duplicate of this session's first row.
                format!(
                    "INSERT INTO {children} (id, parent_id, bal) VALUES ({}, 1, 5)",
                    session * 1_000
                ),
                // A parent that does not exist.
                format!(
                    "INSERT INTO {children} (id, parent_id, bal) VALUES ({}, 99, 5)",
                    session * 1_000 + 500
                ),
                // A failing CHECK.
                format!(
                    "INSERT INTO {children} (id, parent_id, bal) VALUES ({}, 1, -1)",
                    session * 1_000 + 501
                ),
            ];
            for statement in violations {
                let error = client
                    .simple_query(&statement)
                    .await
                    .expect_err("a violation refuses");
                let db = error.as_db_error().expect("a database error");
                refusals.push(db.code().code().to_string());
            }
            match client
                .simple_query(&format!(
                    "INSERT INTO {children} (id, parent_id, bal) VALUES ({RACED_ID}, 1, 0)"
                ))
                .await
            {
                Ok(_) => raced_won = true,
                Err(error) => {
                    let db = error.as_db_error().expect("a database error");
                    refusals.push(db.code().code().to_string());
                }
            }
            (refusals, raced_won)
        }));
    }
    let mut winners = 0;
    for session in sessions {
        let (refusals, raced_won) = session.await.expect("session task");
        assert_eq!(&refusals[..3], ["23505", "23503", "23514"]);
        if raced_won {
            winners += 1;
        } else {
            assert_eq!(refusals[3], "23505", "the lost race is a duplicate key");
        }
    }
    assert_eq!(winners, 1, "exactly one session wins the raced key");
    assert_eq!(
        count_rows(&admin, &children).await as i64,
        SESSIONS * VALID_PER_SESSION + 1,
        "only the valid rows and the one race winner are stored"
    );
}

/// One after-insert trigger on a table; 1,000 pgwire inserts; each trigger effect (a row
/// copied into a second table) ran exactly once.
// spec: MEDULLA-ENGINE-PARITY-R012.3
#[cfg(feature = "statechart")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_insert_trigger_runs_once_per_row() {
    use medulla_statechart::{Action, State, StatechartDef, Transition};

    const ROWS: usize = 1_000;
    let source = unique_table();
    let target = format!("{source}_copy");
    let machine = StatechartDef {
        name: "copy_after_insert".to_string(),
        schema_version: 1,
        states: vec![State::new("idle")],
        alphabet: vec!["after_insert".to_string()],
        transitions: vec![
            Transition::new("idle", "after_insert", "idle").with_actions(vec![Action::Custom {
                name: "insert_row".to_string(),
                args: serde_json::json!({
                    "table": target,
                    "row": {"note": {"from": "new.note"}},
                }),
            }]),
        ],
        initial: "idle".to_string(),
        finals: Vec::new(),
        meta: Default::default(),
    };
    let persist_dir = sql_test_persist_dir();
    let machine_id = medulla_h_jobs::statechart::statechart_store(Some(&persist_dir))
        .expect("statechart store")
        .define(&machine)
        .expect("define the trigger machine");
    let state = state_with_dir(default_persistence(), persist_dir);
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;
    client
        .simple_query(&format!(
            "CREATE TABLE {source} (id BIGINT PRIMARY KEY, note TEXT)"
        ))
        .await
        .expect("CREATE source");
    client
        .simple_query(&format!("CREATE TABLE {target} (note TEXT)"))
        .await
        .expect("CREATE target");
    client
        .simple_query(&format!(
            "CREATE TRIGGER copy_note AFTER INSERT ON {source} EXECUTE MACHINE {machine_id}"
        ))
        .await
        .expect("CREATE TRIGGER");

    for id in 0..ROWS {
        client
            .simple_query(&format!(
                "INSERT INTO {source} (id, note) VALUES ({id}, 'note-{id}')"
            ))
            .await
            .expect("INSERT");
    }

    let rows = client
        .simple_query(&format!("SELECT note FROM {target}"))
        .await
        .expect("read the trigger effects");
    let mut notes: Vec<String> = rows
        .into_iter()
        .filter_map(|message| match message {
            tokio_postgres::SimpleQueryMessage::Row(row) => row.get(0).map(str::to_string),
            _ => None,
        })
        .collect();
    notes.sort();
    let mut expected: Vec<String> = (0..ROWS).map(|id| format!("note-{id}")).collect();
    expected.sort();
    assert_eq!(notes, expected, "each trigger effect ran exactly once");

    let _ = client.simple_query(&format!("DROP TABLE {source}")).await;
    let _ = client.simple_query(&format!("DROP TABLE {target}")).await;
}

/// A view reads through its stored select, a materialized view stores its rows
/// until a refresh recomputes them, and the two refusals carry their codes.
// spec: MEDULLA-ENGINE-PARITY-R014.1
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn view_and_matview_over_wire() {
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;
    let table = unique_table();
    let view = format!("{table}_v");
    let matview = format!("{table}_mv");

    client
        .simple_query(&format!("CREATE TABLE {table} (id BIGINT PRIMARY KEY, sku TEXT)"))
        .await
        .expect("CREATE TABLE");
    client
        .simple_query(&format!("INSERT INTO {table} (id, sku) VALUES (1, 'a')"))
        .await
        .expect("seed row");
    client
        .simple_query(&format!("CREATE VIEW {view} AS SELECT id, sku FROM {table}"))
        .await
        .expect("CREATE VIEW");
    client
        .simple_query(&format!(
            "CREATE MATERIALIZED VIEW {matview} AS SELECT id, sku FROM {table}"
        ))
        .await
        .expect("CREATE MATERIALIZED VIEW");

    client
        .simple_query(&format!("INSERT INTO {table} (id, sku) VALUES (2, 'b')"))
        .await
        .expect("second row");
    let live = simple_ids(
        client
            .simple_query(&format!("SELECT id FROM {view} ORDER BY id"))
            .await
            .expect("read the view"),
    );
    assert_eq!(live, vec!["1".to_string(), "2".to_string()]);
    let stored = simple_ids(
        client
            .simple_query(&format!("SELECT id FROM {matview} ORDER BY id"))
            .await
            .expect("read the matview"),
    );
    assert_eq!(stored, vec!["1".to_string()], "the matview keeps its rows");

    client
        .simple_query(&format!("REFRESH MATERIALIZED VIEW {matview}"))
        .await
        .expect("REFRESH MATERIALIZED VIEW");
    let refreshed = simple_ids(
        client
            .simple_query(&format!("SELECT id FROM {matview} ORDER BY id"))
            .await
            .expect("read the refreshed matview"),
    );
    assert_eq!(refreshed, vec!["1".to_string(), "2".to_string()]);

    let exists = client
        .simple_query(&format!(
            "CREATE MATERIALIZED VIEW {matview} AS SELECT id FROM {table}"
        ))
        .await
        .expect_err("a second create of one name refuses");
    assert_eq!(
        exists.as_db_error().expect("a database error").code().code(),
        "42P07",
        "{}",
        error_chain(&exists)
    );
    assert!(error_chain(&exists).contains("matview_exists"));

    client
        .simple_query(&format!("DROP MATERIALIZED VIEW {matview}"))
        .await
        .expect("DROP MATERIALIZED VIEW");
    let unknown = client
        .simple_query(&format!("REFRESH MATERIALIZED VIEW {matview}"))
        .await
        .expect_err("a refresh of a dropped view refuses");
    assert_eq!(
        unknown.as_db_error().expect("a database error").code().code(),
        "42P01",
        "{}",
        error_chain(&unknown)
    );
    assert!(error_chain(&unknown).contains("matview_unknown"));
    client
        .simple_query(&format!("DROP MATERIALIZED VIEW IF EXISTS {matview}"))
        .await
        .expect("IF EXISTS of an absent view succeeds");
}

/// A materialized view over 100,000 rows refreshes to the same row count and checksum as
/// its input, and a read of the view answers from the stored rows.
// spec: MEDULLA-ENGINE-PARITY-R014.3
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matview_refresh_checksum() {
    const ROWS: i64 = 100_000;
    const BATCH: i64 = 1_000;
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;
    let table = unique_table();
    let matview = format!("{table}_mv");

    client
        .simple_query(&format!("CREATE TABLE {table} (id BIGINT PRIMARY KEY, v BIGINT)"))
        .await
        .expect("CREATE TABLE");
    client
        .simple_query(&format!(
            "CREATE MATERIALIZED VIEW {matview} AS SELECT id, v FROM {table}"
        ))
        .await
        .expect("CREATE MATERIALIZED VIEW over an empty table");
    for first in (0..ROWS).step_by(BATCH as usize) {
        let values: Vec<String> = (first..first + BATCH)
            .map(|id| format!("({id}, {})", id * 7))
            .collect();
        client
            .simple_query(&format!(
                "INSERT INTO {table} (id, v) VALUES {}",
                values.join(", ")
            ))
            .await
            .expect("insert a batch");
    }
    client
        .simple_query(&format!("REFRESH MATERIALIZED VIEW {matview}"))
        .await
        .expect("REFRESH MATERIALIZED VIEW");

    let expected_sum: i64 = (0..ROWS).map(|id| id * 7).sum();
    let summary = |relation: String| {
        let client = &client;
        async move {
            let msgs = client
                .simple_query(&format!("SELECT count(*), sum(v) FROM {relation}"))
                .await
                .expect("summarize");
            msgs.iter()
                .find_map(|m| match m {
                    tokio_postgres::SimpleQueryMessage::Row(row) => Some((
                        row.get(0).map(str::to_string),
                        row.get(1).map(str::to_string),
                    )),
                    _ => None,
                })
                .expect("one summary row")
        }
    };
    let from_view = summary(matview.clone()).await;
    let from_table = summary(table.clone()).await;
    assert_eq!(from_view.0.as_deref(), Some(ROWS.to_string().as_str()));
    assert_eq!(from_view.1.as_deref(), Some(expected_sum.to_string().as_str()));
    assert_eq!(from_view, from_table, "the view equals its input");

    let _ = client.simple_query(&format!("DROP MATERIALIZED VIEW {matview}")).await;
    let _ = client.simple_query(&format!("DROP TABLE {table}")).await;
}

// ─── pg_dump and pg_restore round trip (MEDULLA-DURABLE-KERNEL-R035.2) ──────────────────

/// Rows of each of the two tables the dump and restore carry.
const DUMP_ROWS: i64 = 10_000;
/// The wall-clock budget of the dump and the restore together.
const DUMP_WALL_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);
/// The most resident memory the round trip may add, in kibibytes (256 MiB).
const DUMP_RESIDENT_BUDGET_KIB: u64 = 256 * 1024;

/// A PostgreSQL client binary: the file named by `variable`, else the first `name` on
/// `PATH`. A missing binary fails the test naming both variables; it never skips.
fn postgres_client_binary(variable: &str, name: &str) -> std::path::PathBuf {
    if let Some(path) = std::env::var_os(variable).filter(|value| !value.is_empty()) {
        return std::path::PathBuf::from(path);
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| {
            panic!(
                "no PostgreSQL client `{name}` found: set MEDULLA_PG_DUMP and \
                 MEDULLA_PG_RESTORE to the pg_dump and pg_restore binaries, or put \
                 both on PATH"
            )
        })
}

/// The peak resident set of this process, in kibibytes (`VmHWM`).
fn peak_resident_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmHWM:"))
                .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse().ok())
        })
        .unwrap_or(0)
}

/// Every `(id, label)` row of `table`, in id order.
async fn table_rows(client: &tokio_postgres::Client, table: &str) -> Vec<(i64, String)> {
    client
        .query(&format!("SELECT id, label FROM {table} ORDER BY id"), &[])
        .await
        .expect("read the table back")
        .iter()
        .map(|row| (row.get::<_, i64>("id"), row.get::<_, String>("label")))
        .collect()
}

/// The digest of one table's rows: equal rows hash equal, in order.
fn rows_digest(rows: &[(i64, String)]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for (id, label) in rows {
        hasher.update(id.to_be_bytes());
        hasher.update((label.len() as u64).to_be_bytes());
        hasher.update(label.as_bytes());
    }
    hex::encode(hasher.finalize())
}

// spec: MEDULLA-DURABLE-KERNEL-R035.2
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pg_dump_then_pg_restore_round_trips_two_tables() {
    use std::process::Command;

    let pg_dump = postgres_client_binary("MEDULLA_PG_DUMP", "pg_dump");
    let pg_restore = postgres_client_binary("MEDULLA_PG_RESTORE", "pg_restore");

    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let (host, port) = addr.rsplit_once(':').expect("listener host:port");
    let client = connect(&addr).await;

    // Two tables of 10 000 rows each, written in batches.
    for table in ["dump_alpha", "dump_beta"] {
        client
            .simple_query(&format!(
                "CREATE TABLE {table} (id BIGINT PRIMARY KEY, label TEXT)"
            ))
            .await
            .expect("CREATE TABLE");
        for start in (0..DUMP_ROWS).step_by(500) {
            let values = (start..(start + 500).min(DUMP_ROWS))
                .map(|id| format!("({id}, '{table}-{id}')"))
                .collect::<Vec<_>>()
                .join(",");
            client
                .simple_query(&format!("INSERT INTO {table} (id, label) VALUES {values}"))
                .await
                .expect("INSERT batch");
        }
    }
    let before_alpha = rows_digest(&table_rows(&client, "dump_alpha").await);
    let before_beta = rows_digest(&table_rows(&client, "dump_beta").await);

    let scratch = tempfile::tempdir().expect("scratch directory");
    let archive = scratch.path().join("dump.pgdump");
    let password = pgwire::derive_pg_password("test", "tester");
    let resident_before = peak_resident_kib();
    let began = std::time::Instant::now();

    // Dump both tables in the custom format through the real client.
    let dumped = Command::new(&pg_dump)
        .args(["--format=custom", "--no-password", "--no-owner", "--no-privileges"])
        .args(["--host", host, "--port", port])
        .args(["--username", "tester", "--dbname", "__commons__"])
        .args(["--table", "dump_alpha", "--table", "dump_beta"])
        .arg("--file")
        .arg(&archive)
        .env("PGPASSWORD", &password)
        .output()
        .expect("run pg_dump");
    assert!(
        dumped.status.success(),
        "pg_dump failed: {}",
        String::from_utf8_lossy(&dumped.stderr)
    );
    assert!(archive.is_file() && archive.metadata().unwrap().len() > 0);

    // Drop the tables, then restore them from the archive.
    for table in ["dump_alpha", "dump_beta"] {
        client
            .simple_query(&format!("DROP TABLE {table}"))
            .await
            .expect("DROP TABLE");
    }
    let restored = Command::new(&pg_restore)
        .args(["--no-password", "--no-owner", "--no-privileges"])
        .args(["--host", host, "--port", port])
        .args(["--username", "tester", "--dbname", "__commons__"])
        .arg(&archive)
        .env("PGPASSWORD", &password)
        .output()
        .expect("run pg_restore");
    assert!(
        restored.status.success(),
        "pg_restore failed: {}",
        String::from_utf8_lossy(&restored.stderr)
    );
    let elapsed = began.elapsed();

    // Every row came back, table by table.
    let after_alpha = table_rows(&client, "dump_alpha").await;
    let after_beta = table_rows(&client, "dump_beta").await;
    assert_eq!(after_alpha.len() as i64, DUMP_ROWS);
    assert_eq!(after_beta.len() as i64, DUMP_ROWS);
    assert_eq!(rows_digest(&after_alpha), before_alpha);
    assert_eq!(rows_digest(&after_beta), before_beta);

    // The budget: the round trip finishes in time and adds little resident memory.
    assert!(
        elapsed < DUMP_WALL_BUDGET,
        "dump and restore took {elapsed:?}, budget {DUMP_WALL_BUDGET:?}"
    );
    let added_kib = peak_resident_kib().saturating_sub(resident_before);
    assert!(
        added_kib < DUMP_RESIDENT_BUDGET_KIB,
        "dump and restore added {added_kib} KiB resident, budget {DUMP_RESIDENT_BUDGET_KIB} KiB"
    );

    for table in ["dump_alpha", "dump_beta"] {
        let _ = client.simple_query(&format!("DROP TABLE {table}")).await;
    }
}

// spec: MEDULLA-ENGINE-PARITY-R006.3
/// A prepared point read executed 10,000 times returns the seeded value for every id
/// and is served from the connection's plan cache: the process counter rises by at
/// least one hit per execution after the first. The 1 ms p99 budget runs only in the
/// nightly group `engine_parity/pgwire_prepared_read`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn prepared_point_read_plan_cache_hits() {
    const ROWS: i64 = 1_000;
    const EXECUTIONS: i64 = 10_000;
    let state = seeded_state();
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;
    let table = unique_table();

    client
        .simple_query(&format!(
            "CREATE TABLE {table} (id BIGINT PRIMARY KEY, v BIGINT NOT NULL)"
        ))
        .await
        .expect("CREATE TABLE");
    for chunk in (0..ROWS).collect::<Vec<_>>().chunks(100) {
        let values: Vec<String> = chunk
            .iter()
            .map(|id| format!("({id}, {})", id * 7 + 3))
            .collect();
        client
            .simple_query(&format!(
                "INSERT INTO {table} (id, v) VALUES {}",
                values.join(", ")
            ))
            .await
            .expect("seed rows");
    }

    let stmt = client
        .prepare(&format!("SELECT v FROM {table} WHERE id = $1"))
        .await
        .expect("prepare the point read once");
    let hits_before = pgwire::plan_cache_hits();
    for execution in 0..EXECUTIONS {
        let id = execution % ROWS;
        let rows = client
            .query(&stmt, &[&id])
            .await
            .expect("prepared point read");
        assert_eq!(rows.len(), 1, "id {id} returns one row");
        assert_eq!(rows[0].get::<_, i64>(0), id * 7 + 3, "value of id {id}");
    }
    let hits = pgwire::plan_cache_hits() - hits_before;
    assert!(
        hits >= (EXECUTIONS - 1) as u64,
        "the plan cache served {hits} of {EXECUTIONS} executions"
    );
    let _ = client.simple_query(&format!("DROP TABLE {table}")).await;
}

// ── Catalog views for outside tools (MEDULLA-ENGINE-PARITY-R008.2) ──────────────

/// The grant a principal holds on a user table is the SQL privilege
/// `SqlPrivilege::Select`; the catalog views list exactly the tables the
/// principal may select, and the 40 introspection queries of an outside tool
/// replay without error.
// spec: MEDULLA-ENGINE-PARITY-R008.2
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bi_tool_introspection_queries_pass() {
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use medulla_server_api::carrier::CarrierAuthority;
    use medulla_server_auth::sql_catalog_acl::SqlPrivilege;

    const QUERIES: &str = include_str!("fixtures/pg_introspection/queries.sql");
    let statements: Vec<&str> = QUERIES.lines().collect();
    assert_eq!(statements.len(), 40, "the fixture holds 40 statements");
    assert!(
        statements
            .iter()
            .all(|line| !line.trim().is_empty() && !line.trim_start().starts_with("--")),
        "no blank or comment lines"
    );
    let distinct: std::collections::BTreeSet<&str> = statements.iter().copied().collect();
    assert_eq!(distinct.len(), 40, "the 40 statements are distinct");

    let state = login_state(&["intro-owner", "intro-reader"]);
    let persist_dir = state
        .read()
        .await
        .persist_dir
        .clone()
        .expect("persist dir");
    let addr = spawn_listener_mode(state.clone(), pgwire::PgWireAuthMode::Scram).await;
    register_password_login(&state, "intro-owner-login", "intro-owner", common::TEST_TENANT).await;
    register_password_login(
        &state,
        "intro-reader-login",
        "intro-reader",
        common::TEST_TENANT,
    )
    .await;

    let owner = connect_scram(&addr, "intro-owner-login", LOGIN_PASSWORD, "__commons__")
        .await
        .expect("owner login");
    owner
        .simple_query("CREATE TABLE t_visible (id BIGINT PRIMARY KEY, name TEXT)")
        .await
        .expect("CREATE TABLE t_visible");
    owner
        .simple_query("CREATE TABLE t_hidden (id BIGINT PRIMARY KEY)")
        .await
        .expect("CREATE TABLE t_hidden");

    let owner_carrier = CarrierAuthority::from_verified(
        &VerifiedRequestContext::verified_for_test_in_tenant(
            "intro-owner",
            common::TEST_TENANT,
        ),
    )
    .unwrap();
    medulla_server_auth::sql_catalog_acl::grant(
        std::path::Path::new(&persist_dir),
        &owner_carrier,
        "t_visible",
        "intro-reader",
        &[SqlPrivilege::Select],
        uuid::Uuid::new_v4(),
    )
    .unwrap();

    let reader = connect_scram(&addr, "intro-reader-login", LOGIN_PASSWORD, "__commons__")
        .await
        .expect("grantee login");
    for statement in &statements {
        reader
            .simple_query(statement)
            .await
            .unwrap_or_else(|err| panic!("{statement}: {}", error_chain(&err)));
    }

    let relnames = "SELECT relname FROM pg_catalog.pg_class WHERE relkind = 'r'";
    let tables = "SELECT table_name FROM information_schema.tables";
    for query in [relnames, tables] {
        let seen = simple_ids(reader.simple_query(query).await.expect("reader catalog read"));
        assert!(
            seen.iter().any(|name| name == "t_visible"),
            "{query}: the grantee lists t_visible, got {seen:?}"
        );
        assert!(
            !seen.iter().any(|name| name == "t_hidden"),
            "{query}: the grantee does not list t_hidden, got {seen:?}"
        );
        let owned = simple_ids(owner.simple_query(query).await.expect("owner catalog read"));
        assert!(
            owned.iter().any(|name| name == "t_visible")
                && owned.iter().any(|name| name == "t_hidden"),
            "{query}: the owner lists both tables, got {owned:?}"
        );
    }
}

// ── SQL dialect compatibility matrix (MEDULLA-ENGINE-PARITY-R009.3) ─────────────

/// The one-state `stamp_before_insert` machine the trigger forms run against.
#[cfg(feature = "statechart")]
fn stamp_before_insert_machine() -> medulla_statechart::StatechartDef {
    use medulla_statechart::{Action, ActionValue, State, StatechartDef, Transition};

    StatechartDef {
        name: "stamp_before_insert".to_string(),
        schema_version: 1,
        states: vec![State::new("idle")],
        alphabet: vec!["before_insert".to_string()],
        transitions: vec![
            Transition::new("idle", "before_insert", "idle").with_actions(vec![Action::Assign {
                key: "new.status".to_string(),
                value: ActionValue::Const {
                    value: serde_json::json!("open"),
                },
            }]),
        ],
        initial: "idle".to_string(),
        finals: Vec::new(),
        meta: Default::default(),
    }
}

/// Every `supported` and `partial` entry of the compatibility matrix succeeds
/// over pgwire on its recorded statement, and each of the five refused forms
/// refuses with `0A000`.
// spec: MEDULLA-ENGINE-PARITY-R009.3
#[cfg(feature = "statechart")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compat_matrix_entries_replay_over_pgwire() {
    use bytes::Bytes;
    use futures_util::{SinkExt, TryStreamExt};
    use medulla_mutation_types::pg_operational_surface::{PgCompatibilityMatrix, SqlFormState};

    const STATEMENTS: &str = include_str!("fixtures/sql_compat/statements.json");
    const REFUSED: [&str; 5] = [
        "create_policy",
        "create_role",
        "create_publication",
        "create_rule",
        "create_type",
    ];

    let entries: Vec<serde_json::Value> =
        serde_json::from_str(STATEMENTS).expect("the statements fixture is a JSON array");
    let text = |entry: &serde_json::Value, key: &str| -> String {
        entry[key]
            .as_str()
            .unwrap_or_else(|| panic!("entry {entry} lacks `{key}`"))
            .to_string()
    };
    let texts = |entry: &serde_json::Value, key: &str| -> Vec<String> {
        entry[key]
            .as_array()
            .unwrap_or_else(|| panic!("entry {entry} lacks `{key}`"))
            .iter()
            .map(|v| v.as_str().expect("a statement text").to_string())
            .collect()
    };

    // The fixture's supported and partial form set equals the matrix's entry set.
    let matrix = PgCompatibilityMatrix::with_served_forms();
    let served: std::collections::BTreeSet<String> = matrix
        .forms
        .iter()
        .filter(|f| f.state != SqlFormState::Refused)
        .map(|f| f.name.clone())
        .collect();
    let fixture_served: std::collections::BTreeSet<String> = entries
        .iter()
        .filter(|e| e["state"] != "refused")
        .map(|e| text(e, "form"))
        .collect();
    for name in &served {
        assert!(
            fixture_served.contains(name),
            "matrix entry `{name}` has no entry in the statements fixture"
        );
    }
    assert_eq!(fixture_served, served, "the fixture covers the matrix entries");
    let fixture_refused: Vec<String> = entries
        .iter()
        .filter(|e| e["state"] == "refused")
        .map(|e| text(e, "form"))
        .collect();
    assert_eq!(fixture_refused, REFUSED, "the five closed refused forms");
    for name in REFUSED {
        assert_eq!(
            matrix.resolve_form(name),
            SqlFormState::Refused,
            "`{name}` is absent from the matrix, so a later served form fails here by name"
        );
        assert!(
            matrix.form_state(name).is_none(),
            "`{name}` has no matrix entry"
        );
    }

    let persist_dir = sql_test_persist_dir();
    let machine_id = medulla_h_jobs::statechart::statechart_store(Some(&persist_dir))
        .expect("statechart store")
        .define(&stamp_before_insert_machine())
        .expect("define the trigger machine");
    let state = state_with_dir(default_persistence(), persist_dir);
    let addr = spawn_listener(state).await;
    let client = connect(&addr).await;

    for entry in &entries {
        let form = text(entry, "form");
        let machine = entry["machine"].as_bool().unwrap_or(false);
        let bind = |sql: String| {
            if machine {
                sql.replace("{machine_id}", &machine_id.to_string())
            } else {
                sql
            }
        };
        let sql = bind(text(entry, "sql"));

        if entry["state"] == "refused" {
            let err = client
                .simple_query(&sql)
                .await
                .expect_err(&format!("`{form}` is refused"));
            assert_eq!(
                err.code(),
                Some(&tokio_postgres::error::SqlState::FEATURE_NOT_SUPPORTED),
                "`{form}` refuses 0A000, got {}",
                error_chain(&err)
            );
            assert!(
                error_chain(&err).contains(&form),
                "`{form}`: the message names the entry, got {}",
                error_chain(&err)
            );
            continue;
        }

        for statement in texts(entry, "setup") {
            client
                .simple_query(&bind(statement.clone()))
                .await
                .unwrap_or_else(|err| panic!("{form} setup `{statement}`: {}", error_chain(&err)));
        }
        match entry["client"].as_str() {
            Some("copy_in") => {
                let sink = client
                    .copy_in::<_, Bytes>(sql.as_str())
                    .await
                    .unwrap_or_else(|err| panic!("{form}: {}", error_chain(&err)));
                futures_util::pin_mut!(sink);
                sink.send(Bytes::from(text(entry, "copy_data")))
                    .await
                    .unwrap_or_else(|err| panic!("{form}: {}", error_chain(&err)));
                let rows = sink
                    .finish()
                    .await
                    .unwrap_or_else(|err| panic!("{form}: {}", error_chain(&err)));
                assert_eq!(
                    rows,
                    entry["expect_rows"].as_u64().expect("expect_rows"),
                    "{form}: rows copied in"
                );
            }
            Some("copy_out") => {
                let stream = client
                    .copy_out(sql.as_str())
                    .await
                    .unwrap_or_else(|err| panic!("{form}: {}", error_chain(&err)));
                let chunks: Vec<Bytes> = stream
                    .try_collect()
                    .await
                    .unwrap_or_else(|err| panic!("{form}: {}", error_chain(&err)));
                let data: Vec<u8> = chunks.iter().flat_map(|c| c.iter().copied()).collect();
                assert_eq!(
                    String::from_utf8(data).expect("utf-8 copy data"),
                    text(entry, "expect_data"),
                    "{form}: data copied out"
                );
            }
            Some(other) => panic!("{form}: unknown client `{other}`"),
            None => {
                client
                    .simple_query(&sql)
                    .await
                    .unwrap_or_else(|err| panic!("{form} `{sql}`: {}", error_chain(&err)));
            }
        }
        for statement in texts(entry, "cleanup") {
            client
                .simple_query(&bind(statement.clone()))
                .await
                .unwrap_or_else(|err| panic!("{form} cleanup `{statement}`: {}", error_chain(&err)));
        }
    }
}
