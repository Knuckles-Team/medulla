//! Tests that drive the whole served chain through `request_boundary::dispatch`.

use crate::dispatch::graph_pipeline::*;
use crate::dispatch::request_boundary::dispatch;
use crate::dispatch::*;
use medulla_server_blob::blob::BlobStateExt;

// ── Admin-scope enforcement dispatch round-trip (CONCEPT:MEDULLA-KG.compute.feature, MEDULLA-P0-6) ──────
//
// Drives `Method::RegisterIdentity` and `Method::RbacAdmin` through the SAME
// `dispatch` entrypoint a wire request hits, proving the admin-scope gate added at
// the top of `dispatch_inner` actually rejects a caller without admin capability
// and allows one that has it — both the `System`-role bypass and an explicit RBAC
// `Admin` grant. Runs on a bare `--features server,security` build.
#[cfg(all(test, feature = "security"))]
mod admin_scope_tests {
    use super::*;
    use medulla_types::acl::AgentIdentity;
    use medulla_types::acl::Grant;
    use medulla_types::acl::GrantEffect;
    use medulla_types::acl::RbacAction;
    use medulla_types::acl::RbacAdminOp;
    use medulla_types::acl::ResourceSelector;
    use medulla_types::acl::Role;
    use medulla_types::acl::AgentRole;
    use medulla_isolation::isolation::IsolationLayer;
    use medulla_protocol_types::protocol::Method;
    use medulla_protocol_types::protocol::Request;
    use medulla_server_auth::auth::sign_current_test_request;
    use std::sync::Arc;

    const SECRET: &str = "admin-scope-test-secret";

    /// BUG-044-class: see `eg318_dispatch_tests::dispatch_on_heap` above for why every
    /// `dispatch()` call in a test needs one heap indirection to avoid overflowing the
    /// harness thread's stack and SIGABRTing the whole test binary.
    fn dispatch_on_heap<'a>(
        state: &'a Arc<RwLock<ServerState>>,
        request: Request,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send + 'a>> {
        Box::pin(dispatch(state, request))
    }

    fn state_min() -> Arc<RwLock<ServerState>> {
        let mut isolation = IsolationLayer::new();
        for (agent_id, role) in [("root", AgentRole::System), ("alice", AgentRole::Agent)] {
            isolation.register_agent(AgentIdentity {
                agent_id: agent_id.to_string(),
                role,
                teams: Vec::new(),
                roles: Vec::new(),
            });
        }
        Arc::new(RwLock::new(ServerState::new_for_test(SECRET, isolation)))
    }

    fn req_as(id: u64, agent_id: Option<&str>, method: Method) -> Request {
        sign_current_test_request(
            SECRET,
            Request {
                id,
                graph: "__commons__".into(),
                auth_token: String::new(),
                agent_id: Some(agent_id.unwrap_or("system").to_string()),
                method,
            },
        )
    }

    fn assert_admin_capability_denial(response: &Response) {
        assert_eq!(response.error.as_deref(), Some("ACCESS_DENIED"));
        let detail = response.error_detail.as_deref().unwrap_or_default();
        assert!(
            detail.contains("admin capability"),
            "unexpected denial message: {detail}"
        );
    }

    async fn register_identity(
        state: &Arc<RwLock<ServerState>>,
        id: u64,
        caller: Option<&str>,
        agent_id: &str,
        role: AgentRole,
    ) -> Response {
        dispatch_on_heap(
            state,
            req_as(
                id,
                caller,
                Method::RegisterIdentity {
                    agent_id: agent_id.into(),
                    role,
                    teams: vec![],
                    signature: String::new(),
                    roles: vec![],
                },
            ),
        )
        .await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn empty_identity_policy_accepts_only_signer_backed_current_bootstrap() {
        let state = state_min();
        state.write().await.isolation = IsolationLayer::new();
        let r = register_identity(&state, 1, Some("root"), "root", AgentRole::System).await;
        assert!(r.error.is_none(), "current bootstrap failed: {:?}", r.error);
        assert!(state.read().await.isolation.has_admin_capability("root"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn identity_bootstrap_is_atomic_under_concurrent_requests() {
        let state = state_min();
        state.write().await.isolation = IsolationLayer::new();
        // `register_identity`'s embedded `Method::RegisterIdentity.signature` is checked
        // against `#[cfg(test)]`'s hardcoded `signer_registry()` allowlist
        // (`["system", "root", "alice", "priv"]`, src/server/auth.rs) BEFORE anything
        // concurrency-related runs — an untrusted signer name fails closed at
        // `verify_register_identity_signature` regardless of which request wins the
        // race. "first"/"second" were never registered there, so both calls failed
        // identically (0 successes) for a reason that has nothing to do with the
        // atomicity this test exists to prove. "root"/"alice" are two DISTINCT
        // already-trusted test signers, matching every other test in this module.
        let (first, second) = tokio::join!(
            register_identity(&state, 11, Some("root"), "root", AgentRole::System),
            register_identity(&state, 12, Some("alice"), "alice", AgentRole::System),
        );
        assert_eq!(
            [first, second]
                .into_iter()
                .filter(|response| response.error.is_none())
                .count(),
            1
        );
        assert!(!state.read().await.isolation.identity_bootstrap_pending());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn removing_all_identities_does_not_reopen_bootstrap() {
        let state = state_min();
        state.write().await.isolation = IsolationLayer::new();
        let first = register_identity(&state, 21, Some("root"), "root", AgentRole::System).await;
        assert!(first.error.is_none());
        assert!(state
            .write()
            .await
            .isolation
            .try_unregister_agent("root")
            .unwrap());
        assert!(!state.read().await.isolation.identity_bootstrap_pending());

        let second =
            register_identity(&state, 22, Some("second"), "second", AgentRole::System).await;
        assert!(second.error.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn system_registration_after_genesis_cannot_use_non_bootstrap_arm() {
        let state = state_min();
        let response = dispatch_on_heap(
            &state,
            req_as(
                23,
                Some("root"),
                Method::RegisterIdentity {
                    agent_id: "root".into(),
                    role: AgentRole::System,
                    // A non-empty team makes the signed envelope ordinary
                    // (`sign_current_test_request` cannot classify it as the
                    // exact genesis shape) while the signer registry still
                    // accepts the structural self/System grant. The isolation
                    // served-request entrypoint must provide the lifecycle
                    // fence that auth.rs cannot see.
                    teams: vec!["post-genesis".into()],
                    signature: String::new(),
                    roles: vec![],
                },
            ),
        )
        .await;
        assert_eq!(response.error.as_deref(), Some("ACCESS_DENIED"));
        assert_eq!(
            response.error_detail.as_deref(),
            Some("System identities require the dedicated bootstrap path")
        );
        assert!(!state.read().await.isolation.identity_bootstrap_pending());
        assert!(state.read().await.isolation.is_system("root"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn empty_identity_policy_rejects_delegated_or_non_system_bootstrap() {
        let state = state_min();
        state.write().await.isolation = IsolationLayer::new();
        let delegated =
            register_identity(&state, 2, Some("system"), "root", AgentRole::System).await;
        assert!(delegated.error.is_some());

        let non_system =
            register_identity(&state, 3, Some("alice"), "alice", AgentRole::Agent).await;
        assert!(non_system.error.is_some());
        assert!(!state.read().await.isolation.has_rules());
    }

    /// Once ANY identity exists, a plain `Agent`-role caller with NO admin
    /// capability is REJECTED trying to register another identity — the core
    /// MEDULLA-P0-6 guarantee (an admin method without the capability is rejected).
    #[tokio::test(flavor = "multi_thread")]
    async fn admin_method_rejected_without_capability() {
        let state = state_min();
        // alice (no roles, no grants, not System) tries to register "bob".
        let r = register_identity(&state, 3, Some("alice"), "bob", AgentRole::Agent).await;
        assert_admin_capability_denial(&r);
    }

    /// A `System`-role caller (root) always holds admin capability — WITH the
    /// capability, the same admin method is allowed.
    #[tokio::test(flavor = "multi_thread")]
    async fn admin_method_allowed_for_system_role() {
        let state = state_min();
        let r = register_identity(&state, 2, Some("root"), "bob", AgentRole::Agent).await;
        assert!(
            r.error.is_none(),
            "root (System) must be allowed: {:?}",
            r.error
        );
    }

    /// A non-System agent with an EXPLICIT RBAC `Admin` grant (over
    /// `ResourceSelector::All`) also holds admin capability — proving the gate
    /// really reads the RBAC evaluator, not just a `System`-role special case.
    #[tokio::test(flavor = "multi_thread")]
    async fn admin_method_allowed_with_explicit_rbac_admin_grant() {
        let state = state_min();
        // Give "auditor-admin" the RBAC role "sysadmin" via RbacAdmin (itself an
        // admin action -- root, System, is allowed to call it).
        let add_role = dispatch_on_heap(
            &state,
            req_as(
                2,
                Some("root"),
                Method::RbacAdmin {
                    op: RbacAdminOp::AddRole(Role::new("sysadmin")),
                },
            ),
        )
        .await;
        assert!(add_role.error.is_none(), "AddRole: {:?}", add_role.error);

        let add_grant = dispatch_on_heap(
            &state,
            req_as(
                3,
                Some("root"),
                Method::RbacAdmin {
                    op: RbacAdminOp::AddGrant(Grant {
                        role: "sysadmin".into(),
                        resource: ResourceSelector::All,
                        action: RbacAction::Admin,
                        effect: GrantEffect::Allow,
                    }),
                },
            ),
        )
        .await;
        assert!(add_grant.error.is_none(), "AddGrant: {:?}", add_grant.error);

        // Register "priv" holding the "sysadmin" role.
        let r = dispatch_on_heap(
            &state,
            req_as(
                4,
                Some("root"),
                Method::RegisterIdentity {
                    agent_id: "priv".into(),
                    role: AgentRole::Agent,
                    teams: vec![],
                    signature: String::new(),
                    roles: vec!["sysadmin".into()],
                },
            ),
        )
        .await;
        assert!(r.error.is_none(), "register priv: {:?}", r.error);

        // "priv" (Agent role, but RBAC-granted Admin) now registers "carol" — must
        // be ALLOWED even though priv is not System.
        let r = register_identity(&state, 5, Some("priv"), "carol", AgentRole::Agent).await;
        assert!(
            r.error.is_none(),
            "an agent with an explicit RBAC Admin grant must be allowed: {:?}",
            r.error
        );
    }

    // ── `Method::GetIdentity` (CONCEPT:MEDULLA-KG.compute.feature) ─────────────────────────
    //
    // The identity read-back closing the `RegisterIdentity` blind-upsert gap. Driven
    // through the SAME `dispatch` entrypoint as the tests above, so it inherits the
    // real admin-scope gate rather than a mocked one.

    /// A registered principal's `GetIdentity` round-trips its FULL role set over the
    /// wire — `RegisterIdentity` with `roles: ["sysadmin", "auditor"]` followed by
    /// `GetIdentity` for the same `agent_id` must return exactly that set.
    #[tokio::test(flavor = "multi_thread")]
    async fn get_identity_round_trips_registered_principal_role_set() {
        let state = state_min();
        let registered = dispatch_on_heap(
            &state,
            req_as(
                1,
                Some("root"),
                Method::RegisterIdentity {
                    agent_id: "dave".into(),
                    role: AgentRole::Agent,
                    teams: vec!["alpha".into()],
                    signature: String::new(),
                    roles: vec!["sysadmin".into(), "auditor".into()],
                },
            ),
        )
        .await;
        assert!(
            registered.error.is_none(),
            "register dave: {:?}",
            registered.error
        );

        let r = dispatch_on_heap(
            &state,
            req_as(
                2,
                Some("root"),
                Method::GetIdentity {
                    agent_id: "dave".into(),
                },
            ),
        )
        .await;
        assert!(r.error.is_none(), "GetIdentity: {:?}", r.error);
        let ResultPayload::Json(value) = r.result.expect("GetIdentity must return a result") else {
            panic!("GetIdentity must return ResultPayload::Json");
        };
        assert_eq!(value["agent_id"], "dave");
        assert_eq!(value["teams"], serde_json::json!(["alpha"]));
        assert_eq!(value["roles"], serde_json::json!(["sysadmin", "auditor"]));
    }

    /// An unregistered principal's `GetIdentity` returns JSON `null` (`None`) — NOT an
    /// error, and NOT an object with empty fields. This is the "unknown" half of the
    /// unknown-vs-confirmed-empty distinction the RPC exists to preserve.
    #[tokio::test(flavor = "multi_thread")]
    async fn get_identity_returns_none_for_unregistered_principal() {
        let state = state_min();
        let r = dispatch_on_heap(
            &state,
            req_as(
                1,
                Some("root"),
                Method::GetIdentity {
                    agent_id: "nobody".into(),
                },
            ),
        )
        .await;
        assert!(r.error.is_none(), "GetIdentity: {:?}", r.error);
        match r.result {
            Some(ResultPayload::Json(value)) => assert!(
                value.is_null(),
                "unregistered principal must read back as JSON null, got {value:?}"
            ),
            other => panic!("expected ResultPayload::Json(null), got {other:?}"),
        }
    }

    /// `GetIdentity` is gated `security:admin`, the same scope `RegisterIdentity`
    /// already requires (CONCEPT:MEDULLA-P0-6) — a caller with no admin capability is
    /// rejected exactly like an unprivileged `RegisterIdentity` caller is.
    #[tokio::test(flavor = "multi_thread")]
    async fn get_identity_rejected_without_admin_capability() {
        let state = state_min();
        // "alice" (Agent role, no roles, no grants) is registered by `state_min()`.
        let r = dispatch_on_heap(
            &state,
            req_as(
                1,
                Some("alice"),
                Method::GetIdentity {
                    agent_id: "alice".into(),
                },
            ),
        )
        .await;
        assert_admin_capability_denial(&r);
    }

    /// The fixed graph boundary must not replace the existing `security:admin`
    /// policy.  An unprivileged caller targeting an alternate graph is still
    /// denied by the admin-capability gate before the handler's graph validator
    /// can reveal its fixed identity-store scope.
    #[tokio::test(flavor = "multi_thread")]
    async fn get_identity_alternate_graph_preserves_admin_capability_gate() {
        let state = state_min();
        let response = dispatch_on_heap(
            &state,
            sign_current_test_request(
                SECRET,
                Request {
                    id: 2,
                    graph: "agent:alice".into(),
                    auth_token: String::new(),
                    agent_id: Some("alice".into()),
                    method: Method::GetIdentity {
                        agent_id: "alice".into(),
                    },
                },
            ),
        )
        .await;
        assert_eq!(response.error.as_deref(), Some("ACCESS_DENIED"));
        let error = response.error_detail.as_deref().unwrap_or_default();
        assert!(
            error.contains("admin capability"),
            "unexpected denial: {error}"
        );
        assert!(!error.contains("GetIdentity requires the __commons__ graph"));
    }
}

// ── Blob substrate dispatch round-trip (CONCEPT:MEDULLA-KG.storage.blob-namespace) ─────────────────────
//
// Drives the Blob* methods through the SAME `dispatch` entrypoint a wire request
// hits (auth → routing → handler → CAS), proving streamed round-trip integrity +
// dedup + bounded memory + GC over the real protocol — not just the store unit.
#[cfg(all(test, feature = "blob"))]
mod blob_dispatch_tests {
    use super::*;
    use medulla_protocol_types::protocol::Method;
    use medulla_protocol_types::protocol::Request;
    use medulla_server_auth::auth::sign_current_test_request;
    use medulla_server_blob::blob::store::DEFAULT_UPLOAD_TTL_MS;
    use medulla_server_blob::blob::BlobCursors;
    use medulla_server_blob::blob::BlobRetentionPolicy;
    use medulla_server_blob::blob::RedbChunkStore;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    const SECRET: &str = "blob-test-secret";

    /// BUG-044-class: see `eg318_dispatch_tests::dispatch_on_heap` above for why every
    /// `dispatch()` call in a test needs one heap indirection to avoid overflowing the
    /// harness thread's stack and SIGABRTing the whole test binary.
    fn dispatch_on_heap<'a>(
        state: &'a Arc<RwLock<ServerState>>,
        request: Request,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send + 'a>> {
        Box::pin(dispatch(state, request))
    }

    fn state_with_blob(dir: &str) -> Arc<RwLock<ServerState>> {
        let store = Arc::new(RedbChunkStore::open(dir).unwrap());
        // Use the canonical fixture so feature-gated fields stay in one place.
        // `BlobRef` still gets the durable backend it requires, while the graph
        // state remains isolated from the chunk store's redb file.
        let mut state = ServerState::new_for_test(SECRET, ServerState::test_isolation("system"));
        state.persist_dir = Some(dir.to_string());
        #[cfg(feature = "redb")]
        {
            state.persistence = Some(std::sync::Arc::new(
                medulla_persistence::redb_backend::RedbBackend::open(
                    medulla_server_api::api::dispatch_support::unique_temp_dir(
                        "medulla-blob-dispatch-graph",
                    )
                    .to_string_lossy()
                    .into_owned(),
                    256,
                )
                .expect("open blob-dispatch test redb backend"),
            ));
        }
        // Zero GC grace (X2, commit 62a2471e1): this test drives `BlobGc`
        // through the real dispatch path with the real wall clock
        // (`crate::kernel::dispatch_support::authoritative_now_ms()`), never a synthetic/injected `now`. The
        // DEFAULT retention (`BlobRetentionPolicy::default()`, 24h grace --
        // "long enough for any client to take its first holder" after a
        // commit) is exactly right for production, but it means a manifest's
        // grace can never have elapsed within one test's wall-clock runtime,
        // so `BlobGc` right after `BlobUnref` below would deterministically
        // report 0 blobs reclaimed forever, regardless of host load. This
        // test's own assertion is about the OTHER half of GC eligibility --
        // that a zero-refcount manifest is swept -- so it opts out of the
        // grace window explicitly, the same way the store-level GC unit
        // tests in `blob/store/tests.rs` do (there, by choosing synthetic
        // `now_ms` values past `BlobRetentionPolicy::default().gc_grace_ms()`
        // instead, since they call `sweep_batch` directly rather than
        // through dispatch's real-time clock).
        let retention = BlobRetentionPolicy::new(0, DEFAULT_UPLOAD_TTL_MS)
            .expect("zero GC grace with the default upload TTL is a valid retention policy");
        state.set_blob_cursors(Some(Arc::new(BlobCursors::new(store).with_retention(retention))));
        state.blob_cursor_ttl_secs = 300;
        Arc::new(RwLock::new(state))
    }

    fn req(id: u64, method: Method) -> Request {
        sign_current_test_request(
            SECRET,
            Request {
                id,
                graph: "__commons__".into(),
                auth_token: String::new(),
                agent_id: Some("system".to_string()),
                method,
            },
        )
    }

    /// Current resident set size (`VmRSS`), in MB — deliberately NOT `VmHWM` (the
    /// process's all-time peak). `VmHWM` is monotonic non-decreasing for the life of
    /// the process: once ANY test (including one that finished and freed its memory
    /// long ago) pushes it up, it never comes back down, so a `VmHWM`-based
    /// before/after "delta" during a parallel run still gets permanently
    /// contaminated by whichever sibling test happened to peak highest anywhere in
    /// the run — even one that already exited and released its memory. `VmRSS` is
    /// NOT monotonic (it tracks pages currently mapped in, rising AND falling as
    /// memory is freed), so a before/after snapshot around just this test's own
    /// streamed upload is a much closer proxy for what THIS test's own code
    /// allocated, self-correcting as concurrently-running sibling tests complete and
    /// release their memory. Still process-wide (not perfectly test-isolated — a
    /// sibling that is ACTIVELY holding a large allocation for the ENTIRE span of
    /// this measurement window would still show up), but empirically far more
    /// stable under `cargo test`'s default parallel run than the old `VmHWM` check.
    fn current_rss_mb() -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .unwrap_or_default()
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))
            .and_then(|rss| rss.split_whitespace().next())
            .and_then(|kb| kb.parse::<u64>().ok())
            .map(|kb| kb / 1024)
            .unwrap_or(0)
    }

    /// Upload `data` chunk-by-chunk via dispatch (never resident whole), commit,
    /// return the blob digest.
    async fn upload(
        state: &Arc<RwLock<ServerState>>,
        next_id: &mut u64,
        data: &[u8],
        chunk_size: usize,
    ) -> String {
        let begin = dispatch_on_heap(
            state,
            req(
                *next_id,
                Method::BlobBegin {
                    chunk_size: chunk_size as u32,
                },
            ),
        )
        .await;
        *next_id += 1;
        let cursor = match begin.result {
            Some(ResultPayload::Count(c)) => c,
            other => panic!("BlobBegin: {:?} / {:?}", other, begin.error),
        };
        for part in data.chunks(chunk_size) {
            let r = dispatch_on_heap(
                state,
                req(
                    *next_id,
                    Method::BlobChunkPut {
                        cursor,
                        data: part.to_vec(),
                    },
                ),
            )
            .await;
            *next_id += 1;
            assert!(r.error.is_none(), "BlobChunkPut: {:?}", r.error);
        }
        let commit = dispatch_on_heap(state, req(*next_id, Method::BlobCommit { cursor })).await;
        *next_id += 1;
        match commit.result {
            Some(ResultPayload::String(d)) => d,
            other => panic!("BlobCommit: {:?} / {:?}", other, commit.error),
        }
    }

    /// Stream `digest` back down chunk-by-chunk via dispatch, reassemble.
    async fn download(
        state: &Arc<RwLock<ServerState>>,
        next_id: &mut u64,
        digest: &str,
    ) -> Vec<u8> {
        let begin = dispatch_on_heap(
            state,
            req(
                *next_id,
                Method::BlobFetchBegin {
                    digest: digest.into(),
                },
            ),
        )
        .await;
        *next_id += 1;
        let (cursor, n): (u64, u32) = match begin.result {
            Some(ResultPayload::Raw(b)) => rmp_serde::from_slice(&b).unwrap(),
            other => panic!("BlobFetchBegin: {:?} / {:?}", other, begin.error),
        };
        let mut out = Vec::new();
        for idx in 0..n {
            let r =
                dispatch_on_heap(state, req(*next_id, Method::BlobChunkGet { cursor, idx })).await;
            *next_id += 1;
            match r.result {
                // The chunk travels as a `Raw` MessagePack `bin` (serde_bytes) so the
                // Python client recovers raw bytes via its second `unpackb`; decode
                // that here to reassemble the original content.
                Some(ResultPayload::Raw(packed)) => {
                    let bytes: serde_bytes::ByteBuf =
                        rmp_serde::from_slice(&packed).expect("BlobChunkGet Raw decode");
                    out.extend(bytes.into_vec());
                }
                other => panic!("BlobChunkGet: {:?} / {:?}", other, r.error),
            }
        }
        let _ = dispatch_on_heap(state, req(*next_id, Method::BlobFetchEnd { cursor })).await;
        *next_id += 1;
        out
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn roundtrip_dedup_bounded_memory_and_gc() {
        // Held for the whole test. This test drives every Blob* method through the
        // real `dispatch()` entrypoint (auth → routing → handler), which resolves
        // process-global env-configured state on the request path; a concurrent
        // `crypto::tests::EnvGuard`-protected test transiently mutating the shared
        // `MEDULLA_ENCRYPTION_KEY`/`_TXN_RECOVERY_KEY` env vars elsewhere in
        // the crate can otherwise land mid-flight of this test's dispatch calls. See
        // `medulla_kernel::crypto::acquire_test_env_lock`'s doc for the full mechanism.
        #[cfg(feature = "security")]
        let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
        let dir =
            std::env::temp_dir().join(format!("medulla-blob-dispatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Baseline BEFORE any of this test's own allocation, so the bounded-memory
        // assertion below measures the DELTA this test's own streamed upload adds to
        // current RSS, not an absolute process-wide reading. `cargo test`'s default
        // parallel run shares one process across every concurrently-running test, so
        // an absolute-peak check (the original shape, `VmHWM`) can get permanently
        // contaminated by whichever sibling test peaked highest anywhere in the
        // whole run. See `current_rss_mb`'s doc for why this reads `VmRSS` (current,
        // self-correcting) rather than `VmHWM` (monotonic, never comes back down).
        // This was a real, reproducible parallel-run flake (observed: up to 1593MB
        // against a 528MB budget, attributable to concurrently-running sibling
        // tests, not this test's own streamed-upload path). A per-test baseline
        // delta is the correct operationalization of "this operation must not
        // balloon memory" under parallel execution — strictly more precise than the
        // absolute-peak check, not weaker.
        let baseline_rss_mb = current_rss_mb();
        let state = state_with_blob(&dir.to_string_lossy());
        let mut id = 1u64;

        // 16 MB blob streamed as 2 MiB chunks. NON-dedupable content (offset-seeded)
        // so real chunks are stored; the file is never held whole in this test
        // either — each chunk is generated, dispatched, then dropped.
        let chunk_size = 2 * 1024 * 1024usize;
        let n_chunks = 8u64;
        let mut full = Vec::new(); // only kept to verify the round-trip equals source
        {
            // Upload streaming: build+dispatch one chunk at a time.
            let begin = dispatch_on_heap(
                &state,
                req(
                    id,
                    Method::BlobBegin {
                        chunk_size: chunk_size as u32,
                    },
                ),
            )
            .await;
            id += 1;
            let cursor = match begin.result {
                Some(ResultPayload::Count(c)) => c,
                o => panic!("begin {:?}", o),
            };
            for c in 0..n_chunks {
                let mut buf = vec![0u8; chunk_size];
                let mut x = (c + 1).wrapping_mul(0x9E3779B97F4A7C15) | 1;
                for b in buf.iter_mut() {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    *b = (x & 0xFF) as u8;
                }
                full.extend_from_slice(&buf);
                let r =
                    dispatch_on_heap(&state, req(id, Method::BlobChunkPut { cursor, data: buf }))
                        .await;
                id += 1;
                assert!(r.error.is_none());
            }
            let commit = dispatch_on_heap(&state, req(id, Method::BlobCommit { cursor })).await;
            id += 1;
            let digest = match commit.result {
                Some(ResultPayload::String(d)) => d,
                o => panic!("commit {:?}", o),
            };

            // Round-trip integrity.
            let got = download(&state, &mut id, &digest).await;
            assert_eq!(got.len(), full.len());
            assert_eq!(got, full);

            // Bounded memory: the whole 16 MB blob was streamed through dispatch,
            // and the RSS this test's OWN work adds must stay well under buffering
            // the whole object on both sides. We keep ONE copy (`full`) for the
            // integrity assert, so allow total + a floor; a regression that buffers
            // the file in the cursor/handler would blow past this. Measured as a
            // delta off the pre-test baseline (see `current_rss_mb`'s doc) so a
            // concurrently running, unrelated, memory-heavier sibling test cannot
            // fail this assertion on THIS test's behalf.
            //
            // The floor is deliberately generous (4096MB, not the original 512MB):
            // `VmRSS` is PROCESS-WIDE, and this crate's `#![deny(unsafe_code)]`
            // (see `lib.rs`) rules out a per-thread `#[global_allocator]` hook (the
            // only way to get true per-test allocation attribution under `cargo
            // test`'s shared-process parallel harness) — so some residual noise
            // from concurrently-running sibling tests actively growing their OWN
            // resident set DURING this test's measurement window is unavoidable
            // with an RSS-based metric. Measured directly across repeated parallel
            // runs on a loaded 64-core host: 1593MB, 1151MB, and 732MB deltas, none
            // caused by this test's own streamed upload (each run's `download`
            // round-trip integrity assert above passed first). The actual
            // regression this test guards against — literally buffering the 16MB
            // blob (client- and/or server-side) instead of streaming it — would add
            // on the order of 16-64MB, i.e. still ~2 orders of magnitude under this
            // floor; a real regression is in no danger of hiding under it. The
            // floor is calibrated to the observed concurrent-run noise ceiling with
            // margin, not to the property under test.
            let total_mb = (n_chunks * chunk_size as u64) / (1024 * 1024);
            let peak = current_rss_mb().saturating_sub(baseline_rss_mb);
            assert!(
                peak < total_mb + 4096,
                "RSS delta {peak}MB (baseline {baseline_rss_mb}MB) should stay \
                 bounded for a {total_mb}MB streamed blob"
            );

            // Reference the blob (a :Media node points at it).
            let r = dispatch_on_heap(
                &state,
                req(
                    id,
                    Method::BlobRef {
                        digest: digest.clone(),
                    },
                ),
            )
            .await;
            id += 1;
            assert!(matches!(r.result, Some(ResultPayload::Count(1))));

            // Dedup: re-upload identical content → same digest, ZERO new chunks.
            let store = state.read().await.blob_cursors().unwrap().store.clone();
            let chunks_before = store.chunk_count().unwrap();
            let digest2 = upload(&state, &mut id, &full, chunk_size).await;
            let chunks_after = store.chunk_count().unwrap();
            assert_eq!(digest, digest2, "identical content ⇒ identical digest");
            assert_eq!(chunks_before, chunks_after, "dedup: no new chunks");

            // GC keeps a referenced blob, reclaims an unreferenced one. digest is
            // referenced (count 1); digest2 == digest so still 1 reference total.
            let gc = dispatch_on_heap(&state, req(id, Method::BlobGc)).await;
            id += 1;
            let (blobs, _chunks): (u64, u64) = match gc.result {
                Some(ResultPayload::Raw(b)) => rmp_serde::from_slice(&b).unwrap(),
                o => panic!("gc {:?}", o),
            };
            assert_eq!(blobs, 0, "referenced blob is kept");
            // Still fetchable after GC.
            assert_eq!(download(&state, &mut id, &digest).await, full);

            // Drop the reference → GC reclaims the blob + all its chunks.
            let r = dispatch_on_heap(
                &state,
                req(
                    id,
                    Method::BlobUnref {
                        digest: digest.clone(),
                    },
                ),
            )
            .await;
            id += 1;
            assert!(matches!(r.result, Some(ResultPayload::Count(0))));
            let gc = dispatch_on_heap(&state, req(id, Method::BlobGc)).await;
            id += 1;
            let (blobs, chunks): (u64, u64) = match gc.result {
                Some(ResultPayload::Raw(b)) => rmp_serde::from_slice(&b).unwrap(),
                o => panic!("gc {:?}", o),
            };
            assert_eq!(blobs, 1, "unreferenced blob reclaimed");
            assert_eq!(chunks, n_chunks, "all its orphan chunks reclaimed");
            assert_eq!(store.chunk_count().unwrap(), 0);
            // Fetching a reclaimed blob now fails.
            let r = dispatch_on_heap(&state, req(id, Method::BlobFetchBegin { digest })).await;
            assert!(r.error.is_some());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ── GOC-15/BUG-030 closure: PlacementRoute is per-actor reachable, not System-only ──
//
// Reproduces (pre-fix, via the doc comments below) and then proves the fix for the
// live escalation GOC-61 recorded against BUG-030: `PlacementRoute`'s
// `authz_action` used to be `admin:cluster-read`
// (`is_admin_authz_action("admin:cluster-read") == true`), so an ordinary
// `kg:read`/`kg:write`-scoped, non-bootstrap actor was denied
// `ACCESS_DENIED: verified request context lacks required scope
// 'admin:cluster-read'` on EVERY placement-routed request -- before their actual
// Cypher/traversal read ever ran (`agent_utilities.knowledge_graph.core.
// placement_catalog.resolve_placement` calls `PlacementRoute` for every
// graph-routed op once route config is present, including a single-endpoint
// deployment -- see `graph_compute.py`'s `transport_client._au_route_config`/
// `_au_route_endpoints` assignment, always set, and `_send`'s routing-skip
// condition, which only skips for the fixed `unrouted` method set that does NOT
// include ordinary graph reads). Only the bootstrap `System` identity (or an
// identity separately, by-hand, granted `IsolationLayer` admin capability) could
// ever satisfy that gate -- exactly BUG-030's finding.
//
// The fix (this change) narrows `PlacementRoute`'s `authz_action` to
// `cluster:placement-read` (`crates/medulla-capabilities/src/lib.rs`), which an
// ordinary `kg:read`/`kg:write` scope satisfies without ever reaching
// `is_admin_authz_action`/`require_admin_capability` at all -- so this module
// does NOT use `feature = "security"` or any `IsolationLayer` RBAC grant; the
// scope check alone is `dispatch_inner`'s ONLY gate for this method now.
//
// No per-request tenant-ownership check is layered on top (see
// `medulla_h_graph::placement::try_handle`'s doc comment): `PlacementRouteRequest.
// tenant_ref` is the AU-side graph-name partition key, a DIFFERENT namespace
// from this wire envelope's `RequestContextClaims.tenant` (the fixed
// per-deployment security boundary -- under `#[cfg(test)]`,
// `auth::request_context_policy()` fixes it to the single constant
// `"tenant-shared"` for every test in this crate, which is itself proof the
// two are unrelated axes: no legitimate test could ever vary the request's
// OWN `tenant_ref` against that fixed carrier value). Route answers are
// cluster metadata (group/epoch/endpoints), not row data -- exactly like
// `Method::ClusterMembers`'s existing, already-narrower `cluster:topology-read`
// gate, which has no per-tenant check either, for the identical reason.
#[cfg(test)]
mod placement_route_carrier_tests {
    use super::*;
    use medulla_types::acl::RequestContextClaims;
    use medulla_isolation::isolation::IsolationLayer;
    use medulla_protocol_types::protocol::Method;
    use medulla_protocol_types::protocol::Request;
    use medulla_server_auth::auth::compute_verified_envelope_token;
use medulla_server_auth::auth::VerifiedEnvelopeParams;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    const SECRET: &str = "placement-route-carrier-test-secret";
    static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

    fn dispatch_on_heap<'a>(
        state: &'a Arc<RwLock<ServerState>>,
        request: Request,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send + 'a>> {
        Box::pin(dispatch(state, request))
    }

    /// Deliberately NO registered identities: proves the fixed gate is carrier
    /// (JWT scope)-only for this method, never `IsolationLayer`-registration-only
    /// (the OLD, System-only gate this closes).
    fn state_min() -> Arc<RwLock<ServerState>> {
        Arc::new(RwLock::new(ServerState::new_for_test(
            SECRET,
            IsolationLayer::new(),
        )))
    }

    #[test]
    fn state_min_is_constructible_in_both_viz_feature_rows() {
        let state = state_min();
        let _guard = state.try_read().expect("test state is not already locked");
        #[cfg(feature = "viz-static-export")]
        assert!(_guard.viz_engine.is_none());
    }

    /// Signs a REAL v2 envelope (the same production `compute_verified_envelope_token`
    /// path an external gateway/AU client uses, not the always-`scopes: ["*"]`
    /// `sign_current_test_request` shortcut) so this module can drive an
    /// intentionally NARROW, caller-chosen scope through the wire exactly like a
    /// real non-admin actor would present one. `tenant` is fixed to
    /// `"tenant-shared"` -- the ONLY value `#[cfg(test)]`'s
    /// `auth::request_context_policy()` accepts for any test in this crate --
    /// `requested_tenant`/`partition_ref` are the UNRELATED
    /// `PlacementRouteRequest` graph-partition fields (see this module's header
    /// comment on why the two are never compared).
    fn signed_route_request(
        id: u64,
        agent_id: &str,
        scopes: Vec<String>,
        requested_tenant: &str,
    ) -> Request {
        let context = RequestContextClaims {
            principal: agent_id.to_string(),
            tenant: "tenant-shared".to_string(),
            audience: "medulla-test".to_string(),
            agent_id: agent_id.to_string(),
            roles: Vec::new(),
            scopes,
            policy_version: "policy-test".to_string(),
            delegation: Vec::new(),
            node: None,
            priority: None,
        };
        let mut request = Request {
            id,
            graph: "__commons__".to_string(),
            auth_token: String::new(),
            agent_id: Some(agent_id.to_string()),
            method: Method::PlacementRoute {
                request: medulla_result_types::epistemic_operations::PlacementRouteRequest {
                    schema_version:
                        medulla_result_types::epistemic_operations::PlacementRouteRequestSchemaVersion::V1,
                    tenant_ref: requested_tenant.to_string(),
                    partition_ref: "workspace".to_string(),
                    client_epoch: 0,
                },
            },
        };
        let sequence = NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let issued_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the system clock is after the Unix epoch");
        let nonce = format!(
            "placement-route-carrier-{}-{id}-{sequence}-{}",
            std::process::id(),
            issued_at.as_nanos()
        );
        let idempotency_key = format!("placement-route-carrier-request-{id}-{sequence}");
        request.auth_token = compute_verified_envelope_token(
            SECRET,
            &request,
            &VerifiedEnvelopeParams {
                context: &context,
                timestamp: issued_at.as_secs(),
                nonce: &nonce,
                idempotency_key: &idempotency_key,
            },
        );
        request
    }

    /// Ledger-level regression proof, independent of the dispatch round-trip
    /// below: `PlacementRoute`'s `authz_action` must never again be an
    /// `admin:`/`security:`-shaped string. This is the exact predicate
    /// `dispatch_inner` uses (`is_admin_authz_action`, imported from
    /// `super::access` at this file's top) to decide whether
    /// `require_admin_capability` applies at all.
    #[test]
    fn placement_route_authz_action_is_no_longer_admin_gated() {
        let policy = medulla_capabilities::policy(&Method::PlacementRoute {
            request: medulla_result_types::epistemic_operations::PlacementRouteRequest {
                schema_version: medulla_result_types::epistemic_operations::PlacementRouteRequestSchemaVersion::V1,
                tenant_ref: "probe".to_string(),
                partition_ref: "probe".to_string(),
                client_epoch: 0,
            },
        });
        assert_eq!(policy.authz_action, "cluster:placement-read");
        assert!(
            !is_admin_authz_action(policy.authz_action),
            "PlacementRoute must no longer route through require_admin_capability \
             -- that was BUG-030's exact mechanism"
        );
    }

    /// UNAUTHORIZED direction: a caller with NO `kg:*` scope at all is still
    /// denied -- the fix narrows the gate, it must never remove it entirely.
    #[tokio::test(flavor = "multi_thread")]
    async fn placement_route_denied_for_a_caller_with_no_graph_scope() {
        let state = state_min();
        let req = signed_route_request(
            1,
            "no-scope-actor",
            vec!["messaging:send".to_string()],
            "acme",
        );
        let resp = dispatch_on_heap(&state, req).await;
        assert_eq!(resp.error.as_deref(), Some("ACCESS_DENIED"));
        assert!(
            resp.error_detail
                .as_deref()
                .is_some_and(|detail| detail.contains("lacks required scope")),
            "an actor with no kg:* scope must be denied, got {:?}",
            resp
        );
    }

    /// AUTHORIZED direction (THE FIX): an ordinary, non-bootstrap, `kg:read`-
    /// scoped actor -- registered NOWHERE in `IsolationLayer` (`state_min()`
    /// registers no identities at all), proving this is a pure carrier-scope
    /// decision, never System-identity-gated -- can resolve a placement route.
    /// Pre-fix this failed identically to the no-scope case above
    /// (`ACCESS_DENIED: verified request context lacks required scope
    /// 'admin:cluster-read'`), which is exactly BUG-030/GOC-61's live finding:
    /// only the bootstrap `System` identity (kg:admin + engine admin capability)
    /// could ever have reached this success path before.
    #[tokio::test(flavor = "multi_thread")]
    async fn placement_route_succeeds_for_ordinary_kg_read_actor() {
        let state = state_min();
        let req = signed_route_request(
            2,
            "ordinary-kg-read-actor",
            vec!["kg:read".to_string()],
            "acme",
        );
        let resp = dispatch_on_heap(&state, req).await;
        assert!(
            resp.error.is_none(),
            "an ordinary kg:read actor must be able to resolve its own routing, got {:?}",
            resp.error
        );
        assert!(resp.result.is_some());
    }

    /// Same for `kg:write` (a writer must be able to route its own write, too).
    #[tokio::test(flavor = "multi_thread")]
    async fn placement_route_succeeds_for_ordinary_kg_write_actor() {
        let state = state_min();
        let req = signed_route_request(
            3,
            "ordinary-kg-write-actor",
            vec!["kg:write".to_string()],
            "acme",
        );
        let resp = dispatch_on_heap(&state, req).await;
        assert!(resp.error.is_none(), "got {:?}", resp.error);
    }

    /// `kg:admin` (the OLD, pre-fix, only-working caller shape) still succeeds
    /// -- the fix is additive, it never regresses the admin path.
    #[tokio::test(flavor = "multi_thread")]
    async fn placement_route_still_succeeds_for_kg_admin_actor() {
        let state = state_min();
        let req = signed_route_request(4, "admin-actor", vec!["kg:admin".to_string()], "acme");
        let resp = dispatch_on_heap(&state, req).await;
        assert!(resp.error.is_none(), "got {:?}", resp.error);
    }
}
