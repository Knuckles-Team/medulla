#![allow(unused_imports)]
//! Wire tests that drive the real dispatcher; they live in the facade because dispatch does.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use medulla_core::isolation::AccessLevel;
use medulla_protocol_types::protocol::Request;
use medulla_query::{
    AlterTableAction, AlterTablePlan, AnnIndexPlan, Column, ColumnType, ContinuousAggPlan,
    CopyFormat, CopyPlan, CreateFunctionPlan, CreateTablePlan, CreateViewPlan, DeleteNodes,
    DeleteNodesJoin, DeleteTable, DropFunctionPlan, DropTablePlan, DropViewPlan, HypertablePlan,
    IndexCatalogTxnOp, InsertNodes, InsertNodesSelect, InsertSelect, InsertTable, OnConflict,
    OnConflictAction, PgColType, PropertyGraphTxnOp, StatementKind, TableSchema, TableStore,
    TableTxn, TxnOp, TypedColumn, TypedQueryResult, UpdateNodes, UpdateNodesJoin, UpdateTable,
    WhereEq,
};
use medulla_server_api::access::CarrierAuthority;
use medulla_server_api::state::ServerState;
use medulla_wire::wire::*;

#[cfg(test)]
mod ne_004_ne_005_tests {
    //! Unit tests for NE-004 (mixed graph+table transaction atomicity via a
    //! durable commit-intent) and NE-005 (SQL-selectable isolation level).
    //!
    //! These construct a `WireSession` + `ServerState` directly (in-crate, so
    //! `pub(crate)`/private items are reachable) rather than driving a real
    //! wire listener — the SAME lighter-weight approach `medulla_server::mod::tests`
    //! uses for its own dispatch-level tests. `bind_authenticated_sql_actor`
    //! is the SAME native-SQL-identity seam `sqlite_wire`/`mysql_wire` use;
    //! it is not a test-only shortcut.
    use super::*;
    use medulla_protocol_types::protocol::GraphType;
    use medulla_protocol_types::protocol::Method;

    const SECRET: &str = "medulla-txn-atomicity-test-secret";
    const AGENT: &str = "txn-test-agent";

    fn ensure_env() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            std::env::set_var("MEDULLA_AUDIENCE", "medulla-test");
            std::env::set_var("MEDULLA_TENANT", "tenant-shared");
            std::env::set_var("MEDULLA_POLICY_VERSION", "policy-test");
            // MEDULLA_ENCRYPTION_KEY is deliberately NOT set here. It used to
            // be, through this `Once` and holding NOTHING, and to a value different
            // from the six other modules' -- so this `Once` could change the ambient
            // at-rest key in the middle of ANOTHER test's body no matter what guard
            // that test held, and did: it is what failed
            // `redb_backend::tests::online_reshard_moves_graph_live_no_loss` with
            // "decryption failed (wrong key or tampered ciphertext)" under
            // parallelism while it passed standalone. Every test below now takes
            // `medulla_kernel::crypto::provisioned_test_env_read_lock()`, which provisions the
            // ONE shared key under the WRITE guard and then holds the READ guard for
            // the test's whole body. See `medulla_kernel::crypto::TEST_AT_REST_KEY`'s doc.
        });
    }

    /// A real durable `ServerState` on its own uniquely-named temp dir (mirrors
    /// `medulla_server::mod::tests::test_state`, trimmed to nothing this module's
    /// tests don't need). `AGENT` is registered `AgentRole::System` so every
    /// access check trivially passes regardless of the `security` feature —
    /// this module tests transaction ATOMICITY, not RBAC (that is covered
    /// elsewhere).
    fn test_state() -> Arc<RwLock<ServerState>> {
        ensure_env();
        medulla_server::test_state_with_services(
            SECRET,
            test_isolation(&[AGENT]),
            "medulla-wire-txn-atomicity-test",
            "medulla-wire-txn-atomicity-tsdb-test",
        )
    }

    /// Build a signed `Request` exactly like `medulla_server::mod::tests::request` —
    /// only used here to drive the ONE real `Method::CreateGraph` RPC so a
    /// test graph is durably initialized the SAME way production does it
    /// (never by poking the registry directly).
    fn request(id: u64, graph: &str, method: Method) -> Request {
        static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let claims = medulla_server_api::authority_context::signed_test_claims(AGENT);
        let mut req = Request {
            id,
            graph: graph.to_string(),
            auth_token: String::new(),
            agent_id: Some(AGENT.to_string()),
            method,
        };
        let nonce = format!(
            "ne004-005-{}-{}",
            std::process::id(),
            NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        req.auth_token = medulla_server_api::auth::compute_verified_envelope_token(
            SECRET,
            &req,
            &medulla_server_api::auth::VerifiedEnvelopeParams {
                context: &claims,
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("system clock")
                    .as_secs(),
                nonce: &nonce,
                idempotency_key: &format!("ne004-005-request-{id}"),
            },
        );
        req
    }

    async fn create_test_graph(state: &Arc<RwLock<ServerState>>, graph: &str, id: u64) {
        let resp = medulla_server::dispatch(
            state,
            request(
                id,
                graph,
                Method::CreateGraph {
                    graph_name: graph.to_string(),
                    graph_type: GraphType::Agent,
                },
            ),
        )
        .await;
        assert!(resp.error.is_none(), "CreateGraph failed: {:?}", resp.error);
    }

    /// A fresh, authenticated `WireSession` — a new "connection" sharing
    /// `state`, exactly the shape a crash-then-reconnect looks like: no
    /// in-memory buffer carries over, only what is durable on disk does.
    async fn new_session(state: Arc<RwLock<ServerState>>, graph: &str) -> WireSession {
        let session = WireSession::new(state, graph.to_string());
        session
            .bind_authenticated_sql_actor("pgwire", AGENT)
            .await
            .expect("bind test identity");
        session
    }

    async fn node_count(session: &WireSession, graph: &str, id: &str) -> usize {
        match session
            .run_read(graph, format!("SELECT id FROM nodes WHERE id = '{id}'"))
            .await
        {
            Ok(result) => result.rows.len(),
            Err(_) => 0,
        }
    }

    async fn table_row_count(session: &WireSession, graph: &str, sql: &str) -> usize {
        match session.run_read(graph, sql.to_string()).await {
            Ok(result) => result.rows.len(),
            Err(_) => 0,
        }
    }

    /// Route a read through the FULL `execute()` dispatch (not the lower-level
    /// `run_read` the other helpers use) — this is the ONLY entry point that
    /// runs the NE-005 auto predicate-tracking hook, so a test proving
    /// `SERIALIZABLE` behavior MUST issue its reads this way for the
    /// transaction's `serializable_predicate_reads` to actually populate.
    async fn read_row_count(session: &WireSession, sql: &str) -> usize {
        match session.execute(sql).await.expect("read") {
            WireOutcome::Rows(result) => result.rows.len(),
            other => panic!("expected a row set, got {other:?}"),
        }
    }

    /// Pins the state a freshly constructed session starts in: its default graph,
    /// no actor or transaction, snapshot isolation, and every flag/buffer cleared.
    #[test]
    fn new_session_starts_unbound_on_its_default_graph() {
        let state = Arc::new(RwLock::new(ServerState::new_for_test(
            SECRET,
            test_isolation(&[AGENT]),
        )));
        let session = WireSession::new(state, "g-default".to_string());
        assert_eq!(session.current_graph(), "g-default");
        assert!(!session.in_txn() && session.actor().is_none());
        assert!(session.authority.lock().is_none() && session.attempt_nonce.lock().is_none());
        assert!(session.txn_graph.lock().is_none() && session.copy.lock().is_none());
        assert!(!*session.txn_failed.lock());
        assert!(!session
            .startup_resolved
            .load(std::sync::atomic::Ordering::SeqCst));
        assert!(!session
            .recovered_intents
            .load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            *session.txn_isolation.lock(),
            medulla_server_api::txn::IsolationLevel::Snapshot
        );
        assert!(session.txn_begin_version.lock().is_none());
        assert!(session.session_isolation_default.lock().is_none());
        assert!(session.pending_isolation.lock().is_none());
        assert!(session.serializable_predicate_reads.lock().is_empty());
        assert!(session.txn_replay_log.lock().is_empty());
    }

    /// NE-004, DoD item 1: a mixed graph+table transaction commits
    /// atomically — both sides are visible after `COMMIT`.
    #[tokio::test]
    async fn mixed_txn_commits_atomically() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let state = test_state();
        let graph = "ne004-atomic";
        create_test_graph(&state, graph, 1).await;
        let session = new_session(state.clone(), graph).await;
        session
            .execute("CREATE TABLE ledger (id INT, note TEXT)")
            .await
            .expect("create table");

        session.execute("BEGIN").await.expect("begin");
        session
            .execute("INSERT INTO nodes (id) VALUES ('atomic-node')")
            .await
            .expect("buffer graph insert");
        session
            .execute("INSERT INTO ledger (id, note) VALUES (1, 'paired')")
            .await
            .expect("buffer table insert");
        session.execute("COMMIT").await.expect("commit");

        assert_eq!(node_count(&session, graph, "atomic-node").await, 1);
        assert_eq!(
            table_row_count(&session, graph, "SELECT id FROM ledger WHERE id = 1").await,
            1
        );
    }

    /// NE-004, DoD item 2: an injected failure AT the table-commit step
    /// (here, a natural one — the target table does not exist, which
    /// `medulla_query::classify` cannot detect syntactically, only the table
    /// store's own commit-time schema lookup can) leaves ZERO graph writes
    /// applied. The graph side already landed durably by the time the table
    /// commit is attempted; this proves the synchronous COMPENSATION path
    /// (not just the crash-recovery path) actually undoes it.
    #[tokio::test]
    async fn mixed_txn_table_failure_leaves_zero_graph_writes() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let state = test_state();
        let graph = "ne004-compensate";
        create_test_graph(&state, graph, 2).await;
        let session = new_session(state.clone(), graph).await;

        session.execute("BEGIN").await.expect("begin");
        session
            .execute("INSERT INTO nodes (id) VALUES ('compensated-node')")
            .await
            .expect("buffer graph insert");
        session
            .execute("INSERT INTO nonexistent_table (id) VALUES (1)")
            .await
            .expect("buffering a DML statement never validates the target table");
        let commit_result = session.execute("COMMIT").await;
        assert!(
            commit_result.is_err(),
            "COMMIT against a nonexistent table must fail"
        );

        assert_eq!(
            node_count(&session, graph, "compensated-node").await,
            0,
            "the graph write must be compensated away, not left applied"
        );
    }

    /// NE-004, DoD item 3: an injected failure AFTER the graph commit but
    /// BEFORE the table commit is detected and resolved on restart with no
    /// torn state.
    ///
    /// Simulates the crash by running exactly the first half of
    /// `commit_mixed_txn` (write the durable intent, then commit the graph
    /// side) and deliberately stopping there — never calling the table
    /// commit or deleting the intent, exactly the state a real process death
    /// in that window leaves on disk. The graph write already went through
    /// the SAME `commit_cross_modal_txn` / redb `WriteTransaction::commit()`
    /// path an ordinary commit uses, so it is genuinely durable, not merely
    /// in-memory. A brand-new `WireSession` (no buffers/state carried over —
    /// the connection-level analogue of a process restart) then runs one
    /// ordinary statement, which lazily triggers recovery.
    #[tokio::test]
    async fn mixed_txn_crash_between_commits_recovers_on_restart() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let state = test_state();
        let graph = "ne004-crash-recovery";
        create_test_graph(&state, graph, 3).await;
        let session = new_session(state.clone(), graph).await;
        session
            .execute("CREATE TABLE audit (id INT, note TEXT)")
            .await
            .expect("create table");

        session.execute("BEGIN").await.expect("begin");
        session
            .execute("INSERT INTO nodes (id) VALUES ('crash-node')")
            .await
            .expect("buffer graph insert");
        session
            .execute("INSERT INTO audit (id, note) VALUES (7, 'pending')")
            .await
            .expect("buffer table insert");

        // ---- inline, truncated `commit_mixed_txn`: stop after phase 1 ----
        let (table_txn, node_ops) = session.take_txn();
        let table_txn = table_txn.expect("table ops were buffered");
        let methods = WireSession::node_ops_to_methods(&node_ops).expect("lower node ops");
        let isolation = *session.txn_isolation.lock();
        let compensating = session
            .compensating_methods(graph, &methods)
            .await
            .expect("snapshot pre-txn state");
        let table_steps = std::mem::take(&mut *session.txn_replay_log.lock());
        let operation_id = uuid::Uuid::new_v4();
        let intent = medulla_server_api::api::query_txn::txn_intent::CommitIntent::new(
            graph.to_string(),
            operation_id,
            methods.clone(),
            compensating,
            table_steps,
            medulla_server_api::txn::now_ms(),
        );
        let authority = session.carrier_authority().expect("bound authority");
        let persist_dir_buf = state
            .read()
            .await
            .persist_dir
            .clone()
            .expect("persist dir configured");
        let persist_dir = std::path::Path::new(&persist_dir_buf);
        medulla_server_api::api::query_txn::txn_intent::write_intent(&authority, persist_dir, &intent)
            .expect("write durable intent");
        let begin_version = session
            .require_txn_begin_version()
            .expect("BEGIN captured a real begin-time version");
        session
            .commit_graph_methods_with_op(
                graph,
                methods,
                operation_id,
                isolation,
                begin_version,
                false,
            )
            .await
            .expect("phase 1: graph commit");
        // `table_txn` is deliberately never committed and the intent is
        // deliberately never deleted — this IS the crash.
        drop(table_txn);
        drop(session);

        // The torn state is real: the graph write is durable, the table
        // write is not, and the intent survives on disk.
        assert_eq!(
            medulla_server_api::api::query_txn::txn_intent::list_intents(&authority, persist_dir).len(),
            1,
            "the crashed intent must still be on disk"
        );
        let probe = new_session(state.clone(), graph).await;
        assert_eq!(node_count(&probe, graph, "crash-node").await, 1);
        assert_eq!(
            table_row_count(&probe, graph, "SELECT id FROM audit WHERE id = 7").await,
            0,
            "the table side must NOT be applied yet — this is the torn state"
        );

        // A brand-new connection for the SAME owner: its first statement
        // lazily sweeps and resolves the leftover intent.
        let fresh = new_session(state.clone(), graph).await;
        fresh
            .execute("SELECT id FROM nodes WHERE id = 'unrelated-probe'")
            .await
            .expect("a trivial read triggers recovery as a side effect");

        assert!(
            medulla_server_api::api::query_txn::txn_intent::list_intents(&authority, persist_dir)
                .is_empty(),
            "recovery must clear the resolved intent"
        );
        assert_eq!(
            node_count(&fresh, graph, "crash-node").await,
            1,
            "the graph write must remain applied EXACTLY once (idempotent replay, no duplication)"
        );
        assert_eq!(
            table_row_count(&fresh, graph, "SELECT id FROM audit WHERE id = 7").await,
            1,
            "recovery must complete the table side — no torn state survives"
        );
    }

    /// A recovery graph error is not proof that the graph side stayed clean:
    /// the failure may have followed a durable graph commit. Keep the intent
    /// and clear the once-per-connection latch so the same authenticated
    /// connection retries after the transient graph fault is removed.
    #[tokio::test]
    async fn recovery_graph_error_retains_intent_for_same_connection_retry() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let state = test_state();
        let graph = "ne004-recovery-graph-error";
        let table = format!("ne004_recovery_table_{}", uuid::Uuid::new_v4().simple());
        let session = new_session(state.clone(), graph).await;
        let authority = session.carrier_authority().expect("bound authority");
        let persist_dir_buf = state
            .read()
            .await
            .persist_dir
            .clone()
            .expect("persist dir configured");
        let persist_dir = std::path::Path::new(&persist_dir_buf);
        let operation_id = uuid::Uuid::new_v4();
        let intent = medulla_server_api::api::query_txn::txn_intent::CommitIntent::new(
            graph.to_string(),
            operation_id,
            vec![Method::AddNode {
                node_id: "recovery-after-graph-fault".to_string(),
                properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                    "phase": "recovery"
                }))
                .expect("encode recovery node properties"),
            }],
            vec![Method::RemoveNode {
                node_id: "recovery-after-graph-fault".to_string(),
            }],
            vec![medulla_server_api::api::query_txn::txn_intent::ReplayStep::Sql(
                format!("CREATE TABLE {table} (id INT)"),
            )],
            medulla_server_api::txn::now_ms(),
        );
        medulla_server_api::api::query_txn::txn_intent::write_intent(&authority, persist_dir, &intent)
            .expect("persist phase-1 recovery intent");

        // The first recovery attempt fails at graph lookup because the graph
        // is temporarily absent. The durable intent must survive this
        // uncertain phase-1 outcome.
        let first = session
            .execute("SELECT id FROM nodes")
            .await
            .expect_err("missing graph must fail the first recovery attempt");
        assert!(first.message.contains("graph"), "{first}");
        assert_eq!(
            medulla_server_api::api::query_txn::txn_intent::list_intents(&authority, persist_dir).len(),
            1,
            "an uncertain recovery graph error must retain its intent"
        );

        // Remove the injected fault by making the graph available. The same
        // session's next statement must retry because recovery reset its latch.
        create_test_graph(&state, graph, 99).await;
        session
            .execute("SELECT id FROM nodes")
            .await
            .expect("the same connection retries retained recovery");
        assert!(
            medulla_server_api::api::query_txn::txn_intent::list_intents(&authority, persist_dir)
                .is_empty(),
            "successful retry must retire the recovered intent"
        );
        assert_eq!(
            node_count(&session, graph, "recovery-after-graph-fault").await,
            1,
            "the retried graph phase must apply exactly once"
        );
        match session
            .execute(&format!("SELECT id FROM {table}"))
            .await
            .expect("the recovery retry must create its table")
        {
            WireOutcome::Rows(result) => assert!(result.rows.is_empty()),
            other => panic!("expected an empty recovered table, got {other:?}"),
        }
    }

    /// `ROLLBACK` still drops everything, including a mixed graph+table
    /// transaction's NE-004 bookkeeping (the replay log / serializable
    /// label set) — no commit-intent is ever written for a rolled-back txn.
    #[tokio::test]
    async fn rollback_drops_mixed_transaction_entirely() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let state = test_state();
        let graph = "ne004-rollback";
        create_test_graph(&state, graph, 4).await;
        let session = new_session(state.clone(), graph).await;
        session
            .execute("CREATE TABLE ledger (id INT)")
            .await
            .expect("create table");

        session.execute("BEGIN").await.expect("begin");
        session
            .execute("INSERT INTO nodes (id) VALUES ('rolled-back-node')")
            .await
            .expect("buffer graph insert");
        session
            .execute("INSERT INTO ledger (id) VALUES (1)")
            .await
            .expect("buffer table insert");
        let outcome = session.execute("ROLLBACK").await.expect("rollback");
        assert!(matches!(outcome, WireOutcome::TxnEnd { tag: "ROLLBACK" }));

        assert_eq!(node_count(&session, graph, "rolled-back-node").await, 0);
        assert_eq!(
            table_row_count(&session, graph, "SELECT id FROM ledger WHERE id = 1").await,
            0
        );
    }

    /// The `25P02` aborted-transaction-block behaviour is unchanged: a
    /// statement error inside an open txn latches it aborted; every
    /// subsequent statement except COMMIT/ROLLBACK is rejected `25P02` until
    /// the block ends, and COMMIT while aborted behaves as ROLLBACK.
    #[tokio::test]
    async fn aborted_transaction_block_still_rejects_with_25p02() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let state = test_state();
        let graph = "ne004-aborted";
        create_test_graph(&state, graph, 5).await;
        let session = new_session(state.clone(), graph).await;

        session.execute("BEGIN").await.expect("begin");
        let bad = session
            .execute("SELECT * FROM this_table_does_not_exist")
            .await;
        assert!(bad.is_err(), "a bad statement must fail");

        let next = session.execute("SELECT 1").await;
        let err = next.expect_err("a statement after an error must be rejected");
        assert_eq!(err.code, "25P02");

        let commit_outcome = session.execute("COMMIT").await.expect("commit-as-rollback");
        assert!(matches!(
            commit_outcome,
            WireOutcome::TxnEnd { tag: "ROLLBACK" }
        ));
    }

    /// NE-005: an unsupported isolation-level spelling is REJECTED with a
    /// typed error (SQLSTATE `0A000`) — never silently accepted.
    #[tokio::test]
    async fn unsupported_isolation_level_is_rejected_not_downgraded() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let state = test_state();
        let graph = "ne005-reject";
        create_test_graph(&state, graph, 6).await;
        let session = new_session(state.clone(), graph).await;

        let err = session
            .execute("SET TRANSACTION ISOLATION LEVEL BOGUS")
            .await
            .expect_err("an unsupported level must be rejected");
        assert_eq!(err.code, "0A000");
    }

    /// NE-005: `REPEATABLE READ` / `READ COMMITTED` / `READ UNCOMMITTED` are
    /// accepted (mapped upward onto `Snapshot`, never rejected — this engine
    /// always provides AT LEAST that much).
    #[tokio::test]
    async fn weaker_standard_isolation_levels_are_accepted() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let state = test_state();
        let graph = "ne005-weaker";
        create_test_graph(&state, graph, 7).await;
        let session = new_session(state.clone(), graph).await;

        for level in ["REPEATABLE READ", "READ COMMITTED", "READ UNCOMMITTED"] {
            session
                .execute(&format!("BEGIN ISOLATION LEVEL {level}"))
                .await
                .unwrap_or_else(|e| panic!("{level} must be accepted, got {e:?}"));
            assert_eq!(
                *session.txn_isolation.lock(),
                medulla_server_api::txn::IsolationLevel::Snapshot
            );
            session.execute("ROLLBACK").await.expect("rollback");
        }
    }

    /// NE-005's central rule: a requested `SERIALIZABLE` either genuinely
    /// serializes or is rejected — never silently downgraded to `Snapshot`.
    /// Proved with a concurrent read-modify-write write-skew scenario a
    /// per-node OCC check (which `Snapshot` already has) does NOT catch: two
    /// transactions each scan `label = 'pool'` nodes and each insert one
    /// more IF the scanned count is below a cap — neither writes to a node
    /// the other reads, so only the AUTO predicate-label tracking
    /// (`extract_label_predicates` + `GraphTxnState::add_predicate_read`)
    /// can catch the phantom. Under `SERIALIZABLE`, the second committer
    /// MUST be rejected `40001`; the SAME scenario under the engine default
    /// (`Snapshot`) is proved to let both through, showing this is a REAL
    /// difference in behaviour, not a coincidental failure.
    #[tokio::test]
    async fn serializable_genuinely_catches_a_write_skew_snapshot_misses() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let state = test_state();
        let graph = "ne005-serializable";
        create_test_graph(&state, graph, 8).await;
        let seed = new_session(state.clone(), graph).await;
        for i in 0..2 {
            seed.execute(&format!(
                "INSERT INTO nodes (id, type) VALUES ('pool-{i}', 'pool')"
            ))
            .await
            .expect("seed pool nodes");
        }

        // ---- SERIALIZABLE: the second committer must be rejected. ----
        let a = new_session(state.clone(), graph).await;
        let b = new_session(state.clone(), graph).await;
        a.execute("BEGIN ISOLATION LEVEL SERIALIZABLE")
            .await
            .expect("begin a");
        b.execute("BEGIN ISOLATION LEVEL SERIALIZABLE")
            .await
            .expect("begin b");
        assert_eq!(
            read_row_count(&a, "SELECT id FROM nodes WHERE type = 'pool'").await,
            2
        );
        assert_eq!(
            read_row_count(&b, "SELECT id FROM nodes WHERE type = 'pool'").await,
            2
        );
        a.execute("INSERT INTO nodes (id, type) VALUES ('pool-from-a', 'pool')")
            .await
            .expect("buffer a's insert");
        b.execute("INSERT INTO nodes (id, type) VALUES ('pool-from-b', 'pool')")
            .await
            .expect("buffer b's insert");
        a.execute("COMMIT").await.expect("a commits first");
        let b_commit = b.execute("COMMIT").await;
        assert!(
            b_commit.is_err(),
            "SERIALIZABLE must reject the phantom-inducing second commit"
        );
        assert_eq!(b_commit.unwrap_err().code, "40001");

        // ---- Snapshot (the default): the SAME scenario is let through,
        // proving SERIALIZABLE is not a no-op relabeling. ----
        let c = new_session(state.clone(), graph).await;
        let d = new_session(state.clone(), graph).await;
        c.execute("BEGIN").await.expect("begin c");
        d.execute("BEGIN").await.expect("begin d");
        assert_eq!(
            read_row_count(&c, "SELECT id FROM nodes WHERE type = 'pool'").await,
            3
        );
        assert_eq!(
            read_row_count(&d, "SELECT id FROM nodes WHERE type = 'pool'").await,
            3
        );
        c.execute("INSERT INTO nodes (id, type) VALUES ('pool-from-c', 'pool')")
            .await
            .expect("buffer c's insert");
        d.execute("INSERT INTO nodes (id, type) VALUES ('pool-from-d', 'pool')")
            .await
            .expect("buffer d's insert");
        c.execute("COMMIT").await.expect("c commits");
        d.execute("COMMIT")
            .await
            .expect("Snapshot does NOT catch this write skew — both commit");
    }

    /// NE-071 (MEDULLA-OCC-SWEEP) regression guard: a wire multi-statement
    /// `BEGIN ISOLATION LEVEL SERIALIZABLE … COMMIT` transaction whose
    /// captured predicate read-set is invalidated by a concurrent phantom
    /// insert landing WHILE the transaction is open MUST reject the commit.
    ///
    /// This exercises EXACTLY the `run_commit` `has_node_ops` branch (a
    /// plain `INSERT INTO nodes …`, not a cross-modal statement) that calls
    /// `WireSession::require_txn_begin_version` → `commit_graph_methods_with_op`
    /// — the residue this track (NE-071) closed: `commit_graph_methods_with_op`
    /// used to accept `begin_version: Option<u64>` and silently fall back
    /// (`.unwrap_or_else(|| core.version())`) to the commit-time version on
    /// `None`, defeating `GraphTxnState::validate`'s coarse guard exactly
    /// like the original P0. `TxnBeginVersion` removed the silent default;
    /// this test proves the removal actually matters, not just that it
    /// compiles.
    ///
    /// PROVEN to catch a reintroduced bug (NE-071 definition-of-done):
    /// temporarily changing the `has_node_ops` branch's
    /// `let begin_version = self.require_txn_begin_version()?;` back to
    /// `let begin_version = TxnBeginVersion::Autocommit;` (the commit-time
    /// re-derivation shape) makes THIS test fail — `a`'s `COMMIT` starts
    /// succeeding instead of conflicting, because the coarse guard then
    /// always short-circuits before the predicate re-check runs. Observed
    /// failing under that change, then reverted; see the track report for
    /// the exact commands run.
    #[tokio::test]
    async fn ne071_wire_commit_rejects_concurrent_phantom_via_begin_version() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let state = test_state();
        let graph = "ne071-begin-version";
        create_test_graph(&state, graph, 20).await;
        let seed = new_session(state.clone(), graph).await;
        seed.execute("INSERT INTO nodes (id, type) VALUES ('s0', 'Sensor')")
            .await
            .expect("seed one Sensor node");

        // A: SERIALIZABLE, captures {s0} as its label=Sensor predicate
        // read-set at BEGIN (real begin-time state, via
        // `WireSession::txn_begin_version` captured in the `BEGIN` handler).
        let a = new_session(state.clone(), graph).await;
        a.execute("BEGIN ISOLATION LEVEL SERIALIZABLE")
            .await
            .expect("begin a");
        assert_eq!(
            read_row_count(&a, "SELECT id FROM nodes WHERE type = 'Sensor'").await,
            1,
            "A's predicate read-set must see exactly the seeded Sensor at begin"
        );
        // A durable write of A's own, so its COMMIT actually reaches
        // `commit_graph_methods_with_op` (a read-only txn's `has_node_ops`
        // is false and never calls it at all).
        a.execute("INSERT INTO nodes (id, type) VALUES ('a_node', 'Widget')")
            .await
            .expect("buffer a's own node insert");

        // B: an UNRELATED, off-txn autocommit session inserts a PHANTOM
        // Sensor WHILE A's transaction is still open — bumping
        // `core.version()` past A's captured begin-time value.
        let b = new_session(state.clone(), graph).await;
        b.execute("INSERT INTO nodes (id, type) VALUES ('s_phantom', 'Sensor')")
            .await
            .expect("B's phantom Sensor insert must land immediately (autocommit)");

        // A commits AFTER B's phantom landed: serializable validation MUST
        // re-evaluate the label=Sensor predicate, see the phantom, and
        // reject — this is the assertion that fails if `begin_version`
        // regresses to a commit-time re-derivation.
        let commit = a.execute("COMMIT").await;
        assert!(
            commit.is_err(),
            "A's SERIALIZABLE commit must CONFLICT on B's phantom Sensor, but it \
             succeeded — begin_version silently defeated OCC validation"
        );
        assert_eq!(commit.unwrap_err().code, "40001");

        // A's own buffered node write never landed (true rollback).
        assert_eq!(
            read_row_count(&b, "SELECT id FROM nodes WHERE type = 'Widget'").await,
            0,
            "A's conflicted transaction must not have persisted its buffered write"
        );
    }
}


#[cfg(all(any(test, feature = "test-support"), feature = "query"))]
pub mod wired_catalog_tests {
    //! NE-046 (MEDULLA-WIRE-CATALOG) wiring tests. `sql_catalog_acl.rs`'s own 13
    //! tests already prove the ACL primitives (ownership/grants/RLS)
    //! are correct in isolation; these drive the REAL wire `execute()`
    //! dispatch instead — the buffered `TxnOp`/`TableTxn` commit path
    //! (`commit_table_txn_with_op`, reached by CREATE/INSERT/UPDATE/DELETE/
    //! ALTER/DROP TABLE and COPY) and the free-form multi-table SELECT read
    //! path (`run_read` / `authorized_read_store`) — to prove the ACL is
    //! actually ENFORCED at those sites now, not merely correct on its own.
    use super::*;
    use medulla_protocol_types::protocol::GraphType;
    use medulla_protocol_types::protocol::Method;
    use medulla_server_api::auth::VerifiedRequestContext;
    use medulla_server_api::sql_catalog_acl;
    use medulla_server_api::sql_catalog_acl::SqlPrivilege;

    const SECRET: &str = "medulla-wired-catalog-test-secret";
    pub(crate) const CREATOR: &str = "wired-catalog-graph-creator";

    fn ensure_env() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            std::env::set_var("MEDULLA_AUDIENCE", "medulla-test");
            std::env::set_var("MEDULLA_TENANT", "tenant-shared");
            std::env::set_var("MEDULLA_POLICY_VERSION", "policy-test");
            // MEDULLA_ENCRYPTION_KEY is deliberately NOT set here. It used to
            // be, through this `Once` and holding NOTHING, and to a value different
            // from the six other modules' -- so this `Once` could change the ambient
            // at-rest key in the middle of ANOTHER test's body no matter what guard
            // that test held, and did: it is what failed
            // `redb_backend::tests::online_reshard_moves_graph_live_no_loss` with
            // "decryption failed (wrong key or tampered ciphertext)" under
            // parallelism while it passed standalone. Every test below now takes
            // `medulla_kernel::crypto::provisioned_test_env_read_lock()`, which provisions the
            // ONE shared key under the WRITE guard and then holds the READ guard for
            // the test's whole body. See `medulla_kernel::crypto::TEST_AT_REST_KEY`'s doc.
        });
    }

    /// A real durable `ServerState` on its own uniquely-named temp dir
    /// (mirrors `ne_004_ne_005_tests::test_state`, trimmed to nothing this
    /// module's tests need). Every id in `agents` is registered
    /// `AgentRole::System` so the ENGINE ACL (`check_access`, graph-level)
    /// never interferes with what this module tests — SQL catalog
    /// ownership/grants/RLS is a completely separate, additional layer this
    /// track wires on top, not a replacement for the engine ACL.
    pub(crate) fn test_state(agents: &[&str]) -> Arc<RwLock<ServerState>> {
        ensure_env();
        medulla_server::test_state_with_services(
            SECRET,
            test_isolation(agents),
            "medulla-wired-catalog-test",
            "medulla-wired-catalog-tsdb-test",
        )
    }

    pub(crate) fn request(id: u64, graph: &str, method: Method) -> Request {
        static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let claims = medulla_server_api::authority_context::signed_test_claims(CREATOR);
        let mut req = Request {
            id,
            graph: graph.to_string(),
            auth_token: String::new(),
            agent_id: Some(CREATOR.to_string()),
            method,
        };
        let nonce = format!(
            "wired-catalog-{}-{}",
            std::process::id(),
            NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        req.auth_token = medulla_server_api::auth::compute_verified_envelope_token(
            SECRET,
            &req,
            &medulla_server_api::auth::VerifiedEnvelopeParams {
                context: &claims,
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("system clock")
                    .as_secs(),
                nonce: &nonce,
                idempotency_key: &format!("wired-catalog-request-{id}"),
            },
        );
        req
    }

    /// Register a graph with the engine (`Method::CreateGraph`, via `CREATOR`
    /// — a fixed bootstrap identity distinct from every SQL-catalog actor
    /// this module tests, so graph registration never entangles with the
    /// catalog ACL being tested). `check_access` lets an UNregistered graph
    /// through (see its own doc), so a pure catalog DML/DDL statement never
    /// needs this — only a `SELECT` does (`run_read` requires a real
    /// `GraphCore`).
    pub(crate) async fn create_test_graph(state: &Arc<RwLock<ServerState>>, graph: &str, id: u64) {
        let resp = medulla_server::dispatch(
            state,
            request(
                id,
                graph,
                Method::CreateGraph {
                    graph_name: graph.to_string(),
                    graph_type: GraphType::Agent,
                },
            ),
        )
        .await;
        assert!(resp.error.is_none(), "CreateGraph failed: {:?}", resp.error);
    }

    /// A fresh `WireSession` — a new "connection" sharing `state` — bound to
    /// an EXPLICIT `(tenant, agent_id)` authority via the SAME `bind_authority`
    /// seam `bind_authenticated_sql_actor` uses, generalized so cross-tenant /
    /// cross-actor scenarios can each get their own session against ONE
    /// shared `state` (test-only: `VerifiedRequestContext::verified_for_test_in_tenant`
    /// is `#[cfg(test)]`).
    pub(crate) fn session_for(
        state: Arc<RwLock<ServerState>>,
        graph: &str,
        agent_id: &str,
        tenant: &str,
    ) -> WireSession {
        let session = WireSession::new(state, graph.to_string());
        let authority = CarrierAuthority::from_verified(
            &VerifiedRequestContext::verified_for_test_in_tenant(agent_id, tenant),
        )
        .expect("build test authority");
        session
            .bind_authority(authority, agent_id)
            .expect("bind test identity");
        session
    }

    /// The SAME `CarrierAuthority` a `session_for` with these exact
    /// arguments carries — for tests that also need to call `sql_catalog_acl`
    /// directly (e.g. `grant`/`set_row_level_column`, which have no wire SQL
    /// surface in this track's scope).
    pub(crate) fn authority(agent_id: &str, tenant: &str) -> CarrierAuthority {
        CarrierAuthority::from_verified(&VerifiedRequestContext::verified_for_test_in_tenant(
            agent_id, tenant,
        ))
        .expect("build test authority")
    }

    fn authorize_test_table_txn(
        authority: &CarrierAuthority,
        persist_dir: &Path,
        store: &TableStore,
        txn: &mut TableTxn,
        committed_replay: bool,
    ) -> WireResult<Vec<TableSchema>> {
        medulla_server_api::sql_catalog_acl::with_source_authority_write(
            persist_dir,
            authority,
            |source| {
                authorize_table_txn(source, store, txn, committed_replay)
                    .map_err(|error| error.message)
            },
        )
        .map_err(user_err)
    }

    /// The SAME persistence directory `test_state` configured — for tests
    /// that call `sql_catalog_acl` directly (bypassing the wire) and need the
    /// exact `Path` every `sql_catalog_acl`/`sql_tables` entry point takes.
    pub(crate) async fn test_persist_dir_of(
        state: &Arc<RwLock<ServerState>>,
    ) -> std::path::PathBuf {
        std::path::PathBuf::from(
            state
                .read()
                .await
                .persist_dir
                .clone()
                .expect("test_state always configures persist_dir"),
        )
    }

    pub(crate) async fn read_rows(
        session: &WireSession,
        sql: &str,
    ) -> WireResult<TypedQueryResult> {
        match session.execute(sql).await? {
            WireOutcome::Rows(result) => Ok(result),
            other => panic!("expected a row set, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn create_batch_capability_rejects_reorder_collision_drop_and_rename() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let tenant = "wired-catalog-provisional-negatives";
        let state = test_state(&[CREATOR, "alice-provisional", "bob-provisional"]);
        let graph = "wired-catalog-provisional-negatives-graph";
        create_test_graph(&state, graph, 40).await;
        let persist_dir = test_persist_dir_of(&state).await;
        let alice = authority("alice-provisional", tenant);
        let bob = authority("bob-provisional", tenant);
        let store = medulla_server_api::sql_tables::tenant_table_store(tenant, &persist_dir)
            .expect("open tenant table store");

        let schema = TableSchema::new(
            "provisional_target",
            vec![Column::new("id", ColumnType::Text, false, true)],
        );
        let insert = TxnOp::Insert {
            table: schema.name.clone(),
            col_order: vec!["id".to_string()],
            rows: vec![vec![serde_json::Value::String("one".to_string())]],
        };

        let mut reordered = TableTxn::new();
        reordered.push(insert.clone());
        reordered.push(TxnOp::CreateTable {
            schema: schema.clone(),
            if_not_exists: false,
        });
        assert_eq!(
            authorize_test_table_txn(&alice, &persist_dir, &store, &mut reordered, false)
                .expect_err("DML before CREATE must not receive later authority")
                .message,
            sql_catalog_acl::ACCESS_DENIED
        );

        let mut dropped = TableTxn::new();
        dropped.push(TxnOp::CreateTable {
            schema: schema.clone(),
            if_not_exists: false,
        });
        dropped.push(TxnOp::DropTable {
            name: schema.name.clone(),
            if_exists: false,
        });
        assert!(
            authorize_test_table_txn(&alice, &persist_dir, &store, &mut dropped, false)
                .expect("CREATE then DROP is authorized without residual ownership")
                .is_empty(),
            "a dropped provisional table must not be registered after commit"
        );

        let mut renamed = TableTxn::new();
        renamed.push(TxnOp::CreateTable {
            schema: schema.clone(),
            if_not_exists: false,
        });
        renamed.push(TxnOp::RenameTable {
            table: schema.name.clone(),
            new_name: "renamed_target".to_string(),
        });
        assert_eq!(
            authorize_test_table_txn(&alice, &persist_dir, &store, &mut renamed, false)
                .expect_err("a provisional capability cannot be retargeted")
                .message,
            sql_catalog_acl::ACCESS_DENIED
        );

        store
            .create_table(&schema, false)
            .expect("seed the physical collision");
        sql_catalog_acl::with_source_authority_write(&persist_dir, &alice, |source| {
            sql_catalog_acl::register_owner_after_create_in(
                source,
                &schema.name,
                uuid::Uuid::new_v4(),
            )
        })
        .expect("seed the retained owner");
        let mut collision = TableTxn::new();
        collision.push(TxnOp::CreateTable {
            schema: schema.clone(),
            if_not_exists: false,
        });
        collision.push(insert);
        assert_eq!(
            authorize_test_table_txn(&bob, &persist_dir, &store, &mut collision, false)
                .expect_err("another actor cannot inherit collision authority")
                .message,
            sql_catalog_acl::ACCESS_DENIED
        );
    }

    #[tokio::test]
    async fn ordinary_create_retains_intent_until_owner_repair_completes() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let tenant = "wired-catalog-ordinary-owner-repair";
        let state = test_state(&[CREATOR, "ordinary-owner"]);
        let graph = "wired-catalog-ordinary-owner-repair-graph";
        create_test_graph(&state, graph, 43).await;
        let persist_dir = test_persist_dir_of(&state).await;
        let owner = authority("ordinary-owner", tenant);

        // A malformed owner table lets the physical CREATE commit but makes
        // owner registration fail. This is a deterministic storage fault at
        // the exact cross-redb boundary ordinary autocommit must recover.
        let acl =
            medulla_server_api::sql_tables::tenant_acl_table_store(owner.tenant_scope(), &persist_dir)
                .expect("open tenant ACL store");
        acl.create_table(
            &TableSchema::new(
                "__eg_sql_owners__",
                vec![Column::new("table_name", ColumnType::Text, false, true)],
            ),
            false,
        )
        .expect("install malformed owner table fault");

        let session = session_for(state.clone(), graph, "ordinary-owner", tenant);
        let error = session
            .execute("CREATE TABLE ordinary_repair (id TEXT PRIMARY KEY)")
            .await
            .expect_err("owner registration fault must surface");
        assert!(error.message.starts_with(SQL_OWNER_REPAIR_PENDING));
        let store =
            medulla_server_api::sql_tables::tenant_table_store(owner.tenant_scope(), &persist_dir)
                .expect("open tenant table store");
        assert!(store
            .get_schema("ordinary_repair")
            .expect("read physical create")
            .is_some());
        assert_eq!(
            medulla_server_api::api::query_txn::txn_intent::list_intents(&owner, &persist_dir).len(),
            1,
            "ordinary CREATE keeps its durable recipe while owner repair is pending"
        );

        acl.drop_table("__eg_sql_owners__", false)
            .expect("remove injected malformed catalog");
        let recovering = session_for(state, graph, "ordinary-owner", tenant);
        read_rows(&recovering, "SELECT id FROM ordinary_repair")
            .await
            .expect("the next authenticated connection repairs before serving");
        assert!(
            medulla_server_api::api::query_txn::txn_intent::list_intents(&owner, &persist_dir)
                .is_empty()
        );
        sql_catalog_acl::grant(
            &persist_dir,
            &owner,
            "ordinary_repair",
            "grantee",
            &[SqlPrivilege::Select],
            uuid::Uuid::new_v4(),
        )
        .expect("recovery durably restored the ordinary creator's ownership");
    }

    #[test]
    fn local_source_authority_is_checked_before_any_intent_state_effect() {
        let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/crates/medulla-wire/src/wire/mod.rs"));
        for (function, effect) in [
            ("async fn commit_mixed_txn", "write_intent"),
            ("async fn commit_table_txn_dispatch", "write_intent"),
            ("async fn recover_owner_intents_once", ".swap(true"),
            ("async fn recover_owner_intents_once", "list_intents"),
        ] {
            let body = &source[source.find(function).expect("function must exist")..];
            let authority = body
                .find("require_source_authority")
                .expect("source-authority check must exist");
            let effect = body.find(effect).expect("intent effect must exist");
            assert!(
                authority < effect,
                "{function} must fail closed before {effect}"
            );
        }
    }

    #[test]
    fn durable_table_replay_cannot_enter_dispatch_or_create_an_intent() {
        let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/crates/medulla-wire/src/wire/mod.rs"));
        let replay_start = source
            .find("async fn replay_table_steps(")
            .expect("replay function must exist");
        let replay_end = source[replay_start..]
            .find("async fn recover_owner_intents_once")
            .map(|offset| replay_start + offset)
            .expect("recovery function must follow replay");
        let replay = &source[replay_start..replay_end];
        assert!(replay.contains("try_buffer_transaction_table_statement"));
        for forbidden in ["dispatch_kind", "commit_table_txn", "write_intent"] {
            assert!(
                !replay.contains(forbidden),
                "durable replay must not call {forbidden}"
            );
        }

        let leaf_start = source
            .find("fn try_buffer_table_statement(")
            .expect("commit-free replay leaves must exist");
        let leaf_end = source[leaf_start..]
            .find("/// The classify")
            .map(|offset| leaf_start + offset)
            .expect("dispatch must follow replay leaves");
        let leaf = &source[leaf_start..leaf_end];
        assert!(leaf.contains("async fn try_buffer_transaction_table_statement("));
        assert!(leaf.contains("StatementKind::CreateTable"));
        assert!(leaf.contains("self.buffer("));
        for forbidden in ["commit_table_txn", "resolve_commit_intent", "write_intent"] {
            assert!(
                !leaf.contains(forbidden),
                "the replay leaves must not call {forbidden}"
            );
        }
    }

    #[tokio::test]
    async fn committed_create_replay_repairs_owner_only_for_exact_intent() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let tenant = "wired-catalog-owner-replay";
        let state = test_state(&[CREATOR, "owner-replay", "foreign-replay"]);
        let graph = "wired-catalog-owner-replay-graph";
        create_test_graph(&state, graph, 41).await;
        let persist_dir = test_persist_dir_of(&state).await;
        let owner = authority("owner-replay", tenant);
        let operation_id = uuid::Uuid::new_v4();
        let table = "owner_replay_target";
        let steps = vec![
            medulla_server_api::api::query_txn::txn_intent::ReplayStep::Sql(format!(
                "CREATE TABLE {table} (id TEXT PRIMARY KEY)"
            )),
            medulla_server_api::api::query_txn::txn_intent::ReplayStep::Sql(format!(
                "INSERT INTO {table} (id) VALUES ('one')"
            )),
        ];
        let intent = medulla_server_api::api::query_txn::txn_intent::CommitIntent::new(
            graph.to_string(),
            operation_id,
            Vec::new(),
            Vec::new(),
            steps,
            medulla_server_api::txn::now_ms(),
        );
        let descriptor = intent
            .table_operation_descriptor()
            .expect("hash the exact replay recipe");
        assert!(descriptor.starts_with("transaction:sha256:"));
        assert!(
            !descriptor.contains(table),
            "the receipt must not retain SQL text"
        );

        let session = session_for(state.clone(), graph, "owner-replay", tenant);
        *session.txn.lock() = Some(TableTxn::new());
        session
            .replay_table_steps(graph, &intent.table_steps)
            .await
            .expect("rebuild the original table transaction");
        let table_txn = session.txn.lock().take().expect("rebuilt table txn");

        let request_id = u64::from_be_bytes(
            operation_id.as_bytes()[..8]
                .try_into()
                .expect("UUID prefix is eight bytes"),
        );
        let batch_id = medulla_server_api::mutation_batch::opaque_coordinator_key(
            "wire-sql-owner",
            owner.owner_scope(),
            &operation_id.simple().to_string(),
        );
        let operation = medulla_protocol_types::protocol::Method::Sql {
            query: descriptor,
            params_msgpack: Vec::new(),
        };
        let store =
            medulla_server_api::sql_tables::tenant_table_store(owner.tenant_scope(), &persist_dir)
                .expect("open tenant table store");
        // Fabricate the recoverable crash shape directly: the durable table
        // batch landed while owner registration did not. Current production
        // retains this exact intent across that gap, and the exclusive
        // capability prevents any competing source decision during each attempt.
        let created_at_ms = medulla_server_api::txn::now_ms();
        let expected_version = store
            .mutation_version(owner.tenant_scope(), graph)
            .expect("read SQL mutation version");
        let batch = medulla_server_api::mutation_batch::compile_opaque_method(
            medulla_server_api::mutation_batch::CompileBatch {
                batch_id: &batch_id,
                request_id,
                attempt_nonce: None,
                principal: Some(owner.actor_scope()),
                tenant: owner.tenant_scope(),
                graph,
                placement_epoch: 0,
                idempotency_key: &batch_id,
                expected_graph_version: Some(expected_version),
                fencing_token: None,
                created_at_ms,
                default_surface: medulla_mutation_types::mutation_scope::MutationSurface::Query,
                authoritative_state: None,
            },
            &operation,
            medulla_mutation_types::mutation_scope::MutationSurface::Query,
            medulla_mutation_types::mutation_scope::DurabilityDomain::SqlCatalog,
            "sql_catalog_operation",
        )
        .expect("compile the exact durable SQL receipt");
        store
            .commit_txn_batch(&table_txn, &batch, created_at_ms)
            .expect("simulate physical commit before owner registration");

        assert_eq!(
            sql_catalog_acl::grant(
                &persist_dir,
                &owner,
                table,
                "foreign-replay",
                &[SqlPrivilege::Select],
                uuid::Uuid::new_v4(),
            )
            .expect_err("the injected crash left no owner yet"),
            sql_catalog_acl::ACCESS_DENIED
        );

        assert_eq!(
            session
                .replay_table_steps_and_commit(&intent)
                .await
                .expect("the exact committed replay repairs ownership"),
            1
        );
        sql_catalog_acl::grant(
            &persist_dir,
            &owner,
            table,
            "foreign-replay",
            &[SqlPrivilege::Select],
            uuid::Uuid::new_v4(),
        )
        .expect("owner repair is durable before recovery closes");

        let altered = medulla_server_api::api::query_txn::txn_intent::CommitIntent::new(
            graph.to_string(),
            operation_id,
            Vec::new(),
            Vec::new(),
            vec![medulla_server_api::api::query_txn::txn_intent::ReplayStep::Sql(
                "CREATE TABLE altered_replay_target (id TEXT PRIMARY KEY)".to_string(),
            )],
            medulla_server_api::txn::now_ms(),
        );
        let altered_error = session
            .replay_table_steps_and_commit(&altered)
            .await
            .expect_err("the same operation id cannot authorize an altered recipe");
        assert_eq!(altered_error.message, SQL_REPLAY_OPERATION_CONFLICT);
        assert!(store
            .get_schema("altered_replay_target")
            .expect("read altered-table absence")
            .is_none());

        let foreign = session_for(state, graph, "foreign-replay", tenant);
        let foreign_error = foreign
            .replay_table_steps_and_commit(&intent)
            .await
            .expect_err("another actor cannot replay the owner's receipt");
        assert_eq!(foreign_error.message, sql_catalog_acl::ACCESS_DENIED);

        session
            .replay_table_steps_and_commit(&intent)
            .await
            .expect("repeated restart replay is idempotent");
        let rows = read_rows(&session, &format!("SELECT id FROM {table}"))
            .await
            .expect("read the recovered table");
        assert_eq!(rows.rows.len(), 1, "replay must not duplicate DML");
    }

    /// RF-RULING-006 made the SQL catalog a kernel-owned owner store, so
    /// `mutation_batch::compile::finish_batch` stamps the FILE's bound
    /// serving principal on `context.principal` -- uniformly, for every domain --
    /// and the verified caller travels
    /// as the outbox row's `actor` header. This pins both halves of that on a
    /// real committed receipt: the recorded `context.principal` is the engine's,
    /// NOT the caller's — so the pre-ruling comparison would refuse every replay
    /// of a statement this engine itself committed — and
    /// `committed_sql_replay_receipt` matches the owner while still refusing a
    /// different actor, because it reads the caller from the `actor` header.
    #[tokio::test]
    async fn sql_replay_receipt_matches_the_caller_on_the_outbox_actor_header() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let tenant = "wired-catalog-actor-header";
        let state = test_state(&[CREATOR, "actor-header-owner", "actor-header-foreign"]);
        let graph = "wired-catalog-actor-header-graph";
        create_test_graph(&state, graph, 61).await;
        let persist_dir = test_persist_dir_of(&state).await;
        let owner = authority("actor-header-owner", tenant);
        let stranger = authority("actor-header-foreign", tenant);
        // `CarrierAuthority::tenant_scope` is an opaque digest of the tenant,
        // not the raw name, and the served commit path
        // (`commit_table_txn_under_source`) compiles under exactly that value.
        // The receipt is looked up under the same one, so the fixture has to
        // use it too or it commits into a scope nothing reads.
        let tenant_scope = owner.tenant_scope().to_string();
        let store = medulla_server_api::sql_tables::tenant_table_store(&tenant_scope, &persist_dir)
            .expect("open tenant table store");

        let operation = medulla_protocol_types::protocol::Method::Sql {
            query: "CREATE TABLE actor_header_target (id TEXT PRIMARY KEY)".to_string(),
            params_msgpack: Vec::new(),
        };
        let batch_id = medulla_server_api::mutation_batch::opaque_coordinator_key(
            "wire-sql-owner",
            owner.owner_scope(),
            "actor-header-receipt",
        );
        let request_id = 0x5150_4143_4b45_5401_u64;
        let created_at_ms = medulla_server_api::txn::now_ms();
        let expected_version = store
            .mutation_version(&tenant_scope, graph)
            .expect("read SQL mutation version");
        let batch = medulla_server_api::mutation_batch::compile_opaque_method(
            medulla_server_api::mutation_batch::CompileBatch {
                batch_id: &batch_id,
                request_id,
                attempt_nonce: None,
                principal: Some(owner.actor_scope()),
                tenant: &tenant_scope,
                graph,
                placement_epoch: 0,
                idempotency_key: &batch_id,
                expected_graph_version: Some(expected_version),
                fencing_token: None,
                created_at_ms,
                default_surface: medulla_mutation_types::mutation_scope::MutationSurface::Query,
                authoritative_state: None,
            },
            &operation,
            medulla_mutation_types::mutation_scope::MutationSurface::Query,
            medulla_mutation_types::mutation_scope::DurabilityDomain::SqlCatalog,
            "sql_catalog_operation",
        )
        .expect("compile the SQL receipt");

        let owner_fingerprint =
            medulla_server_api::mutation_batch::principal_fingerprint(owner.actor_scope())
                .expect("fingerprint the owner");
        // The ledger principal is the SQL owner file's, not the caller's.
        assert_eq!(
            batch.serving_principal(),
            medulla_kernel::store_authority::ENGINE_PRINCIPAL,
            "a SqlCatalog batch must name the owner file's serving principal"
        );
        assert_ne!(batch.serving_principal(), owner_fingerprint);
        // ... and the caller IS inside the stable replay identity, which is what
        // makes a cross-actor replay a conflict rather than an anonymous match.
        assert_eq!(
            batch
                .envelope
                .operation()
                .expect("a served SQL batch is a caller operation")
                .authority
                .actor
                .as_str(),
            owner_fingerprint,
            "the verified caller must be inside the operation replay identity"
        );
        // ... and the caller is not lost: it is the outbox row's actor header.
        assert_eq!(
            batch.outbox[0].headers.get("actor"),
            Some(&owner_fingerprint),
            "the verified caller must travel as the outbox actor header"
        );

        let mut txn = TableTxn::new();
        txn.push(TxnOp::CreateTable {
            schema: TableSchema::new(
                "actor_header_target",
                vec![Column::new("id", ColumnType::Text, false, true)],
            ),
            if_not_exists: false,
        });
        store
            .commit_txn_batch(&txn, &batch, created_at_ms)
            .expect("commit the SQL batch through the kernel ledger");

        let matched = committed_sql_replay_receipt(&store, &owner, &batch)
            .expect("the owner's own receipt matches")
            .expect("the receipt is present");
        assert_eq!(matched.batch.batch_id, batch_id);

        // Planted known-bad input: a different actor, everything else identical.
        // The refusal must be reached by the ACTOR, which is inside the stable
        // operation identity, not by a timestamp or an OCC observation.
        let refused = committed_sql_replay_receipt(&store, &stranger, &batch)
            .expect_err("another actor may not claim this receipt");
        assert!(refused.contains("IDEMPOTENCY_CONFLICT"), "{refused}");
    }

    // ── owner / grant / deny-indistinguishable-from-absence ────────────────

    #[tokio::test]
    async fn wired_catalog_owner_can_use_own_table_through_wire_dml() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let tenant = "wired-catalog-tenant-owner";
        let state = test_state(&[CREATOR, "owner-1"]);
        let graph = "wired-catalog-owner-graph";
        create_test_graph(&state, graph, 1).await;

        let owner = session_for(state.clone(), graph, "owner-1", tenant);
        owner
            .execute("CREATE TABLE widgets (id TEXT PRIMARY KEY, name TEXT)")
            .await
            .expect("owner creates their own table");
        owner
            .execute("INSERT INTO widgets (id, name) VALUES ('1', 'a')")
            .await
            .expect("owner inserts into their own table");
        let rows = read_rows(&owner, "SELECT id, name FROM widgets")
            .await
            .expect("owner selects");
        assert_eq!(rows.rows.len(), 1, "owner sees the row they just inserted");
    }

    #[tokio::test]
    async fn wired_catalog_granted_same_tenant_actor_can_use_table() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let tenant = "wired-catalog-tenant-grant";
        let state = test_state(&[CREATOR, "owner-2", "grantee-2"]);
        let graph = "wired-catalog-grant-graph";
        create_test_graph(&state, graph, 1).await;
        let persist_dir = test_persist_dir_of(&state).await;

        let owner = session_for(state.clone(), graph, "owner-2", tenant);
        owner
            .execute("CREATE TABLE widgets2 (id TEXT PRIMARY KEY, name TEXT)")
            .await
            .expect("owner creates widgets2");
        owner
            .execute("INSERT INTO widgets2 (id, name) VALUES ('1', 'owner-row')")
            .await
            .expect("owner inserts");

        // `grant` requires the grantor to already be recognized as OWNER of
        // `widgets2` — succeeding here is itself proof that the wire CREATE
        // TABLE above (`authorize_table_txn` -> owner registration under the
        // same source-authority capability)
        // registered ownership correctly.
        let owner_authority = authority("owner-2", tenant);
        sql_catalog_acl::grant(
            &persist_dir,
            &owner_authority,
            "widgets2",
            "grantee-2",
            &[SqlPrivilege::Select, SqlPrivilege::Insert],
            uuid::Uuid::new_v4(),
        )
        .expect("owner grants Select+Insert to grantee-2 (proves ownership was registered)");

        let grantee = session_for(state.clone(), graph, "grantee-2", tenant);
        let rows = read_rows(&grantee, "SELECT id, name FROM widgets2")
            .await
            .expect("granted actor can select");
        assert_eq!(rows.rows.len(), 1, "granted actor sees the owner's row");
        grantee
            .execute("INSERT INTO widgets2 (id, name) VALUES ('2', 'grantee-row')")
            .await
            .expect("granted actor can insert");

        let rows = read_rows(&owner, "SELECT id, name FROM widgets2")
            .await
            .expect("owner selects again");
        assert_eq!(
            rows.rows.len(),
            2,
            "owner sees BOTH rows — the grant shares the SAME physical table"
        );
    }

    #[tokio::test]
    async fn wired_catalog_ungranted_actor_denied_indistinguishable_from_absence() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let tenant = "wired-catalog-tenant-deny";
        let state = test_state(&[CREATOR, "owner-3", "stranger-3"]);
        let graph = "wired-catalog-deny-graph";
        create_test_graph(&state, graph, 1).await;

        let owner = session_for(state.clone(), graph, "owner-3", tenant);
        owner
            .execute("CREATE TABLE widgets3 (id TEXT PRIMARY KEY, name TEXT)")
            .await
            .expect("owner creates widgets3");

        let stranger = session_for(state.clone(), graph, "stranger-3", tenant);

        // Write path: `authorize_insert` returns the SAME literal ACCESS_DENIED
        // string whether the table is real-but-forbidden or plain nonexistent —
        // assert EXACT string equality between the two, not just "both error".
        let denied_real = stranger
            .execute("INSERT INTO widgets3 (id, name) VALUES ('x', 'y')")
            .await
            .expect_err("ungranted actor must be denied on a REAL table");
        let denied_absent = stranger
            .execute("INSERT INTO widgets3_does_not_exist (id, name) VALUES ('x', 'y')")
            .await
            .expect_err("insert into a genuinely nonexistent table must also be denied");
        assert_eq!(
            denied_real.message, denied_absent.message,
            "the denial for a real-but-forbidden table must read IDENTICALLY to the \
             denial for a nonexistent one — no existence leak"
        );
        assert!(
            denied_real.message.contains(sql_catalog_acl::ACCESS_DENIED),
            "got: {}",
            denied_real.message
        );

        // Read path: `run_read`'s ephemeral authorized-only store makes an
        // inaccessible table genuinely ABSENT (not merely denied), so
        // DataFusion's own "not found" is the signal for BOTH cases — assert
        // neither error's wording distinguishes "forbidden" from "absent"
        // (no "denied"/"grant"/"owner" leaking through the read path either).
        let read_real = stranger
            .execute("SELECT * FROM widgets3")
            .await
            .expect_err("ungranted actor's read of a REAL table must fail");
        let read_absent = stranger
            .execute("SELECT * FROM widgets3_does_not_exist_either")
            .await
            .expect_err("read of a genuinely nonexistent table must also fail");
        for (label, err) in [("real", &read_real), ("absent", &read_absent)] {
            let lower = err.message.to_ascii_lowercase();
            assert!(
                !lower.contains("denied") && !lower.contains("grant") && !lower.contains("owner"),
                "{label}-table read error leaks privilege wording: {}",
                err.message
            );
        }
    }

    // ── cross-tenant physical isolation ─────────────────────────────────────

    #[tokio::test]
    async fn wired_catalog_second_tenant_reads_zero_rows() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let tenant_a = "wired-catalog-tenant-a";
        let tenant_b = "wired-catalog-tenant-b";
        let state = test_state(&[CREATOR, "actor-a", "actor-b"]);
        let graph = "wired-catalog-cross-tenant-graph";
        create_test_graph(&state, graph, 1).await;

        let a = session_for(state.clone(), graph, "actor-a", tenant_a);
        a.execute("CREATE TABLE widgets4 (id TEXT PRIMARY KEY, name TEXT)")
            .await
            .expect("tenant A creates widgets4");
        a.execute("INSERT INTO widgets4 (id, name) VALUES ('1', 'a-row')")
            .await
            .expect("tenant A inserts");
        a.execute("INSERT INTO widgets4 (id, name) VALUES ('2', 'a-row-2')")
            .await
            .expect("tenant A inserts a second row");

        // Tenant B's actor never touched `widgets4` — it does not exist in
        // tenant B's PHYSICAL catalog file at all (a separate redb file, not
        // an ACL-hidden row in a shared one).
        let b = session_for(state.clone(), graph, "actor-b", tenant_b);
        let denied = b
            .execute("SELECT * FROM widgets4")
            .await
            .expect_err("tenant B has no such table");
        assert!(!denied.message.to_ascii_lowercase().contains("denied"));

        // Tenant B creates its OWN same-named table with different data —
        // proves separation is PHYSICAL, not just an empty ACL view onto the
        // SAME file.
        b.execute("CREATE TABLE widgets4 (id TEXT PRIMARY KEY, name TEXT)")
            .await
            .expect("tenant B creates its own widgets4");
        b.execute("INSERT INTO widgets4 (id, name) VALUES ('99', 'b-row')")
            .await
            .expect("tenant B inserts into its own widgets4");
        let rows = read_rows(&b, "SELECT id, name FROM widgets4")
            .await
            .expect("tenant B selects its own widgets4");
        assert_eq!(
            rows.rows.len(),
            1,
            "tenant B sees ONLY its own row — zero of tenant A's rows leaked across \
             the tenant boundary"
        );
        assert_eq!(rows.rows[0][0], serde_json::Value::String("99".to_string()));
    }

    // ── row-level security constrains BOTH reads and writes ────────────────

    #[tokio::test]
    async fn wired_catalog_row_level_predicate_constrains_reads_and_writes() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let tenant = "wired-catalog-tenant-rls";
        let state = test_state(&[CREATOR, "rls-owner", "rls-grantee"]);
        let graph = "wired-catalog-rls-graph";
        create_test_graph(&state, graph, 1).await;
        let persist_dir = test_persist_dir_of(&state).await;

        let owner = session_for(state.clone(), graph, "rls-owner", tenant);
        owner
            .execute("CREATE TABLE notes (id TEXT PRIMARY KEY, owner_tag TEXT, body TEXT)")
            .await
            .expect("owner creates notes");

        let owner_authority = authority("rls-owner", tenant);
        sql_catalog_acl::set_row_level_column(
            &persist_dir,
            &owner_authority,
            "notes",
            Some("owner_tag"),
            uuid::Uuid::new_v4(),
        )
        .expect("declare owner_tag as the RLS discriminator");
        sql_catalog_acl::grant(
            &persist_dir,
            &owner_authority,
            "notes",
            "rls-grantee",
            &[
                SqlPrivilege::Select,
                SqlPrivilege::Insert,
                SqlPrivilege::Update,
                SqlPrivilege::Delete,
            ],
            uuid::Uuid::new_v4(),
        )
        .expect("grant the full DML set to rls-grantee");

        // Both actors insert WITHOUT naming `owner_tag` at all — RLS must
        // stamp it to each principal's OWN agent_id regardless.
        owner
            .execute("INSERT INTO notes (id, body) VALUES ('o1', 'owner note')")
            .await
            .expect("owner inserts (RLS stamps owner_tag)");
        let grantee = session_for(state.clone(), graph, "rls-grantee", tenant);
        grantee
            .execute("INSERT INTO notes (id, body) VALUES ('g1', 'grantee note')")
            .await
            .expect("grantee inserts (RLS stamps owner_tag)");

        // READS are constrained: each principal sees ONLY their own row.
        let owner_rows = read_rows(&owner, "SELECT id FROM notes")
            .await
            .expect("owner selects");
        assert_eq!(owner_rows.rows.len(), 1, "owner sees only their own row");
        let grantee_rows = read_rows(&grantee, "SELECT id FROM notes")
            .await
            .expect("grantee selects");
        assert_eq!(
            grantee_rows.rows.len(),
            1,
            "grantee sees only their own row"
        );

        // WRITES are constrained: a broadly-matching UPDATE from the grantee
        // (a plain WHERE clause is required by the engine's own unscoped-DML
        // guard, unrelated to RLS — `id IS NOT NULL` matches EVERY row a
        // naive scan would see) only ever touches the grantee's OWN row.
        grantee
            .execute("UPDATE notes SET body = 'edited' WHERE id IS NOT NULL")
            .await
            .expect("grantee's broadly-matching UPDATE is RLS-scoped to their own row");
        let owner_rows_after = read_rows(&owner, "SELECT id, body FROM notes")
            .await
            .expect("owner selects after grantee's UPDATE");
        assert_eq!(owner_rows_after.rows.len(), 1);
        assert_eq!(
            owner_rows_after.rows[0][1],
            serde_json::Value::String("owner note".to_string()),
            "the owner's row must be UNCHANGED by the grantee's UPDATE"
        );

        // A broadly-matching DELETE from the grantee (same `IS NOT NULL`
        // reasoning as the UPDATE above) only ever deletes the grantee's OWN
        // row.
        grantee
            .execute("DELETE FROM notes WHERE id IS NOT NULL")
            .await
            .expect("grantee's broadly-matching DELETE is RLS-scoped to their own row");
        let owner_rows_final = read_rows(&owner, "SELECT id FROM notes")
            .await
            .expect("owner selects after grantee's DELETE");
        assert_eq!(
            owner_rows_final.rows.len(),
            1,
            "the owner's row must SURVIVE the grantee's broadly-matching DELETE"
        );
    }

    // ── COPY FROM STDIN authorizes like INSERT (start_copy) ────────────────

    #[tokio::test]
    async fn wired_catalog_copy_authorizes_like_insert() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let tenant = "wired-catalog-tenant-copy";
        let state = test_state(&[CREATOR, "copy-owner", "copy-stranger"]);
        let graph = "wired-catalog-copy-graph";
        create_test_graph(&state, graph, 1).await;

        let owner = session_for(state.clone(), graph, "copy-owner", tenant);
        owner
            .execute("CREATE TABLE copy_target (id TEXT PRIMARY KEY, name TEXT)")
            .await
            .expect("owner creates copy_target");

        // The owner may enter copy-in mode against their own table.
        match owner
            .execute("COPY copy_target (id, name) FROM STDIN")
            .await
        {
            Ok(WireOutcome::CopyIn { .. }) => {}
            other => panic!("owner COPY should authorize and enter copy-in mode: {other:?}"),
        }

        let stranger = session_for(state.clone(), graph, "copy-stranger", tenant);
        let denied_real = stranger
            .execute("COPY copy_target (id, name) FROM STDIN")
            .await
            .expect_err("ungranted actor's COPY into a REAL table must be denied");
        let denied_absent = stranger
            .execute("COPY copy_target_missing (id, name) FROM STDIN")
            .await
            .expect_err("COPY into a nonexistent table must also be denied");
        assert_eq!(
            denied_real.message, denied_absent.message,
            "COPY's denial must read IDENTICALLY whether the table is real-but-forbidden \
             or nonexistent"
        );
    }

    // ── DDL (ALTER/DROP) requires Alter privilege ───────────────────────────

    #[tokio::test]
    async fn wired_catalog_alter_and_drop_require_alter_privilege() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let tenant = "wired-catalog-tenant-ddl";
        let state = test_state(&[CREATOR, "ddl-owner", "ddl-stranger"]);
        let graph = "wired-catalog-ddl-graph";
        create_test_graph(&state, graph, 1).await;

        let owner = session_for(state.clone(), graph, "ddl-owner", tenant);
        owner
            .execute("CREATE TABLE temp_tbl (id TEXT PRIMARY KEY)")
            .await
            .expect("owner creates temp_tbl");

        let stranger = session_for(state.clone(), graph, "ddl-stranger", tenant);
        let alter_denied = stranger
            .execute("ALTER TABLE temp_tbl ADD COLUMN extra TEXT")
            .await
            .expect_err("ungranted actor cannot ALTER another actor's table");
        assert!(alter_denied
            .message
            .contains(sql_catalog_acl::ACCESS_DENIED));
        let drop_denied = stranger
            .execute("DROP TABLE temp_tbl")
            .await
            .expect_err("ungranted actor cannot DROP another actor's table");
        assert!(drop_denied.message.contains(sql_catalog_acl::ACCESS_DENIED));

        owner
            .execute("ALTER TABLE temp_tbl ADD COLUMN extra TEXT")
            .await
            .expect("owner may ALTER their own table");
        owner
            .execute("DROP TABLE temp_tbl")
            .await
            .expect("owner may DROP their own table");
    }

    fn admin_session(
        state: Arc<RwLock<ServerState>>,
        graph: &str,
        agent_id: &str,
        tenant: &str,
    ) -> WireSession {
        let session = WireSession::new(state, graph.to_string());
        let authority = CarrierAuthority::from_verified(
            &VerifiedRequestContext::verified_for_test_with_scopes(agent_id, tenant, &["kg:admin"]),
        )
        .expect("build admin authority");
        session
            .bind_authority(authority, agent_id)
            .expect("bind admin identity");
        session
    }

    #[tokio::test]
    async fn property_graph_ddl_commits_and_rolls_back_with_its_base_table() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let tenant = "wired-catalog-tenant-pgq-txn";
        let graph = "wired-catalog-pgq-txn-graph";
        let state = test_state(&[CREATOR, "pgq-admin"]);
        create_test_graph(&state, graph, 1).await;
        let admin = admin_session(state.clone(), graph, "pgq-admin", tenant);

        admin.execute("BEGIN").await.unwrap();
        admin
            .execute("CREATE TABLE people (person_id TEXT PRIMARY KEY, name TEXT)")
            .await
            .unwrap();
        admin
            .execute(
                "CREATE PROPERTY GRAPH social VERTEX TABLES (\
                 people KEY (person_id) LABEL person PROPERTIES (name))",
            )
            .await
            .unwrap();
        admin.execute("COMMIT").await.unwrap();

        let persist_dir = test_persist_dir_of(&state).await;
        let canonical_tenant = admin
            .carrier_authority()
            .unwrap()
            .tenant_scope()
            .to_string();
        let store =
            medulla_server_api::sql_tables::tenant_table_store(&canonical_tenant, &persist_dir).unwrap();
        assert!(store.get_schema("people").unwrap().is_some());
        assert!(store
            .property_graph(
                &canonical_tenant,
                &medulla_query::tables::SqlName::new(vec![
                    medulla_query::tables::SqlIdentifier::unquoted("social").unwrap(),
                ])
                .unwrap(),
            )
            .unwrap()
            .is_some());

        admin.execute("BEGIN").await.unwrap();
        admin
            .execute("CREATE TABLE places (place_id TEXT PRIMARY KEY)")
            .await
            .unwrap();
        admin
            .execute(
                "CREATE PROPERTY GRAPH geography VERTEX TABLES (\
                 places KEY (place_id) LABEL place PROPERTIES (place_id))",
            )
            .await
            .unwrap();
        admin.execute("ROLLBACK").await.unwrap();
        assert!(store.get_schema("places").unwrap().is_none());
        assert!(!store
            .list_property_graphs()
            .unwrap()
            .contains(&"geography".to_string()));
    }

    #[tokio::test]
    async fn graph_select_grant_and_revoke_gate_live_graph_table_reads() {
        // Provisions the ONE shared test at-rest key (this module's `ensure_env` no
        // longer does, and must not) and then holds the ambient encryption env still
        // for this whole body, which opens a durable store. See
        // `medulla_kernel::crypto::provisioned_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::provisioned_test_env_read_lock().await;
        let tenant = "wired-catalog-tenant-pgq-grant";
        let graph = "wired-catalog-pgq-grant-graph";
        let state = test_state(&[CREATOR, "pgq-owner", "pgq-reader"]);
        create_test_graph(&state, graph, 1).await;
        let owner = admin_session(state.clone(), graph, "pgq-owner", tenant);
        owner
            .execute("CREATE TABLE people (person_id TEXT PRIMARY KEY, name TEXT)")
            .await
            .unwrap();
        owner
            .execute(
                "CREATE PROPERTY GRAPH social VERTEX TABLES (\
                 people KEY (person_id) LABEL person PROPERTIES (name))",
            )
            .await
            .unwrap();
        let persist_dir = test_persist_dir_of(&state).await;
        sql_catalog_acl::grant(
            &persist_dir,
            &authority("pgq-owner", tenant),
            "people",
            "pgq-reader",
            &[SqlPrivilege::Select],
            uuid::Uuid::new_v4(),
        )
        .unwrap();
        let reader = session_for(state, graph, "pgq-reader", tenant);
        let graph_sql = "SELECT * FROM GRAPH_TABLE (social MATCH (p:person) COLUMNS (p.name))";
        assert!(reader.execute(graph_sql).await.is_err());
        assert!(read_rows(
            &reader,
            "SELECT property_graph_name FROM information_schema.property_graphs"
        )
        .await
        .unwrap()
        .rows
        .is_empty());

        owner
            .execute("GRANT SELECT ON PROPERTY GRAPH social TO pgq_reader")
            .await
            .unwrap();
        // Unquoted SQL identifiers fold to lowercase and preserve underscores;
        // grant the exact verified id with quoting when punctuation differs.
        owner
            .execute("REVOKE SELECT ON PROPERTY GRAPH social FROM pgq_reader")
            .await
            .unwrap();
        owner
            .execute("GRANT SELECT ON PROPERTY GRAPH social TO \"pgq-reader\"")
            .await
            .unwrap();
        assert!(reader.execute(graph_sql).await.is_ok());
        assert_eq!(
            read_rows(
                &reader,
                "SELECT property_graph_name FROM information_schema.property_graphs"
            )
            .await
            .unwrap()
            .rows,
            vec![vec![serde_json::json!("social")]]
        );
        owner
            .execute("REVOKE SELECT ON PROPERTY GRAPH social FROM \"pgq-reader\"")
            .await
            .unwrap();
        assert!(reader.execute(graph_sql).await.is_err());
        assert!(read_rows(
            &reader,
            "SELECT property_graph_name FROM information_schema.property_graphs"
        )
        .await
        .unwrap()
        .rows
        .is_empty());
    }
}

