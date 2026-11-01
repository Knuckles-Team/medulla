//! Startup steps of the `medulla-server` binary.
//!
//! Each function owns one boot concern `run_inner` used to spell out inline:
//! startup refusals, admission sizing, opening one durable store, the shutdown
//! signal, the transports, and the periodic memory/lifecycle sweep ticks. Every
//! refusal keeps its exact message and exit status; a store-open failure stays
//! fatal at boot (loud + early).

use std::net::SocketAddr;
use std::sync::Arc;

use tracing::info;

use medulla_server_api::state::ServerState;

/// Service configuration read by the startup steps (the binary flattens this into its CLI).
#[derive(clap::Args, Debug)]
pub struct ServerArgs {
    /// Unix Domain Socket path.
    /// Falls back to $GRAPH_SERVICE_SOCKET, then $XDG_RUNTIME_DIR/medulla.sock,
    /// then /tmp/medulla.sock.
    #[arg(long, env = "GRAPH_SERVICE_SOCKET")]
    pub socket_path: Option<String>,

    /// Octal file-mode applied to the UDS socket right after bind (e.g. `0600`,
    /// `0660`). **Default `0600`** (owner-only) — byte-for-byte today's
    /// hardcoded behavior; set this instead of relying on an external `chmod`
    /// watcher when a non-root client container needs group access (e.g. a
    /// `fsGroup`-owned socket directory under `readOnlyRootFilesystem`).
    /// Validated at startup and REFUSED if it would be world-writable or
    /// world-readable — this can loosen who may `connect()`, never who may
    /// read the graph over that connection (`eg2.` auth still applies), but a
    /// world-open UDS is refused outright rather than accepted and footgunned.
    #[arg(long, default_value = "0600", env = "GRAPH_SERVICE_SOCKET_MODE")]
    pub socket_mode: String,

    /// Serve only native TCP instead of also binding a Unix socket.
    #[arg(long, env = "GRAPH_SERVICE_TCP_ONLY", requires = "tcp_addr")]
    pub tcp_only: bool,

    /// Optional TCP address (e.g., 0.0.0.0:9100). If set, TCP listener is started.
    #[arg(long, env = "GRAPH_SERVICE_TCP_ADDR")]
    pub tcp_addr: Option<String>,

    /// PEM certificate chain for native TCP TLS.
    #[arg(long, env = "GRAPH_SERVICE_TLS_CERT")]
    pub tcp_tls_cert: Option<String>,

    /// PEM private key for native TCP TLS.
    #[arg(long, env = "GRAPH_SERVICE_TLS_KEY")]
    pub tcp_tls_key: Option<String>,

    /// Optional PEM CA bundle for required client-certificate authentication.
    #[arg(long, env = "GRAPH_SERVICE_TLS_CLIENT_CA")]
    pub tcp_tls_client_ca: Option<String>,

    /// Explicit isolated local anonymous mode. Defaults to authenticated.
    #[arg(long, env = "GRAPH_SERVICE_AUTH_MODE", default_value = "authenticated", value_parser = ["authenticated", "none"])]
    pub auth_mode: String,

    /// HMAC-SHA256 shared secret for authentication.
    #[arg(long, env = "GRAPH_SERVICE_AUTH_SECRET", default_value = "")]
    pub auth_secret: String,

    /// Directory for the authoritative durable store.
    #[arg(long, env = "GRAPH_SERVICE_PERSIST_DIR")]
    pub persist_dir: Option<String>,

    /// Ebbinghaus decay sweep interval in seconds (0 = disabled).
    #[arg(long, default_value = "0", env = "GRAPH_SERVICE_DECAY_INTERVAL")]
    pub decay_interval: u64,

    /// Half-life (seconds) for the periodic decay sweep. Default: 7 days.
    #[arg(long, default_value = "604800", env = "GRAPH_SERVICE_DECAY_HALF_LIFE")]
    pub decay_half_life: f64,

    /// Prune nodes/edges whose decayed confidence falls below this floor
    /// (0 = decay only, never prune).
    #[arg(long, default_value = "0.0", env = "GRAPH_SERVICE_DECAY_FLOOR")]
    pub decay_floor: f64,

    /// Prometheus /metrics HTTP listener address (e.g. 127.0.0.1:9101).
    /// Disabled when unset. Separate from the MessagePack RPC transports.
    #[arg(long, env = "GRAPH_SERVICE_METRICS_ADDR")]
    pub metrics_addr: Option<String>,

    /// W3C SPARQL 1.1 Protocol HTTP listener address (e.g. 127.0.0.1:7878), feature
    /// `sparql-http`. Disabled when unset. Lets existing Stardog/Jena/rdflib SPARQL
    /// clients query + update the engine unchanged. Separate from the RPC transports.
    #[arg(long, env = "MEDULLA_SPARQL_ADDR")]
    pub sparql_addr: Option<String>,

    /// CA-16 (DEC-CA-04) `/policy/export` HTTP listener address (e.g.
    /// 127.0.0.1:7879), feature `policy_export`. Disabled when unset. Serves the
    /// M1 row-visibility policy bundle as JSON; admin-gated by an OIDC bearer
    /// token (`kg:admin`/`policy:export`) via the SAME primary-protocol OIDC
    /// validator as the `eg2.` RPC surface (`oidc::JwtValidator::from_env_primary`)
    /// -- no separate credential shape. Separate from the RPC transports.
    #[arg(long, env = "MEDULLA_POLICY_EXPORT_ADDR")]
    pub policy_export_addr: Option<String>,

    /// Loopback address for the authenticated GraphQL subscription SSE carrier
    /// (e.g. 127.0.0.1:7879), feature `graphql`. Non-loopback binds are refused;
    /// remote access must use a same-host TLS reverse proxy. Every subscription
    /// requires a current eg2 envelope, graph ACL, and RLS projection.
    #[arg(long, env = "MEDULLA_GRAPHQL_ADDR")]
    pub graphql_addr: Option<String>,

    /// Maximum concurrent GraphQL SSE handshakes and sessions.
    #[arg(long, env = "MEDULLA_GRAPHQL_MAX_CONNECTIONS", default_value_t = 128)]
    pub graphql_max_connections: usize,

    /// Maximum GraphQL SSE session lifetime before a fresh eg2 envelope is required.
    #[arg(long, env = "MEDULLA_GRAPHQL_MAX_SESSION_SECS", default_value_t = 300)]
    pub graphql_max_session_secs: u64,

    /// Observability log-ingestion HTTP listener address (e.g. 127.0.0.1:5080),
    /// feature `obs` (CONCEPT:AU-KG.ingest.self-ingest/161). Disabled when unset. Accepts OTLP/HTTP
    /// (`/v1/logs`), Elasticsearch `_bulk`/`_doc`, and JSON-lines log records, landing
    /// them in medulla-tsdb series + medulla-text full-text indices and rolling Parquet segments
    /// into the blob CAS. Separate from the RPC transports.
    #[arg(long, env = "MEDULLA_OBS_ADDR")]
    pub obs_addr: Option<String>,

    /// Interactive native-visualization HTTP listener address (e.g.
    /// 127.0.0.1:5090), feature `viz-interactive` (D-VZ-1 lane V3b). Disabled
    /// when unset. Serves the reference WebGPU/WebGL2 client (`GET /`) and the
    /// binary viewport-tile protocol (`GET /tile`) a browser cannot reach over
    /// the length-prefixed MessagePack RPC transports — separate from those and
    /// from every other auxiliary HTTP surface. Loopback-only, like every
    /// auxiliary listener; a remote client goes through a same-host reverse
    /// proxy (this fleet's `edge-ingress`), never a direct non-loopback bind.
    #[arg(long, env = "MEDULLA_VIZ_INTERACTIVE_ADDR")]
    pub viz_interactive_addr: Option<String>,

    /// Iceberg-REST catalog HTTP listener address (e.g. 127.0.0.1:8181), feature
    /// `lake-rest` (INT-P2-3, `iceberg.apache.org/rest-catalog-spec`). Disabled when
    /// unset. Serves list/load (+ a compaction-bridged commit) over the tables the
    /// `lake` materialization tier writes, so a standard Iceberg REST client (PyIceberg/
    /// Spark/Trino) can list + load them. Separate from the RPC transports and the
    /// `/sparql`/`/obs` surfaces.
    #[arg(long, env = "MEDULLA_ICEBERG_ADDR")]
    pub iceberg_addr: Option<String>,

    /// Super-cluster federated-search HTTP listener address (e.g. 127.0.0.1:7900),
    /// feature `federation-search` (CONCEPT:MEDULLA-KG.ontology.federation-client). Disabled when unset. Serves a
    /// `/federated` POST (`{query, lang}`) that fans the read query across the peers in
    /// `MEDULLA_FEDERATION_PEERS` AND the local store, then unions/de-dups +
    /// RRF-re-ranks the partials (a slow/dead peer degrades to `partial: true`). Separate
    /// from the RPC transports and the `/sparql` surface.
    #[arg(long, env = "MEDULLA_FEDERATED_ADDR")]
    pub federated_addr: Option<String>,

    /// Self-terminate after N seconds with ZERO active connections (reference-
    /// counted idle shutdown). 0 or absent ⇒ NEVER self-terminate on idle: the
    /// engine is long-living/persistent and runs forever like a normal server.
    /// N>0 ⇒ a shared tiny daemon shuts itself down (checkpointing cleanly) N
    /// seconds after its last client disconnects; a new connection during the
    /// grace period cancels the timer. SIGTERM/SIGINT graceful shutdown works in
    /// BOTH modes. agent-utilities' EngineResolver passes this to its autostarted
    /// tiny daemon.
    #[arg(long, default_value = "0", env = "MEDULLA_IDLE_SHUTDOWN_SECS")]
    pub idle_shutdown_secs: u64,
}

pub fn native_tcp_addr_is_loopback(addr: &str) -> bool {
    addr.parse::<SocketAddr>()
        .map(|value| value.ip().is_loopback())
        .unwrap_or_else(|_| {
            addr.rsplit_once(':')
                .map(|(host, port)| {
                    host.trim_matches(|character| character == '[' || character == ']')
                        .eq_ignore_ascii_case("localhost")
                        && !port.is_empty()
                        && port.chars().all(|character| character.is_ascii_digit())
                })
                .unwrap_or(false)
        })
}

type SharedState = Arc<tokio::sync::RwLock<ServerState>>;
type BoxError = Box<dyn std::error::Error>;

fn exit_with(message: impl std::fmt::Display, code: i32) -> ! {
    eprintln!("error: {message}");
    std::process::exit(code);
}


pub fn socket_mode_or_exit(raw: &str) -> u32 {
    crate::transport::parse_unix_socket_mode(raw).unwrap_or_else(|reason| {
        exit_with(
            format_args!("invalid --socket-mode/GRAPH_SERVICE_SOCKET_MODE {raw:?}: {reason}"),
            2,
        )
    })
}

/// Security gate: an auth secret and a durable-state directory are mandatory.
pub fn require_secret_and_persist_dir(auth_secret: &str, persist_dir: Option<&str>) {
    if auth_secret.is_empty() {
        exit_with(
            "no auth secret configured — refusing to start.\n\
             Set GRAPH_SERVICE_AUTH_SECRET (or pass --auth-secret) to enable \
             HMAC-SHA256 authentication.",
            2,
        );
    }
    if persist_dir.is_none() {
        exit_with(
            "the served engine requires an externally configured durable-state directory",
            2,
        );
    }
}

/// Validate the TCP TLS configuration (both halves or neither; TLS for any
/// non-loopback address) and prepare it.
pub async fn prepare_tcp_tls(
    args: &ServerArgs,
) -> Result<Option<crate::transport::PreparedTcpTls>, BoxError> {
    let config = match (&args.tcp_tls_cert, &args.tcp_tls_key) {
        (Some(cert_path), Some(key_path)) => Some(crate::transport::TcpTlsConfig {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            client_ca_path: args.tcp_tls_client_ca.clone(),
        }),
        (None, None) if args.tcp_tls_client_ca.is_none() => None,
        _ => exit_with(
            "native TCP TLS requires both certificate and private-key material",
            2,
        ),
    };
    let insecure_remote =
        |addr: &str| !native_tcp_addr_is_loopback(addr) && config.is_none();
    if args.tcp_addr.as_deref().is_some_and(insecure_remote) {
        exit_with("non-loopback native TCP requires TLS", 2);
    }
    Ok(match config {
        Some(tls) => Some(crate::transport::prepare_tcp_tls(tls).await?),
        None => None,
    })
}

/// Detect the host capacity and log the startup banner, including the native
/// TCP listener's TLS posture when one is configured.
pub fn detect_capacity_and_log_startup(
    args: &ServerArgs,
    tcp_tls: bool,
) -> medulla_kernel::autosize::Capacity {
    info!("Starting medulla-server");
    if args.tcp_only {
        info!("  Native TCP-only transport configured");
    } else {
        info!("  UDS: private local socket configured");
    }
    let capacity = medulla_kernel::autosize::detect_capacity();
    info!(
        "  Capacity: {} cpu(s), {} MiB RAM, tier {:?} (auto-sizing inflight/writer/node-cap defaults)",
        capacity.cpus,
        capacity.total_ram_bytes / (1024 * 1024),
        capacity.tier
    );
    if capacity.total_ram_bytes == 0 {
        tracing::warn!(
            "  RAM undetectable (non-Linux or a restricted /proc) — defaulting the \
             per-graph node cap to a conservative {} (the same cap a real 1 GiB Pi \
             gets); override with MEDULLA_MAX_NODES_PER_GRAPH",
            capacity.node_cap()
        );
    }
    if args.tcp_addr.is_some() {
        info!(
            "  TCP: configured (tls={}, mtls={})",
            tcp_tls,
            args.tcp_tls_client_ca.is_some()
        );
    }
    info!("  Auth: enabled");
    capacity
}

/// Flush any bounded writer work that had not yet crossed its acknowledgement
/// barrier once the accept loop has exited.
pub fn flush_durable_state(persistence: Persistence) {
    info!("Accept loop stopped — flushing durable state");
    match medulla_server_api::request_replay::seal_request_replay_ledger() {
        Ok(handed_off) => info!("Request replay window handed off ({handed_off} nonces)"),
        Err(error) => tracing::warn!(
            "Request replay handoff failed; the next start refuses the last accepted second: {error}"
        ),
    }
    if let Some(p) = &persistence {
        p.shutdown();
    }
    info!("Shutdown complete");
}

/// Refuse to start if another engine already owns this persist dir; the lock is
/// held for the whole process lifetime by the caller.
pub fn acquire_persist_lock(
    persist_dir: Option<&str>,
) -> Option<medulla_kernel::persist_lock::PersistDirLock> {
    let dir = persist_dir?;
    let lock = medulla_kernel::persist_lock::acquire(dir).unwrap_or_else(|e| exit_with(e, 1));
    info!("Acquired the configured single-writer persistence lock");
    migrate_stores_on_open(dir, &lock);
    Some(lock)
}

/// Migrate an older but supported store before anything opens it, under the
/// lock just taken (MEDULLA-DURABLE-KERNEL-R070.1). With the switch off, nothing is
/// migrated and `open_persistence` refuses such a store and names the command.
#[cfg(feature = "redb")]
fn migrate_stores_on_open(dir: &str, lock: &medulla_kernel::persist_lock::PersistDirLock) {
    use medulla_persistence::store_upgrade as upgrade;

    let enabled = upgrade::auto_migrate_enabled().unwrap_or_else(|error| exit_with(error, 2));
    if !enabled {
        info!(
            "Automatic store migration is off ({}=false)",
            upgrade::AUTO_MIGRATE_ENV
        );
        medulla_kernel::metrics::store_migration_step("run", "disabled");
        return;
    }
    let migration = upgrade::migrate_on_open(std::path::Path::new(dir), lock)
        .unwrap_or_else(|error| exit_with(error, 1));
    if let Some(refusal) = migration.startup_refusal() {
        exit_with(refusal, 1);
    }
}

#[cfg(not(feature = "redb"))]
fn migrate_stores_on_open(_dir: &str, _lock: &medulla_kernel::persist_lock::PersistDirLock) {}

/// A positive `variable` override, bounded by `bound`. Absent, unparsable and
/// zero values yield `None`.
fn bounded_env_override(variable: &str, bound: usize) -> Option<usize> {
    std::env::var(variable)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .map(|value| medulla_kernel::autosize::bound_explicit(value, bound))
}

/// Bound an explicit `requested` value by `automatic`, warning when it clamps.
fn clamp_to_automatic(variable: &str, requested: usize, automatic: usize) -> usize {
    let bounded = medulla_kernel::autosize::bound_explicit(requested, automatic);
    if bounded != requested {
        tracing::warn!(
            requested,
            bounded,
            automatic,
            "{variable} exceeds cgroup-aware automatic capacity; clamping"
        );
    }
    bounded
}

/// In-flight admission sizing (CONCEPT:AU-KG.backend.b-auto-size).
pub struct AdmissionLimits {
    pub max_in_flight: usize,
    pub per_graph_inflight_limit: usize,
    pub read_reserved: usize,
}

/// Size the admission pools from effective CPU capacity. A positive env
/// override can only lower each cgroup-aware default.
pub fn admission_limits(capacity: &medulla_kernel::autosize::Capacity) -> AdmissionLimits {
    let automatic_max_in_flight = capacity.max_inflight();
    let max_in_flight = std::env::var("MEDULLA_MAX_INFLIGHT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .map(|value| clamp_to_automatic("MEDULLA_MAX_INFLIGHT", value, automatic_max_in_flight))
        .unwrap_or(automatic_max_in_flight);
    // Per-graph fairness cap (Phase C-D): default to a quarter of the global pool
    // so any one hot graph holds at most 25% of capacity and ~4 graphs can saturate
    // the server, instead of a single tenant monopolizing all in-flight slots.
    let per_graph_inflight_limit =
        bounded_env_override("MEDULLA_MAX_INFLIGHT_PER_GRAPH", max_in_flight)
            .unwrap_or_else(|| (max_in_flight / 4).max(1));
    // Reserved READ-admission lane (CONCEPT:MEDULLA-KG.coordination.reserved-read-lane): a
    // dedicated pool of in-flight slots that ONLY reads/queries may use, so a write
    // firehose that saturates the global pool + per-graph cap can never shed an
    // interactive MCP read to BUSY (an eighth of the admission cap, floored).
    let automatic_read_reserved = capacity.read_reserved().min(max_in_flight).max(1);
    let read_reserved = bounded_env_override("MEDULLA_READ_RESERVED", automatic_read_reserved)
        .unwrap_or(automatic_read_reserved);
    info!(
        "Backpressure: max in-flight = {} (per-graph cap = {}, reserved read lane = {})",
        max_in_flight, per_graph_inflight_limit, read_reserved
    );
    // Per-graph write coalescer (CONCEPT:MEDULLA-KG.sharding.per-graph-write-coalescer): batch
    // size auto-sized from cpu count and always enabled with bounded queues.
    let coalescer = medulla_kernel::write_coalescer::CoalescerConfig::auto();
    info!(
        "Write coalescer: batch up to {} ops/lock (queue {}, linger {:?})",
        coalescer.max_batch, coalescer.queue_capacity, coalescer.max_linger
    );
    AdmissionLimits {
        max_in_flight,
        per_graph_inflight_limit,
        read_reserved,
    }
}

type Persistence = Option<Arc<dyn medulla_server_api::api::persistence::PersistenceBackend>>;

/// The served engine has one durability implementation: authoritative redb.
/// Every acknowledged mutation crosses the commit barrier, and bounded writer
/// queues apply backpressure rather than dropping work.
#[cfg(feature = "redb")]
pub fn open_persistence(
    persist_dir: Option<&str>,
    capacity: &medulla_kernel::autosize::Capacity,
) -> Persistence {
    let dir = persist_dir?;
    let automatic_writer_queue = capacity.writer_queue();
    let queue = bounded_env_override("MEDULLA_REDB_WRITER_QUEUE", automatic_writer_queue)
        .unwrap_or(automatic_writer_queue);
    refuse_upgradable_stores(dir);
    info!("Persistence: authoritative redb (queue {})", queue);
    let backend = medulla_persistence::redb_backend::RedbBackend::open(dir.to_string(), queue)
        .unwrap_or_else(|error| {
            exit_with(
                format_args!("failed to open durable graph store: {error}"),
                1,
            )
        });
    Some(Arc::new(backend))
}

/// Log whether the configured SPARQL federation endpoint is reachable.
///
/// Best-effort and logged, never enforced: startup continues either way. It
/// runs the same guarded service client the live `SERVICE <endpoint> {…}`
/// dispatch uses, so a reachable line is evidence the federation path works.
/// A no-op unless both `MEDULLA_FUSEKI_HEALTH_CHECK_ENDPOINT` (the
/// endpoint to probe) and `MEDULLA_SPARQL_SERVICE_ALLOW` (the
/// fail-closed allowlist) are set.
pub fn run_fuseki_startup_health_check() {
    #[cfg(feature = "sparql-fuseki")]
    {
        let outcome = medulla_h_query::sparql_service::startup_health_check();
        match outcome {
            medulla_h_query::sparql_service::HealthCheckOutcome::Reachable { .. } => {
                info!("{}", outcome.summary());
            }
            medulla_h_query::sparql_service::HealthCheckOutcome::NotConfigured => {
                tracing::debug!("{}", outcome.summary());
            }
            _ => tracing::warn!("{}", outcome.summary()),
        }
    }
}

/// A data directory still holding a store that a registered offline upgrade
/// applies to is refused before any store is opened, with that store's named
/// error and the exact operator command. Automatic migration at lock time
/// leaves no such store, so this refuses only with the switch off.
#[cfg(feature = "redb")]
fn refuse_upgradable_stores(dir: &str) {
    let checked =
        medulla_persistence::store_upgrade::refuse_upgradable_stores(std::path::Path::new(dir));
    if let Err(refusal) = checked {
        exit_with(refusal, 1);
    }
}

/// `RedbBackend` is gated behind the `redb` feature, so a build without it fails
/// loudly at boot on an attempted `--persist-dir` rather than silently
/// downgrading to in-memory.
#[cfg(not(feature = "redb"))]
pub fn open_persistence(
    persist_dir: Option<&str>,
    _capacity: &medulla_kernel::autosize::Capacity,
) -> Persistence {
    if persist_dir.is_some() {
        exit_with(
            "--persist-dir requires a redb-enabled build (the `redb` feature is not compiled in)",
            2,
        );
    }
    None
}

/// Native time-series store (feature `tsdb`): a durable `series.redb` beside the
/// graph shards when a persist dir is set, else a process-temp file.
#[cfg(feature = "tsdb")]
pub fn open_tsdb_store(
    persist_dir: Option<&str>,
) -> Option<Arc<medulla_tsdb::store::SeriesStore>> {
    let path = match persist_dir {
        Some(dir) => std::path::Path::new(dir).join("series.redb"),
        None => std::env::temp_dir().join(format!("medulla-tsdb-{}.redb", std::process::id())),
    };
    let authority = medulla_kernel::store_authority::process_authority();
    let verifier = medulla_kernel::store_authority::process_verifier();
    match medulla_tsdb::store::SeriesStore::open(
        &path,
        verifier,
        authority.principal(),
        &authority.proof(),
    ) {
        Ok(store) => {
            info!("Time-series store (tsdb): durable store ready");
            Some(Arc::new(store))
        }
        Err(e) => {
            tracing::error!("failed to open durable time-series store: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(feature = "blob")]
type BlobSubstrate = (Option<Arc<medulla_server_api::blob::BlobCursors>>, u64);

/// Streamed content-addressed BLOB substrate (CONCEPT:MEDULLA-KG.storage.blob-namespace).
/// With no persist dir there is no durable place for the bytes, so the substrate
/// is disabled and the Blob* methods report "not available".
#[cfg(feature = "blob")]
pub fn open_blob_substrate(persist_dir: Option<&str>) -> BlobSubstrate {
    let ttl = std::env::var("MEDULLA_BLOB_CURSOR_TTL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(300u64);
    let Some(dir) = persist_dir else {
        tracing::warn!("Blob substrate disabled: no persist dir (blob bytes need durable storage)");
        return (None, ttl);
    };
    let store = open_blob_chunk_store(dir);
    info!("Blob substrate: durable content-addressed CAS ready");
    (Some(Arc::new(medulla_server_api::blob::BlobCursors::new(store))), ttl)
}

#[cfg(all(feature = "blob", feature = "blob-s3"))]
fn open_blob_chunk_store(dir: &str) -> Arc<dyn medulla_server_api::blob::ChunkStore> {
    match medulla_server_api::blob::s3::S3ChunkStore::open(dir) {
        Ok(store) => Arc::new(store),
        Err(e) => exit_with(format_args!("failed to open durable blob-s3 CAS: {e}"), 1),
    }
}

#[cfg(all(feature = "blob", not(feature = "blob-s3")))]
fn open_blob_chunk_store(dir: &str) -> Arc<dyn medulla_server_api::blob::ChunkStore> {
    match medulla_server_api::blob::RedbChunkStore::open(dir) {
        Ok(store) => Arc::new(store),
        Err(e) => exit_with(format_args!("failed to open durable blob CAS: {e}"), 1),
    }
}

/// Generic Key→Value store (feature `kv`): a durable `{persist_dir}/kv.redb` when
/// a persist dir is set, else an in-memory scratch map.
#[cfg(feature = "kv")]
pub fn open_kv_store(persist_dir: Option<&str>) -> Option<Arc<medulla_server_api::kv::KvStore>> {
    let store = medulla_server_api::kv::KvStore::open(persist_dir)
        .unwrap_or_else(|e| exit_with(format_args!("failed to open kv store: {e}"), 1));
    if store.is_durable() {
        info!(
            "Key→Value store (kv): durable kv.redb (CONCEPT:MEDULLA-OS.config.configurable-listeners)"
        );
    } else {
        info!("Key→Value store (kv): in-memory scratch — no persist dir");
    }
    Some(Arc::new(store))
}

/// Every durable store the served state owns, opened at boot.
pub struct DurableStores {
    pub persistence: Persistence,
    #[cfg(feature = "tsdb")]
    tsdb_store: Option<Arc<medulla_tsdb::store::SeriesStore>>,
    #[cfg(feature = "blob")]
    blob: BlobSubstrate,
    #[cfg(feature = "kv")]
    kv: Option<Arc<medulla_server_api::kv::KvStore>>,
}

/// Open the persistence backend and the feature-gated tsdb/blob/kv stores, in
/// that order.
pub fn open_durable_stores(
    persist_dir: Option<&str>,
    capacity: &medulla_kernel::autosize::Capacity,
) -> DurableStores {
    DurableStores {
        persistence: open_persistence(persist_dir, capacity),
        #[cfg(feature = "tsdb")]
        tsdb_store: open_tsdb_store(persist_dir),
        #[cfg(feature = "blob")]
        blob: open_blob_substrate(persist_dir),
        #[cfg(feature = "kv")]
        kv: open_kv_store(persist_dir),
    }
}

/// Install the values sized or opened at boot into the canonical state.
///
/// The complete feature-gated field composition stays in `ServerState::new`;
/// startup overrides only these values, so the orchestration path cannot drift
/// from other state users.
pub fn compose_server_state(
    mut state: ServerState,
    persist_dir: Option<String>,
    stores: DurableStores,
    limits: &AdmissionLimits,
    (txn_ttl_secs, txn_max_per_graph, txn_max_per_agent): (u64, usize, usize),
) -> ServerState {
    crate::seams_install::install_seams(&state);
    #[cfg(all(feature = "streaming", feature = "cdc-kafka"))]
    if let Some(hub) = &state.cdc {
        // CA-11 (DEC-CA-03): install the optional Kafka sink exactly once,
        // after pure state composition and before any listener can serve.
        crate::cdc_sink::install_from_env(hub);
    }
    #[cfg(feature = "redb")]
    {
        // Upper-crate service handles are type-erased on `ServerState`; the
        // composition root installs the always-present ones (R056.8).
        use medulla_persistence::cold_offload::ColdTrackerStateExt;
        let _ = state.ensure_cold_tracker();
    }
    #[cfg(feature = "lake")]
    {
        use crate::lake::LakeStateExt;
        state.set_lake(Arc::default());
    }
    state.persist_dir = persist_dir;
    state.persistence = stores.persistence;
    state.max_in_flight = Arc::new(tokio::sync::Semaphore::new(limits.max_in_flight));
    state.read_admission = Arc::new(tokio::sync::Semaphore::new(limits.read_reserved));
    state.per_graph_inflight_limit = limits.per_graph_inflight_limit;
    state.txn_ttl_secs = txn_ttl_secs;
    state.txn_max_per_graph = txn_max_per_graph;
    state.txn_max_per_agent = txn_max_per_agent;
    #[cfg(feature = "blob")]
    {
        (state.blob, state.blob_cursor_ttl_secs) = stores.blob;
    }
    #[cfg(feature = "tsdb")]
    {
        state.tsdb_store = stores.tsdb_store;
    }
    #[cfg(feature = "kv")]
    {
        state.kv = stores.kv;
    }
    state
}

/// RLS is unconditionally default-deny: every served request carries a verified
/// tenant context and resolves through provisioned durable identity/RBAC policy
/// before any row is exposed.
#[cfg(feature = "security")]
pub fn open_isolation_layer(
    auth_secret: &str,
    persist_dir: Option<&str>,
    anonymous: bool,
) -> medulla_isolation::isolation::IsolationLayer {
    info!("RLS default-deny ACTIVE: rows require explicit public visibility or an owner grant");
    let Some(dir) = persist_dir else {
        exit_with(
            "secure request context requires --persist-dir / GRAPH_SERVICE_PERSIST_DIR for durable identity policy and replay state",
            1,
        );
    };
    let authority = medulla_kernel::store_authority::process_authority();
    let mut isolation = medulla_isolation::isolation::IsolationLayer::with_persist_dir(
        dir,
        authority.as_ref(),
        authority.principal(),
        &authority.proof(),
    )
    .unwrap_or_else(|error| {
        exit_with(
            format_args!("could not open durable identity/RBAC policy: {error}"),
            1,
        )
    });
    if anonymous {
        isolation
            .try_register_agent(medulla_types::acl::AgentIdentity {
                agent_id: "anonymous".into(),
                role: medulla_types::acl::AgentRole::System,
                teams: Vec::new(),
                roles: Vec::new(),
            })
            .unwrap_or_else(|error| exit_with(error, 1));
        info!("Explicit anonymous local-owner authority ACTIVE; no authentication credentials accepted");
        return isolation;
    }
    if let Err(error) = crate::validate_verified_request_context_startup(auth_secret, persist_dir)
    {
        exit_with(
            format_args!("invalid verified request-context configuration: {error}"),
            1,
        );
    }
    if isolation.identity_bootstrap_pending() {
        info!(
            "Identity policy is empty: only a signer-backed current-envelope System identity bootstrap is admitted"
        );
    }
    isolation
}

/// SIGTERM (a supervisor / `kill` / agent-utilities stopping the daemon) and
/// SIGINT (Ctrl-C) both fire the same graceful signal. On non-unix only Ctrl-C
/// is available.
pub fn spawn_shutdown_signal_handler(coordinator: Arc<crate::transport::ShutdownCoordinator>) {
    tokio::spawn(async move {
        if wait_for_shutdown_signal().await {
            coordinator.trigger();
        }
    });
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() -> bool {
    use tokio::signal::unix::{signal, SignalKind};
    // Installed one after the other: a failed SIGTERM handler must leave SIGINT
    // at its default disposition.
    let mut term = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to install SIGTERM handler: {e}");
            return false;
        }
    };
    let mut int = match signal(SignalKind::interrupt()) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to install SIGINT handler: {e}");
            return false;
        }
    };
    tokio::select! {
        _ = term.recv() => info!("Received SIGTERM — graceful shutdown"),
        _ = int.recv()  => info!("Received SIGINT — graceful shutdown"),
    }
    true
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal() -> bool {
    if tokio::signal::ctrl_c().await.is_ok() {
        info!("Received Ctrl-C — graceful shutdown");
    }
    true
}

/// Optional reference-counted idle shutdown (CONCEPT:MEDULLA-KG.backend.tiny-shared).
/// `0` means no watcher: the engine is long-living and never self-terminates.
pub fn spawn_idle_shutdown_watcher(
    coordinator: &Arc<crate::transport::ShutdownCoordinator>,
    secs: u64,
) {
    if secs == 0 {
        info!("Idle shutdown disabled (persistent mode): engine stays up while idle");
        return;
    }
    info!(
        "Idle shutdown ARMED: will self-terminate after {}s with zero active connections",
        secs
    );
    let idle_coord = coordinator.clone();
    tokio::spawn(async move {
        crate::transport::run_idle_watcher(idle_coord, secs).await;
    });
}

/// Where and how the engine listens.
pub struct Transports {
    pub tcp_only: bool,
    pub socket_path: String,
    pub socket_mode: u32,
    pub tcp_addr: Option<String>,
    pub tcp_tls: Option<crate::transport::PreparedTcpTls>,
}

/// UDS is the primary transport on unix, with an optional secondary TCP
/// listener. Returns when the shutdown signal fires.
///
/// The bind is split from the accept loop precisely so the post-bind
/// maintenance (matview reload, time-series startup reconciliation —
/// MEDULLA-DURABLE-KERNEL-R006.4) starts only once the listener is up: it must
/// run in the background AFTER serving begins, never before it.
#[cfg(unix)]
pub async fn serve_transports(
    transports: Transports,
    state: &SharedState,
    shutdown: &Arc<crate::transport::ShutdownCoordinator>,
) -> Result<(), BoxError> {
    if transports.tcp_only {
        let addr = transports
            .tcp_addr
            .ok_or("TCP-only mode requires --tcp-addr")?;
        let listener = crate::transport::bind_tcp(&addr, &transports.tcp_tls).await?;
        medulla_h_admin::boot_maintenance::spawn_post_bind_maintenance(state.clone()).await;
        crate::transport::serve_tcp_listener(
            listener,
            state.clone(),
            shutdown.clone(),
            transports.tcp_tls,
        )
        .await?;
        return Ok(());
    }
    if let Some(addr) = transports.tcp_addr {
        let tcp_state = state.clone();
        let tcp_shutdown = shutdown.clone();
        let tls = transports.tcp_tls;
        tokio::spawn(async move {
            if let Err(e) = crate::transport::serve_tcp(&addr, tcp_state, tcp_shutdown, tls).await {
                tracing::error!("TCP server error ({:?})", e.kind());
            }
        });
    }
    let listener = crate::transport::bind_uds(&transports.socket_path, transports.socket_mode).await?;
    medulla_h_admin::boot_maintenance::spawn_post_bind_maintenance(state.clone()).await;
    crate::transport::serve_uds_listener(listener, state.clone(), shutdown.clone()).await?;
    Ok(())
}

/// Non-unix (Windows): Tokio has no UnixListener, so TCP loopback is the
/// per-platform DEFAULT transport — an explicit --tcp-addr wins. The socket
/// path/mode are still resolved+validated for config/lock parity & logging.
#[cfg(not(unix))]
pub async fn serve_transports(
    transports: Transports,
    state: &SharedState,
    shutdown: &Arc<crate::transport::ShutdownCoordinator>,
) -> Result<(), BoxError> {
    let _ = (
        &transports.socket_path,
        transports.socket_mode,
        transports.tcp_only,
    );
    let addr = transports
        .tcp_addr
        .unwrap_or_else(|| "127.0.0.1:8765".to_string());
    info!("AF_UNIX unavailable; using the configured native TCP transport");
    let listener = crate::transport::bind_tcp(&addr, &transports.tcp_tls).await?;
    medulla_h_admin::boot_maintenance::spawn_post_bind_maintenance(state.clone()).await;
    crate::transport::serve_tcp_listener(
        listener,
        state.clone(),
        shutdown.clone(),
        transports.tcp_tls,
    )
    .await?;
    Ok(())
}

/// Read `variable`: absent means `default`; a present value must satisfy
/// `parse` (after trimming) or startup exits with `invalid`.
pub fn env_or_exit<T>(
    variable: &str,
    default: T,
    parse: impl FnOnce(&str) -> Option<T>,
    invalid: &str,
) -> T {
    match std::env::var(variable) {
        Ok(value) => parse(value.trim()).unwrap_or_else(|| exit_with(invalid, 2)),
        Err(std::env::VarError::NotPresent) => default,
        Err(std::env::VarError::NotUnicode(_)) => {
            exit_with(format_args!("{variable} is not valid Unicode"), 2)
        }
    }
}

/// The per-graph resident node cap: auto-sized from effective RAM; an explicit
/// override must be positive and may only lower it.
pub fn max_nodes_per_graph(capacity: &medulla_kernel::autosize::Capacity) -> usize {
    const VARIABLE: &str = "MEDULLA_MAX_NODES_PER_GRAPH";
    let automatic = capacity.node_cap();
    env_or_exit(
        VARIABLE,
        automatic,
        |value| {
            value
                .parse::<usize>()
                .ok()
                .filter(|&limit| limit > 0)
                .map(|limit| clamp_to_automatic(VARIABLE, limit, automatic))
        },
        "MEDULLA_MAX_NODES_PER_GRAPH must be positive",
    )
}

/// Seconds from `variable`, when set to a positive integer; otherwise off (`0`).
pub fn optional_sweep_secs(variable: &str) -> u64 {
    std::env::var(variable)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

pub async fn memcap_tick(state: SharedState, max_nodes_per_graph: usize) {
    let evicted = medulla_kernel::persist::evict_oversized_all(&state, max_nodes_per_graph).await;
    if evicted > 0 {
        tracing::info!("Memory cap: evicted {} LRU node(s) over cap", evicted);
    }
}

#[cfg(feature = "cost")]
pub async fn budget_tick(state: SharedState, config: medulla_persistence::cost::CostConfig) {
    let (evicted, hibernated) = medulla_persistence::cost::enforce_memory_budgets(&state, config).await;
    if evicted > 0 || hibernated > 0 {
        tracing::info!(
            "Memory budget: evicted {} node(s), hibernated {} graph(s) to keep \
                 tenants under budget",
            evicted,
            hibernated
        );
    }
}

pub async fn sealed_expiry_tick(
    state: SharedState,
    policy: Vec<crate::sealed_retention::Retention>,
) {
    let now_ms = medulla_server_api::txn::now_ms();
    let n = crate::sealed_retention::expire_sealed_records(&state, &policy, now_ms).await;
    if n > 0 {
        tracing::info!("Sealed records: retired {} expired record(s) (EH-558)", n);
    }
}

pub async fn registry_reap_tick(state: SharedState) {
    let now_ms = medulla_server_api::txn::now_ms();
    let n = crate::registry_reaper::reap_expired_servers(&state, now_ms).await;
    if n > 0 {
        tracing::info!("Server registry: reaped {} expired :Server lease(s)", n);
    }
}

#[cfg(feature = "security")]
pub async fn provenance_anchor_tick(state: SharedState) {
    let anchored = medulla_persistence::provenance_anchor::sweep(&state).await;
    if anchored > 0 {
        tracing::info!(
            "Provenance anchoring: anchored {} graph(s) this tick",
            anchored
        );
    }
}

#[cfg(feature = "redb")]
pub async fn storage_scrub_tick(state: SharedState) {
    let Some(persistence) = state.read().await.persistence.clone() else {
        return;
    };
    match medulla_persistence::storage_scrub::run_step(persistence.as_ref()).await {
        Ok(0) => {}
        Ok(found) => tracing::error!(
            "Storage scrub: {} unreadable node row(s) found this pass (named above)",
            found
        ),
        Err(error) => tracing::warn!(%error, "storage scrub: pass failed; will retry next tick"),
    }
}

pub async fn txn_ttl_tick(state: SharedState, ttl: u64) {
    let now = medulla_server_api::txn::now_ms();
    let reclaimed = medulla_server_api::txn::sweep_expired_txns(&state, ttl, now);
    if reclaimed > 0 {
        tracing::info!(
            "Txn TTL sweep: rolled back {} idle transaction(s)",
            reclaimed
        );
    }
}

/// Narrow opt-in consumed from Arbora's local admission mode (MEDULLA-IDENTITY-R003.1).
/// Never infer anonymous authority from an absent secret.
pub fn validate_auth_mode(args: &ServerArgs, socket_mode: u32) -> Result<(), BoxError> {
    if args.auth_mode != "none" {
        require_secret_and_persist_dir(&args.auth_secret, args.persist_dir.as_deref());
        return Ok(());
    }
    if !args.auth_secret.is_empty() || args.persist_dir.is_none() {
        return Err(
            "anonymous mode requires a durable directory and forbids an auth secret".into(),
        );
    }
    if socket_mode != 0o600
        || args.tcp_addr.as_deref().is_some_and(|addr| {
            !addr
                .parse::<std::net::SocketAddr>()
                .is_ok_and(|address| address.ip().is_loopback())
        })
    {
        return Err("anonymous mode requires owner-only UDS and literal loopback TCP".into());
    }
    if [
        args.metrics_addr.as_ref(),
        args.sparql_addr.as_ref(),
        args.policy_export_addr.as_ref(),
        args.graphql_addr.as_ref(),
        args.obs_addr.as_ref(),
        args.viz_interactive_addr.as_ref(),
        args.iceberg_addr.as_ref(),
        args.federated_addr.as_ref(),
        args.tcp_tls_cert.as_ref(),
        args.tcp_tls_key.as_ref(),
        args.tcp_tls_client_ca.as_ref(),
    ]
    .into_iter()
    .any(|value| value.is_some())
    {
        return Err(
            "anonymous mode forbids auxiliary listeners and TLS identity configuration".into(),
        );
    }
    for (name, value) in std::env::vars() {
        if anonymous_env_conflicts(&name, &value) {
            return Err(format!("anonymous mode conflicts with {name}").into());
        }
    }
    Ok(())
}

fn anonymous_env_conflicts(name: &str, value: &str) -> bool {
    !value.is_empty()
        && ((name.starts_with("MEDULLA_")
            && (name.ends_with("_ADDR")
                || name.contains("RAFT_")
                || name.contains("FEDERATION_")
                || name.contains("OIDC_")
                || name.contains("SIGNER")))
            || name.starts_with("OIDC_"))
}

/// Called while holding the existing engine.lock and before opening any stores.
/// A mode marker is not an authentication credential. It prevents accidental
/// reuse of protected data under anonymous authority. No implicit conversions.
pub fn prepare_auth_store(mode: &str, dir: &str) -> Result<(), BoxError> {
    use std::io::Write;
    let root = std::path::Path::new(dir);
    let marker = root.join("anonymous-store-v1");
    match std::fs::symlink_metadata(&marker) {
        Ok(meta) => {
            if mode != "none"
                || !meta.file_type().is_file()
                || std::fs::read(&marker)? != b"medulla anonymous local store v1\n"
            {
                return Err(
                    "store authentication mode conflict or invalid anonymous marker".into(),
                );
            }
            return Ok(());
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    if mode != "none" {
        return Ok(());
    }
    for entry in std::fs::read_dir(root)? {
        if entry?.file_name() != "engine.lock" {
            return Err(
                "anonymous mode refuses an existing unmarked store; select a fresh directory"
                    .into(),
            );
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)?;
    file.write_all(b"medulla anonymous local store v1\n")?;
    file.sync_all()?;
    std::fs::File::open(root)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod anonymous_store_tests {
    use super::{anonymous_env_conflicts, prepare_auth_store};

    #[test]
    fn anonymous_environment_conflicts_preserve_exact_prefix_and_value_rules() {
        for (name, conflicts) in [
            ("MEDULLA_PGWIRE_ADDR", true),
            ("MEDULLA_RAFT_NODE_ID", true),
            ("MEDULLA_FEDERATION_TOKEN", true),
            ("MEDULLA_OIDC_ISSUER", true),
            ("MEDULLA_SIGNER_KEY", true),
            ("OIDC_ISSUER", true),
            ("MEDULLA_OTHER", false),
            ("OTHER_RAFT_NODE_ID", false),
            ("OTHER_ADDR", false),
            ("GRAPH_SERVICE_AUTH_SECRET", false),
        ] {
            assert_eq!(anonymous_env_conflicts(name, "value"), conflicts, "{name}");
            assert_eq!(anonymous_env_conflicts(name, " "), conflicts, "{name}");
            assert!(!anonymous_env_conflicts(name, ""), "{name}");
        }
    }

    #[test]
    fn anonymous_store_mode_is_explicit_and_restart_stable() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        prepare_auth_store("none", root).unwrap();
        std::fs::write(dir.path().join("real-engine-state"), b"data").unwrap();
        prepare_auth_store("none", root).unwrap();
        assert!(prepare_auth_store("authenticated", root).is_err());
    }

    #[test]
    fn anonymous_refuses_preexisting_unmarked_state() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("protected-data"), b"private").unwrap();
        assert!(prepare_auth_store("none", dir.path().to_str().unwrap()).is_err());
        assert!(!dir.path().join("anonymous-store-v1").exists());
        prepare_auth_store("authenticated", dir.path().to_str().unwrap()).unwrap();
    }

    #[test]
    fn anonymous_refuses_invalid_marker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("anonymous-store-v1"), b"wrong").unwrap();
        assert!(prepare_auth_store("none", dir.path().to_str().unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn anonymous_refuses_symlink_marker() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, b"medulla anonymous local store v1\n").unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("anonymous-store-v1")).unwrap();
        assert!(prepare_auth_store("none", dir.path().to_str().unwrap()).is_err());
    }
}
