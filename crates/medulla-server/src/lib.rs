//! medulla-server: dispatch, router and startup composition. Split out of the `medulla-engine` facade (MEDULLA-CONTRACT-R056.11);
//! level 6 in `scripts/crate_split/manifest.toml`.

#![recursion_limit = "256"]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

















/// Moved from the facade (MEDULLA-CONTRACT-R056).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod server_startup;


// ---- merged from the moved module root (src/server/mod.rs) ----
use medulla_server_ops::api::query_txn::ForeignStateExt;
use medulla_server_cdc::cdc::CdcStateExt;
use medulla_h_query::semantic_index_runtime::PendingRf019Activation;
// CONCEPT:MEDULLA-KG.query.tokio-service-server — Tokio Service Server
//
// Long-running Tokio server that holds the GraphRegistry in memory
// and serves requests over UDS or TCP with HMAC-SHA256 authentication.

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod seams_install;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod issuer_trust;

/// Verified minimum stack for engine Tokio workers.
///
/// Full multimodal/job publication dispatch can exceed Tokio's 2 MiB default in
/// debug and instrumented builds. Keep one explicit product value shared by the
/// production runtime and exact integration runtimes; environment variables are
/// not part of this safety contract.
pub const ENGINE_WORKER_STACK_BYTES: usize = 4 * 1024 * 1024;

const _: () = assert!(ENGINE_WORKER_STACK_BYTES >= 4 * 1024 * 1024);

/// Start the engine's async driver on an explicitly sized stack.
///
/// Tokio's `thread_stack_size` applies only to runtime-owned workers; calling
/// `Runtime::block_on` from a default-sized launcher would still execute the
/// outer driver future on that launcher's stack. Production and exact tests use
/// this helper so both the outer driver and Tokio workers share the same verified
/// minimum. Spawn diagnostics are deliberately normalized and contain no host,
/// path, or identity data.
pub fn spawn_engine_driver<F, T>(driver: F) -> std::io::Result<std::thread::JoinHandle<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    std::thread::Builder::new()
        .name("medulla-runtime-driver".to_string())
        .stack_size(ENGINE_WORKER_STACK_BYTES)
        .spawn(driver)
        .map_err(|error| {
            std::io::Error::new(error.kind(), "engine runtime driver thread could not start")
        })
}

/// Join the engine driver without reflecting a panic payload into logs or errors.
pub fn join_engine_driver<T>(driver: std::thread::JoinHandle<T>) -> std::io::Result<T> {
    // `JoinHandle::join` is a `disallowed_methods` entry because an unbounded
    // wait turns a fault elsewhere into a hang here. This is the one join where
    // that reasoning does not apply: the engine runtime driver runs for the
    // whole life of the server, so this wait IS the process's lifetime rather
    // than a wait for a concurrent party to arrive. Any deadline here would
    // mean "stop serving after N seconds", and there is no bounded replacement
    // for "run until the server is asked to stop".
    // Invariant `process-lifetime-join`: docs/architecture/liveness_invariants.md.
    #[allow(clippy::disallowed_methods)]
    driver
        .join()
        .map_err(|_| std::io::Error::other("engine runtime driver terminated unexpectedly"))
}

/// Decode the node ids carried by a successful unified-query response.
///
/// This deliberately lives at the server boundary because both in-crate query
/// tests and out-of-crate integration fixtures exercise the same response
/// contract. It is hidden from generated API documentation but remains public
/// so Cargo's integration-test crates can share the assertion semantics.
#[doc(hidden)]
pub fn decode_unified_ids(response: &medulla_protocol_types::protocol::Response) -> Vec<String> {
    let rows: Vec<(String, Option<f32>)> = rmp_serde::from_slice(raw_result(response)).unwrap();
    rows.into_iter().map(|(id, _)| id).collect()
}

/// Decode the row ids of a successful `Method::Uql` response (a `Rows` or `Profile`
/// result) — the `decode_unified_ids` of the one query-text surface (EH-434).
#[cfg(feature = "query")]
#[doc(hidden)]
pub fn decode_uql_ids(response: &medulla_protocol_types::protocol::Response) -> Vec<String> {
    use medulla_types::wire::UqlResult;
    let rows = match rmp_serde::from_slice(raw_result(response)).unwrap() {
        UqlResult::Rows { rows, .. } | UqlResult::Profile { rows, .. } => rows,
        UqlResult::Explain { .. } => panic!("an EXPLAIN result has no rows"),
    };
    rows.into_iter().map(|row| row.id).collect()
}

/// The raw payload of a successful query response.
fn raw_result(response: &medulla_protocol_types::protocol::Response) -> &[u8] {
    assert!(
        response.error.is_none(),
        "query error: {:?}",
        response.error
    );
    match response.result.as_ref() {
        Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) => bytes,
        other => panic!("expected Raw result, got {other:?}"),
    }
}

#[cfg(test)]
mod direct_wire_security_tests {
    use super::{join_engine_driver, spawn_engine_driver, ENGINE_WORKER_STACK_BYTES};

    #[test]
    fn engine_driver_uses_the_verified_stack_contract_and_joins_exactly() {
        assert_eq!(ENGINE_WORKER_STACK_BYTES, 4 * 1024 * 1024);
        let driver = spawn_engine_driver(|| 41_u64).unwrap();
        assert_eq!(join_engine_driver(driver).unwrap(), 41);
    }

    #[test]
    fn engine_driver_join_failure_does_not_reflect_the_panic_payload() {
        let driver =
            spawn_engine_driver(|| -> u64 { panic!("opaque-driver-test-failure") }).unwrap();
        let error = join_engine_driver(driver).unwrap_err();
        assert_eq!(
            error.to_string(),
            "engine runtime driver terminated unexpectedly"
        );
        assert!(!error.to_string().contains("opaque-driver-test-failure"));
    }
}

/// Validate secure request-context configuration and initialize its durable
/// replay adapter before listeners are opened.
pub fn validate_verified_request_context_startup(
    secret: &str,
    state_dir: Option<&str>,
) -> Result<(), String> {
    medulla_server_auth::auth::validate_verified_context_startup(secret, state_dir)
}
// CDC -> Kafka sink (CA-11, feature `cdc-kafka`, off by default). Reserved and EMPTY
// until CA-11 implements it; landed by CA-17's single feature-stub commit so CA-11 never
// edits this shared module list.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
#[cfg(feature = "cdc-kafka")]
pub mod cdc_sink;
// Semantic ANN index activation (W0.4, CONCEPT:MEDULLA-KG.storage.semantic-index-directory): the ONE
// reopen-else-build-else-activate body plus the two triggers the boot-time task
// cannot supply — the post-write hook and the periodic backstop — for a graph
// created, or crossing `ANN_BUILD_THRESHOLD`, after boot. Gated with the `ann`
// feature the mechanism requires; its durable tier additionally needs `ann-redb`.
#[cfg(feature = "ann")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod semantic_activation;


/// Validate the persisted RF-019 semantic-index activation state before the listeners bind.
#[cfg(feature = "ann-redb")]
pub fn preflight_rf019_activation(
    persist_dir: &std::path::Path,
) -> Result<PendingRf019Activation, String> {
    medulla_h_query::semantic_index_runtime::preflight_rf019_activation(persist_dir)
}

/// Install the validated RF-019 semantic-index activation once the registry exists.
#[cfg(feature = "ann-redb")]
pub fn install_rf019_activation(
    pending: PendingRf019Activation,
    persist_dir: &std::path::Path,
) -> Result<(), String> {
    medulla_h_query::semantic_index_runtime::install_rf019_activation(pending, persist_dir)
}

/// Operator-only, offline RF-019 observation. The public server route remains
/// fenced until signed activation and serving are composed.
#[cfg(feature = "ann-redb")]
pub fn observe_rf019_tenant_owner(
    persist_dir: &std::path::Path,
    tenant: &str,
    binding_ids: &[String],
    max_source_bytes: u64,
    max_owner_bytes: u64,
) -> Result<serde_json::Value, String> {
    medulla_h_query::semantic_index_runtime::observe_rf019_tenant_owner(
        persist_dir,
        tenant,
        binding_ids,
        max_source_bytes,
        max_owner_bytes,
    )
}
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod dispatch;
// Fleet server registry stale-lease reaper (CONCEPT:MEDULLA-KG.sharding.server-registry, W2.5): periodic
// sweep that expires a `:Server` node whose `Method::RegisterServer`-issued
// lease has lapsed. Always declared (mirrors `semantic_activation` above) — the
// sweep is a no-op when nothing has registered.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod handlers;
// The owner-scoped foreign-source catalog, host-driven foreign reads and source sharing live
// under `api::query_txn` (R056.8).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod registry_reaper;
// EH-558: policy-driven expiry of sealed records through their owning op.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sealed_retention;

/// Tests that need the full server: the router, seam installation and the redb backend.
#[cfg(test)]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod split_tests;

// OBDA wire method + Python client seam (CA-13, feature `obda-wire`, off by default).
// Reserved and EMPTY until CA-13 implements it; landed by CA-17's single feature-stub
// commit so CA-13 never edits this shared module list. The OBDA mapping/rewrite engine
// itself already exists behind the `obda` feature this one implies.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
#[cfg(feature = "obda-wire")]
pub mod obda;



// Cross-region async read-replica tier (CONCEPT:MEDULLA-KG.sharding.follower-pull-loop) + capacity guardrails
// (CONCEPT:MEDULLA-KG.coordination.circuit-breaker): a bounded LSN replication log the primary ships over `/replicate`, an
// async follower pull-loop that applies the tail via the canonical `mutation_apply::apply` path, and
// the pure circuit-breaker / per-tenant-quota / backpressure guards the transport + the
// follower consult. Behind `federation-search` (reuses the same pure-Rust `ureq` stack —
// NO new dep, kept OUT of the Pi tier).
#[cfg(feature = "federation-search")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod replica;
// The durable cross-store commit-intent log (`txn_intent`) lives under
// `api::query_txn` (R056.8).

// External path surface — `server::ServerState`, `server::MAX_BATCH_IDS`,
// `server::dispatch`, and
// `server::{handle_connection,serve_uds,serve_tcp}` — used by main.rs/persist.rs/tests.
// NL planner injection (CONCEPT:MEDULLA-KG.query.fence-stripper): an embedder opts into engine-driven NL→query.
// Distributed-compute materialized-view boot reload (CONCEPT:MEDULLA-KG.storage.feature): the binary
// calls this on startup to repopulate the in-RAM matview index from redb.
// Post-bind maintenance hook (MEDULLA-DURABLE-KERNEL-R006.4): the binary calls this
// from `serve_transports` right after the listeners bind.
use medulla_server_api::state::{txn_limits_from_env, ServerState};

/// Build the canonical feature-complete state used by dispatch and wire tests.
/// Keeping the constructor and every feature-gated service attachment here
/// means a new `ServerState` field cannot silently disappear from one fixture.
/// The helper is test-only and lives at the server seam so unconditional server
/// tests do not depend on the optional `wire` feature being enabled.
#[cfg(test)]
pub(crate) fn test_state_with_services(
    auth_secret: &str,
    isolation: medulla_isolation::isolation::IsolationLayer,
    persistence_label: &str,
    tsdb_label: &str,
) -> std::sync::Arc<tokio::sync::RwLock<ServerState>> {
    let _ = (persistence_label, tsdb_label);
    let state = ServerState::new_for_test(auth_secret.to_owned(), isolation);
    #[cfg(any(feature = "query", feature = "redb", feature = "tsdb"))]
    let mut state = state;
    #[cfg(feature = "query")]
    {
        state.persist_dir = Some(
            medulla_server_auth::sql_tables::test_persist_dir()
                .to_string_lossy()
                .into_owned(),
        );
    }
    #[cfg(feature = "redb")]
    {
        state.persistence = Some(std::sync::Arc::new(
            medulla_persistence::redb_backend::RedbBackend::open(
                medulla_server_api::api::dispatch_support::unique_temp_dir(persistence_label)
                    .to_string_lossy()
                    .into_owned(),
                256,
            )
            .expect("open test redb backend"),
        ));
    }
    #[cfg(feature = "tsdb")]
    {
        state.tsdb_store = Some(std::sync::Arc::new(
            medulla_tsdb::store::SeriesStore::open_in_dir(
                &medulla_server_api::api::dispatch_support::unique_temp_dir(tsdb_label),
                medulla_kernel::store_authority::process_verifier(),
                medulla_kernel::store_authority::process_authority().principal(),
                &medulla_kernel::store_authority::process_authority().proof(),
            )
            .expect("open test series store"),
        ));
    }
    std::sync::Arc::new(tokio::sync::RwLock::new(state))
}

#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod transport;

// serve_uds is Unix-only (UnixListener); on Windows the server uses serve_tcp,
// so gate the re-export to keep the windows-msvc wheel building (main.rs already
// guards its serve_uds call with #[cfg(unix)]).
// parse_unix_socket_mode is pure parsing/validation (no fs calls) and is exported
// unconditionally so main.rs can validate --socket-mode/GRAPH_SERVICE_SOCKET_MODE
// up front on every platform, even though the mode itself is only applied by
// serve_uds on unix.

/// CA-17-W00 feature-stub contract.
///
/// Six features are declared empty and default-off so that lanes CA-11..CA-16 each edit
/// only their own `src/server/<feature>/` directory and their owning crate's manifest,
/// never the root `Cargo.toml` `[features]` table or this file's module list -- both of
/// which a dozen concurrent eg lanes share. These tests pin the two properties that make
/// that safe: each reserved feature keeps the parent its owning lane builds on, and none
/// of them leaks into a release bundle before its lane has implemented anything.
///
/// The check is textual against the manifest (rather than `cfg!(feature = ..)`) so that
/// it reports the same verdict no matter which `--features` the test run itself was
/// built with.
#[cfg(test)]
mod ca17_feature_stub_contract {
    const CARGO_TOML: &str = include_str!("../../../Cargo.toml");

    /// `(feature name, the exact dependency list its owning lane builds on)`.
    const RESERVED: [(&str, &str); 6] = [
        // CA-11 widened this from `["streaming"]` (CA-17's original stub) to route
        // `dep:medulla-stream`/`medulla-stream/cdc-kafka` -- see the Cargo.toml comment on the
        // `cdc-kafka` line and CA-11's report for why (the `CdcSink` trait/`rdkafka`
        // producer live in `medulla-stream`, which nothing else pulls in without `stream`).
        (
            "cdc-kafka",
            "[\"streaming\", \"dep:medulla-stream\", \"medulla-stream/cdc-kafka\"]",
        ),
        ("sparql-fuseki", "[\"sparql\"]"),
        ("obda-wire", "[\"obda\"]"),
        ("federation-opensearch", "[\"federation-search\"]"),
        ("lineage-transport", "[\"lake\"]"),
        // HTTP-only policy export shares the verified security authority. It has
        // no Method or capability-ledger feature to propagate.
        ("policy_export", "[\"security\"]"),
    ];

    /// Release/aggregate bundles a reserved feature must not appear in. Each is an
    /// existing `[features]` key whose value transitively defines a shipped artifact.
    const BUNDLES: [&str; 5] = ["default", "full", "all", "full-extras", "cluster"];

    fn declaration(feature: &str) -> &'static str {
        let prefix = format!("{feature} = ");
        let mut found = CARGO_TOML.lines().filter(|line| line.starts_with(&prefix));
        let line = found.next().unwrap_or_else(|| {
            panic!("reserved feature {feature:?} is not declared in Cargo.toml")
        });
        assert!(
            found.next().is_none(),
            "reserved feature {feature:?} is declared more than once"
        );
        line
    }

    #[test]
    fn every_reserved_feature_keeps_the_parent_its_lane_builds_on() {
        for (feature, parent) in RESERVED {
            let line = declaration(feature);
            assert_eq!(
                line,
                format!("{feature} = {parent}"),
                "reserved feature {feature:?} no longer implies exactly {parent}; a lane \
                 changing its own feature's parents must say so in its report"
            );
        }
    }

    #[test]
    fn no_reserved_feature_leaks_into_a_release_bundle() {
        for bundle in BUNDLES {
            let line = declaration(bundle);
            for (feature, _) in RESERVED {
                assert!(
                    !line.contains(&format!("\"{feature}\"")),
                    "bundle {bundle:?} enables reserved feature {feature:?}, which is still \
                     an empty placeholder -- a lane adding it to a bundle must land its \
                     implementation in the same commit"
                );
            }
        }
    }

    /// The point of the stub commit is that the shared module lists already carry every
    /// gate, so no later lane has to touch them. Pin that the gate lines are present.
    #[test]
    fn every_reserved_module_gate_is_already_declared_in_its_shared_module_list() {
        for (file, source, gate, module) in [
            (
                "src/server/mod.rs",
                include_str!("lib.rs"),
                "cdc-kafka",
                "pub mod cdc_sink;",
            ),
            (
                "crates/medulla-h-query/src/lib.rs",
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../medulla-h-query/src/lib.rs")),
                "sparql-fuseki",
                "pub mod sparql_service;",
            ),
            (
                "src/server/mod.rs",
                include_str!("lib.rs"),
                "obda-wire",
                "pub mod obda;",
            ),
            (
                "src/server/mod.rs",
                include_str!("lib.rs"),
                "policy_export",
                "pub mod policy_export;",
            ),
            (
                "src/server/federation/mod.rs",
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../medulla-h-query/src/federation/mod.rs"
                )),
                "federation-opensearch",
                "pub mod opensearch;",
            ),
            (
                "src/server/lake/mod.rs",
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../medulla-server-lake/src/lake/mod.rs"
                )),
                "lineage-transport",
                "pub mod lineage_transport;",
            ),
        ] {
            let expected = format!("#[cfg(feature = \"{gate}\")]\n{module}");
            assert!(
                source.contains(&expected),
                "{file} is missing the CA-17-W00 gate for {gate:?}: {expected:?}"
            );
        }
    }
}

#[cfg(test)]
mod tests;
