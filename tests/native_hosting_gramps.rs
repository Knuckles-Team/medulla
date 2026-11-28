//! Live P0 to P2 of the native-hosting pilot on the synthetic family-tree fixture
//! (MEDULLA-UNIFIED-DATA-PLANE-R026.2.2).
//!
//! The test is a client only: it starts the `medulla-server` binary, imports the tree through
//! the served `AddTriples`, exports it with `GetRdf`, and runs the 14-statement corpus through
//! the pgwire listener as an external client. It links no handler crate and reads no other
//! repository's corpus. P0 records the digest of the export; P1 runs the corpus on a real
//! PostgreSQL server (`MEDULLA_CONFORMANCE_POSTGRES_DSN`; the test prints why it stops when the
//! variable is unset); P2 replays the same statements unmodified on the engine and lists every
//! statement whose outcome differs. Each phase writes `conformance/native_hosting/<phase>.json`.

#![cfg(all(
    unix,
    feature = "pgwire",
    feature = "rdf",
    feature = "obda",
    feature = "federation-sql"
))]

mod common;
#[path = "support/family_tree.rs"]
mod family_tree;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_catalog_types::gramps_baseline_digest::GrampsBaselineDigest;
use medulla_catalog_types::schema_context::{
    SchemaContextAnswer, SchemaContextIntent, SCHEMA_CONTEXT_UNMAPPED,
};
use medulla_protocol_types::protocol::{GraphType, Method, Request, ResultPayload};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use {
    medulla_catalog_source_types::gramps_pilot_phase::GrampsPilotMilestone,
    medulla_catalog_source_types::gramps_pilot_phase::PhaseExitArtifact,
};

use family_tree::{
    family_tree, family_tree_corpus, table_digest, PERSON_MAPPING, PHASE_EXIT_INVALID,
};

const AUTH_SECRET: &str = "test";
const ADMIN: &str = "gramps-pilot";
const ADMIN_SIGNER_KEY: &str = "gramps-pilot-signing-key";
const REVIEWER: &str = "native-hosting-test";
const POSTGRES_DSN_ENV: &str = "MEDULLA_CONFORMANCE_POSTGRES_DSN";

fn server_bin() -> std::path::PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_medulla-server") {
        return path.into();
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent()
        .and_then(std::path::Path::parent)
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("medulla-server")
}

/// The pgwire password of the pilot identity: derived from the served secret, never printed
/// and never written to a log or an artifact.
fn pgwire_password() -> String {
    medulla_wire::pgwire::derive_pg_password(AUTH_SECRET, ADMIN)
}

/// The DSN of the engine's own pgwire listener as a source (P5): the pilot identity and its
/// pgwire credentials against `graph`. It carries a secret, so callers never format it into a
/// message.
fn pgwire_dsn(pg_addr: &str, graph: &str) -> String {
    let scheme = "postgres";
    format!("{scheme}://{ADMIN}:{}@{pg_addr}/{graph}", pgwire_password())
}

/// One running `medulla-server` with its native socket and pgwire listener. It exports the
/// pgwire credentials (`pgwire_dsn`), the private backup root of the `Backup` and `Restore`
/// methods and the exact `MEDULLA_FEDERATION_ALLOW` entry that lets the engine's own listener
/// pass the outbound SSRF check as a source (loopback is a sensitive range).
struct RunningServer {
    child: std::process::Child,
    socket: std::path::PathBuf,
    pg_addr: String,
    backup_root: std::path::PathBuf,
}

impl RunningServer {
    /// Start the server over `persist` (a data directory), keeping its socket, log and backup
    /// root under `dir`.
    async fn start(dir: &std::path::Path, persist: &std::path::Path, pg_addr: &str) -> Self {
        common::configure_authority();
        let socket = dir.join("native.sock");
        let _ = std::fs::remove_file(&socket);
        let log = std::fs::File::options()
            .create(true)
            .append(true)
            .open(dir.join("server.log"))
            .expect("server log");
        let mut signers = serde_json::Map::new();
        signers.insert(
            ADMIN.to_string(),
            json!({"key": ADMIN_SIGNER_KEY, "allowed_roles": [], "may_grant_system": true}),
        );
        let signers = Value::Object(signers);
        let backup_root = dir.join("backups");
        std::fs::create_dir_all(&backup_root).expect("backup root");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&backup_root, std::fs::Permissions::from_mode(0o700))
                .expect("private backup root");
        }
        let mut command = std::process::Command::new(server_bin());
        command
            .arg("--socket-path")
            .arg(&socket)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("GRAPH_SERVICE_AUTH_SECRET", AUTH_SECRET)
            .env("GRAPH_SERVICE_PERSIST_DIR", persist)
            .env("MEDULLA_SIGNER_KEYS_JSON", signers.to_string())
            .env(medulla_wire::pgwire::PGWIRE_ADDR_ENV, pg_addr)
            .env("MEDULLA_FEDERATION_ALLOW", pg_addr)
            .env("MEDULLA_BACKUP_ROOT", &backup_root)
            .stdout(log.try_clone().expect("log handle"))
            .stderr(log);
        for name in [
            "MEDULLA_REQUIRE_OIDC",
            "MEDULLA_AUDIENCE",
            "MEDULLA_TENANT",
            "MEDULLA_POLICY_VERSION",
            "MEDULLA_SECURITY_STATE_DIR",
        ] {
            command.env(
                name,
                std::env::var(name).expect("authority env is configured"),
            );
        }
        std::fs::create_dir_all(persist).expect("persist dir");
        let child = command.spawn().expect("spawn medulla-server");
        let mut server = Self {
            child,
            socket,
            pg_addr: pg_addr.to_string(),
            backup_root,
        };
        server.wait_ready(pg_addr).await;
        server
    }

    async fn wait_ready(&mut self, pg_addr: &str) {
        for _ in 0..1500 {
            if let Some(status) = self.child.try_wait().expect("poll server") {
                panic!("medulla-server exited before it listened: {status}");
            }
            let native = tokio::net::UnixStream::connect(&self.socket).await.is_ok();
            let wire = tokio::net::TcpStream::connect(pg_addr).await.is_ok();
            if native && wire {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("medulla-server did not open its native socket and pgwire listener");
    }

    /// SIGKILL (`Child::kill`), then wait for the process to exit: the process gets no chance
    /// to finish a commit it has started.
    fn kill(mut self) {
        self.child.kill().expect("SIGKILL the server");
        let _ = self.child.wait().expect("server exit");
    }

    /// SIGTERM, then wait for the process to exit.
    fn stop(mut self) {
        let status = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(self.child.id().to_string())
            .status()
            .expect("kill");
        assert!(status.success(), "SIGTERM the server");
        let _ = self.child.wait().expect("server exit");
    }
}

/// A native-socket client that signs every request the way a served caller does.
struct Native {
    stream: tokio::net::UnixStream,
    next_id: u64,
    sequence: u64,
}

impl Native {
    async fn connect(socket: &std::path::Path) -> Self {
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_micros() as u64;
        Self {
            stream: tokio::net::UnixStream::connect(socket)
                .await
                .expect("native connect"),
            next_id: epoch,
            sequence: 0,
        }
    }

    fn context(scopes: &[&str]) -> medulla_identity_core_types::acl::RequestContextClaims {
        let mut claims = common::context_for(ADMIN);
        claims.scopes = scopes.iter().map(|scope| scope.to_string()).collect();
        claims
    }

    async fn call(
        &mut self,
        graph: &str,
        context: &medulla_identity_core_types::acl::RequestContextClaims,
        method: Method,
        signer: Option<&str>,
    ) -> medulla_protocol_types::protocol::Response {
        self.send(graph, context, method, signer).await;
        self.recv().await
    }

    /// Sign and send one request without waiting for its reply.
    async fn send(
        &mut self,
        graph: &str,
        context: &medulla_identity_core_types::acl::RequestContextClaims,
        method: Method,
        signer: Option<&str>,
    ) {
        use hmac::Mac as _;
        self.next_id += 1;
        self.sequence += 1;
        let id = self.next_id;
        let nonce = format!("gramps-{id}-{}", self.sequence);
        let idempotency_key = format!("gramps-request-{id}-{}", self.sequence);
        let method = match (method, signer) {
            (
                Method::RegisterIdentity {
                    agent_id,
                    role,
                    teams,
                    roles,
                    ..
                },
                Some(signer),
            ) => {
                let unsigned = Method::RegisterIdentity {
                    agent_id: agent_id.clone(),
                    role: role.clone(),
                    teams: teams.clone(),
                    signature: String::new(),
                    roles: roles.clone(),
                };
                let bytes =
                    medulla_protocol_types::protocol::build_context_operation_signature_bytes(
                        "eg-register-identity-v2",
                        context,
                        &idempotency_key,
                        graph,
                        &unsigned.canonical_body_bytes(),
                    );
                let digest = Sha256::digest(bytes);
                let mut mac = hmac::Hmac::<Sha256>::new_from_slice(ADMIN_SIGNER_KEY.as_bytes())
                    .expect("signing key");
                mac.update(&digest);
                Method::RegisterIdentity {
                    agent_id,
                    role,
                    teams,
                    signature: format!("{signer}:{}", hex::encode(mac.finalize().into_bytes())),
                    roles,
                }
            }
            (method, _) => method,
        };
        let mut request = Request {
            id,
            graph: graph.to_string(),
            auth_token: String::new(),
            agent_id: Some(context.agent_id.clone()),
            method,
            trace_id: None,
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs();
        request.auth_token = medulla_server_auth::auth::compute_verified_envelope_token(
            AUTH_SECRET,
            &request,
            &medulla_server_auth::auth::VerifiedEnvelopeParams {
                context,
                timestamp,
                nonce: &nonce,
                idempotency_key: &idempotency_key,
            },
        );
        let body = rmp_serde::to_vec_named(&request).expect("encode request");
        self.stream
            .write_all(&(body.len() as u32).to_be_bytes())
            .await
            .expect("send length");
        self.stream.write_all(&body).await.expect("send body");
    }

    /// Read one reply.
    async fn recv(&mut self) -> medulla_protocol_types::protocol::Response {
        let mut length = [0u8; 4];
        self.stream
            .read_exact(&mut length)
            .await
            .expect("response length");
        let mut reply = vec![0u8; u32::from_be_bytes(length) as usize];
        self.stream
            .read_exact(&mut reply)
            .await
            .expect("response body");
        rmp_serde::from_slice(&reply).expect("decode response")
    }

    async fn ok(
        &mut self,
        graph: &str,
        context: &medulla_identity_core_types::acl::RequestContextClaims,
        method: Method,
    ) -> Option<ResultPayload> {
        let response = self.call(graph, context, method, None).await;
        assert!(
            response.error.is_none(),
            "served method refused: {response:?}"
        );
        response.result
    }
}

/// Create `graph`, import the tree in batches of 500 lines through `AddTriples`, and read the
/// graph back with `GetRdf`.
async fn import_and_export(native: &mut Native, graph: &str) -> String {
    let context = Native::context(&["*"]);
    native
        .ok(
            graph,
            &context,
            Method::CreateGraph {
                graph_name: graph.to_string(),
                graph_type: GraphType::Global,
            },
        )
        .await;
    for batch in family_tree().ntriples_batches(500) {
        native
            .ok(
                graph,
                &context,
                Method::AddTriples {
                    turtle: String::new(),
                    ntriples: batch,
                },
            )
            .await;
    }
    match native.ok(graph, &context, Method::GetRdf).await {
        Some(ResultPayload::Raw(bytes)) => {
            rmp_serde::from_slice::<String>(&bytes).expect("GetRdf answers one string")
        }
        other => panic!("GetRdf answered {other:?}"),
    }
}

fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// One statement's record: `{index, sql_sha256, sqlstate, result_sha256}` with a 1-based index.
fn record(index: usize, sql: &str, sqlstate: &str, rows: &str) -> Value {
    json!({
        "index": index,
        "sql_sha256": sha256_hex(sql),
        "sqlstate": sqlstate,
        "result_sha256": if rows.is_empty() { String::new() } else { sha256_hex(rows) },
    })
}

/// Run `statements` in order on `client`; a statement that fails is recorded with its SQLSTATE.
async fn run_corpus(client: &tokio_postgres::Client, statements: &[String]) -> Vec<Value> {
    use tokio_postgres::SimpleQueryMessage;
    let mut records = Vec::new();
    for (position, sql) in statements.iter().enumerate() {
        let index = position + 1;
        match client.simple_query(sql).await {
            Ok(messages) => {
                let lines: Vec<String> = messages
                    .iter()
                    .filter_map(|message| match message {
                        SimpleQueryMessage::Row(row) => Some(
                            (0..row.len())
                                .map(|column| row.get(column).unwrap_or("").to_string())
                                .collect::<Vec<_>>()
                                .join("\t"),
                        ),
                        _ => None,
                    })
                    .collect();
                records.push(record(index, sql, "00000", &lines.join("\n")));
            }
            Err(error) => {
                let state = error
                    .code()
                    .map(|code| code.code().to_string())
                    .unwrap_or_else(|| "XX000".to_string());
                records.push(record(index, sql, &state, ""));
            }
        }
    }
    records
}

/// A phase whose exit is invalid stops the gate with `PHASE_EXIT_INVALID`.
fn valid_exit(milestone: GrampsPilotMilestone) -> PhaseExitArtifact {
    let artifact = PhaseExitArtifact {
        closes: milestone,
        artifact_kind: milestone.required_artifact_kind().to_string(),
        reviewed_by: REVIEWER.to_string(),
    };
    if let Err(error) = artifact.validate() {
        panic!("{PHASE_EXIT_INVALID}: {error}");
    }
    artifact
}

fn write_artifact(phase: &str, body: &Value) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/native_hosting");
    std::fs::create_dir_all(&dir).expect("artifact directory");
    let text = serde_json::to_string_pretty(body).expect("artifact json");
    std::fs::write(dir.join(format!("{phase}.json")), text).expect("write artifact");
}

fn differences(p1: &[Value], p2: &[Value]) -> Vec<Value> {
    p1.iter()
        .zip(p2)
        .filter(|(a, b)| a["sqlstate"] != b["sqlstate"] || a["result_sha256"] != b["result_sha256"])
        .map(|(a, b)| {
            json!({
                "index": a["index"],
                "p1": {"sqlstate": a["sqlstate"], "result_sha256": a["result_sha256"]},
                "p2": {"sqlstate": b["sqlstate"], "result_sha256": b["result_sha256"]},
            })
        })
        .collect()
}

// proves R026.2.2: Live P0 to P2 on the synthetic family-tree fixture
// spec: MEDULLA-UNIFIED-DATA-PLANE-R026.2.2
#[tokio::test(flavor = "multi_thread")]
async fn p0_p1_p2_exit_artifacts() {
    let dir = test_support::fresh_dir("native-hosting-gramps");
    let pg_addr = test_support::ephemeral_listener_addr().await;
    let server = RunningServer::start(&dir, &dir.join("persist"), &pg_addr).await;

    // The one System identity every session acts as, registered through the bootstrap gate.
    let mut native = Native::connect(&server.socket).await;
    let bootstrap = Native::context(&["security:bootstrap"]);
    let registered = native
        .call(
            "__commons__",
            &bootstrap,
            Method::RegisterIdentity {
                agent_id: ADMIN.to_string(),
                role: medulla_identity_core_types::acl::AgentRole::System,
                teams: Vec::new(),
                signature: String::new(),
                roles: Vec::new(),
            },
            Some(ADMIN),
        )
        .await;
    assert!(
        registered.error.is_none(),
        "bootstrap identity: {registered:?}"
    );

    // P0: import the tree, export it, record the digest; two fresh graphs agree.
    let export = import_and_export(&mut native, "gramps-p0-a").await;
    let again = import_and_export(&mut native, "gramps-p0-b").await;
    assert_eq!(
        export.lines().count(),
        1_019,
        "the export holds every triple"
    );
    assert_eq!(sha256_hex(&export), sha256_hex(&again), "two imports agree");
    let baseline = GrampsBaselineDigest {
        record_count: (family_tree::PEOPLE + family_tree::FAMILIES) as u64,
        digest_hex: sha256_hex(&export),
    };
    let p0_exit = baseline.to_phase_exit(REVIEWER).unwrap_or_else(|error| {
        panic!("{PHASE_EXIT_INVALID}: {error}");
    });
    write_artifact(
        "p0",
        &json!({"phase_exit": p0_exit, "baseline": baseline, "triples": export.lines().count()}),
    );

    // P1 needs the real PostgreSQL control; say why the gate stops when there is none.
    let Ok(dsn) = std::env::var(POSTGRES_DSN_ENV) else {
        eprintln!(
            "native hosting P1 and P2 skipped: {POSTGRES_DSN_ENV} is not set (a postgres:16 \
             control is required)"
        );
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
        return;
    };
    let corpus = family_tree_corpus(&family_tree());
    let (control, connection) = tokio_postgres::connect(&dsn, tokio_postgres::NoTls)
        .await
        .expect("connect to the PostgreSQL control");
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let p1 = run_corpus(&control, &corpus).await;
    assert_eq!(p1.len(), 14);
    write_artifact(
        "p1",
        &json!({
            "phase_exit": valid_exit(GrampsPilotMilestone::P1PostgresControl),
            "statements": p1,
        }),
    );

    // P2: the same statements, unmodified, through the engine's pgwire listener.
    native
        .ok(
            "gramps-p2",
            &Native::context(&["*"]),
            Method::CreateGraph {
                graph_name: "gramps-p2".to_string(),
                graph_type: GraphType::Global,
            },
        )
        .await;
    let engine = test_support::connect_pgwire(&pg_addr, AUTH_SECRET, ADMIN, "gramps-p2")
        .await
        .expect("pgwire connect");
    let p2 = run_corpus(&engine, &corpus).await;
    assert_eq!(p2.len(), 14);
    let differing = differences(&p1, &p2);
    assert!(
        !differing.is_empty(),
        "statement 14 differs at least by its SQLSTATE: the difference list is never empty"
    );
    write_artifact(
        "p2",
        &json!({
            "phase_exit": valid_exit(GrampsPilotMilestone::P2UnmodifiedReplay),
            "statements": p2,
            "differences": differing,
        }),
    );

    drop(native);
    server.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- P4 restore and P5 schema understanding (MEDULLA-UNIFIED-DATA-PLANE-R026.3.2) ----

/// The graph that holds the tables (the pgwire database name) and the source DSN's database.
const TABLE_GRAPH: &str = "medulla";
/// The graph that carries the P0 import whose export is the graph digest.
const BASELINE_GRAPH: &str = "gramps-p4-base";
/// The attached source of P5: the engine's own pgwire listener.
const SOURCE_ID: &str = "gramps_native";

/// The pilot identity must be able to act on a fresh engine. The identity registry lives in the
/// shared security state, so a probe comes first and a registration only when it is refused.
async fn ensure_admin(native: &mut Native) {
    let all = Native::context(&["*"]);
    let probe = native.call("__commons__", &all, Method::GetRdf, None).await;
    if probe.error.is_some() {
        let registered = native
            .call(
                "__commons__",
                &Native::context(&["security:bootstrap"]),
                Method::RegisterIdentity {
                    agent_id: ADMIN.to_string(),
                    role: medulla_identity_core_types::acl::AgentRole::System,
                    teams: Vec::new(),
                    signature: String::new(),
                    roles: Vec::new(),
                },
                Some(ADMIN),
            )
            .await;
        assert!(
            registered.error.is_none(),
            "bootstrap identity: {registered:?}"
        );
    }
    // The table graph exists on a fresh engine and already on a restored one.
    let _ = native
        .call(
            TABLE_GRAPH,
            &all,
            Method::CreateGraph {
                graph_name: TABLE_GRAPH.to_string(),
                graph_type: GraphType::Global,
            },
            None,
        )
        .await;
}

async fn pg(server: &RunningServer, graph: &str) -> tokio_postgres::Client {
    test_support::connect_pgwire(&server.pg_addr, AUTH_SECRET, ADMIN, graph)
        .await
        .expect("pgwire connect")
}

/// The tab-separated text lines of one query.
async fn text_rows(client: &tokio_postgres::Client, sql: &str) -> Vec<String> {
    use tokio_postgres::SimpleQueryMessage;
    client
        .simple_query(sql)
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"))
        .iter()
        .filter_map(|message| match message {
            SimpleQueryMessage::Row(row) => Some(
                (0..row.len())
                    .map(|column| row.get(column).unwrap_or("").to_string())
                    .collect::<Vec<_>>()
                    .join("\t"),
            ),
            _ => None,
        })
        .collect()
}

/// The table digest of the engine's current state.
async fn tables_digest(client: &tokio_postgres::Client) -> GrampsBaselineDigest {
    let person = text_rows(client, "SELECT * FROM person ORDER BY handle").await;
    let family = text_rows(client, "SELECT * FROM family ORDER BY handle").await;
    let child_ref = text_rows(client, "SELECT * FROM child_ref ORDER BY family, child").await;
    table_digest(&person, &family, &child_ref)
}

/// The graph digest: SHA-256 of the exact `GetRdf` string of `graph`.
async fn graph_digest(native: &mut Native, graph: &str) -> String {
    match native
        .ok(graph, &Native::context(&["*"]), Method::GetRdf)
        .await
    {
        Some(ResultPayload::Raw(bytes)) => {
            sha256_hex(&rmp_serde::from_slice::<String>(&bytes).expect("GetRdf answers one string"))
        }
        other => panic!("GetRdf answered {other:?}"),
    }
}

/// The engine state every restore path starts from: the P0 import and corpus statements 1 to 6.
async fn load_baseline(native: &mut Native, server: &RunningServer) {
    ensure_admin(native).await;
    import_and_export(native, BASELINE_GRAPH).await;
    let client = pg(server, TABLE_GRAPH).await;
    for sql in &family_tree_corpus(&family_tree())[..6] {
        client
            .simple_query(sql)
            .await
            .unwrap_or_else(|error| panic!("baseline statement: {error}"));
    }
}

/// Start the server on a fresh directory under `root`; `name` keeps the directories apart.
async fn fresh_engine(root: &std::path::Path, name: &str) -> (RunningServer, std::path::PathBuf) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("engine dir");
    let persist = dir.join("persist");
    let addr = test_support::ephemeral_listener_addr().await;
    (RunningServer::start(&dir, &persist, &addr).await, persist)
}

/// Both digests of `server`, compared with the ones taken before the restore.
async fn assert_restored(
    server: &RunningServer,
    graph_before: &str,
    tables_before: &GrampsBaselineDigest,
    check_graph: bool,
    path: &str,
) {
    let mut native = Native::connect(&server.socket).await;
    ensure_admin(&mut native).await;
    let client = pg(server, TABLE_GRAPH).await;
    assert_eq!(
        &tables_digest(&client).await,
        tables_before,
        "{path}: table digest"
    );
    if check_graph {
        assert_eq!(
            graph_digest(&mut native, BASELINE_GRAPH).await,
            graph_before,
            "{path}: graph digest"
        );
    }
}

/// A plain value: `"4"^^xsd:integer` and `4` both read `4`.
fn plain(cell: &str) -> String {
    cell.split("^^")
        .next()
        .unwrap_or(cell)
        .trim_matches('"')
        .to_string()
}

// proves R026.3.2: Live P4 restore and P5 schema understanding
// spec: MEDULLA-UNIFIED-DATA-PLANE-R026.3.2
#[tokio::test(flavor = "multi_thread")]
async fn p4_restore_and_p5_schema() {
    use futures_util::{SinkExt, TryStreamExt};
    let root = test_support::fresh_dir("native-hosting-gramps-p4");
    let all = Native::context(&["*"]);

    // ---- Engine A: the baseline, path one (native Backup and Restore), then P5.
    let (engine_a, persist_a) = fresh_engine(&root, "a").await;
    let mut native = Native::connect(&engine_a.socket).await;
    load_baseline(&mut native, &engine_a).await;
    let graph_before = graph_digest(&mut native, BASELINE_GRAPH).await;
    let client_a = pg(&engine_a, TABLE_GRAPH).await;
    let tables_before = tables_digest(&client_a).await;
    assert_eq!(tables_before.record_count, 239, "120 + 60 + 59 rows");
    let _ = persist_a;

    native
        .ok(
            "__commons__",
            &all,
            Method::Backup {
                destination: "gramps-p4".to_string(),
                label: Some("p4".to_string()),
            },
        )
        .await;
    let bundle = engine_a.backup_root.join("gramps-p4");
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(bundle.join("MANIFEST.json")).expect("bundle manifest"),
    )
    .expect("manifest json");
    let shards = manifest["shard_count"].as_u64().expect("shard_count") as usize;
    match native
        .ok(
            "__commons__",
            &all,
            Method::Restore {
                source: "gramps-p4".to_string(),
                target_shards: shards,
            },
        )
        .await
    {
        Some(ResultPayload::Json(report)) => {
            assert!(report.is_object(), "the Restore report parses as an object")
        }
        other => panic!("Restore answered {other:?}"),
    }

    // P5 on the same engine: (a) the source, (b) the mapping, (c) the schema context.
    let context = Native::context(&["*"]);
    native
        .ok(
            TABLE_GRAPH,
            &context,
            Method::RegisterForeignSource {
                name: SOURCE_ID.to_string(),
                source: medulla_query_types::wire::ForeignSourceSpec::Sql {
                    dsn: pgwire_dsn(&engine_a.pg_addr, TABLE_GRAPH),
                    query: "SELECT handle FROM person".to_string(),
                    id_field: "handle".to_string(),
                    score_field: None,
                    columns: vec!["handle".to_string()],
                },
                quota: None,
                acceleration: None,
            },
        )
        .await;
    let schema_context = |intent| Method::SchemaContext {
        source_id: SOURCE_ID.to_string(),
        schema: "public".to_string(),
        object: "person".to_string(),
        intent,
    };
    // The catalog probe runs after registration. The engine answers once it has settled
    // (MEDULLA-DURABLE-KERNEL-R878), so the test neither sleeps nor polls.
    let settled = native
        .ok(
            TABLE_GRAPH,
            &context,
            Method::AwaitWatermark {
                resource: format!("catalog:{SOURCE_ID}"),
                at_least: 1,
                deadline_ms: 30_000,
            },
        )
        .await;
    let settled: medulla_catalog_source_types::await_watermark::WatermarkAwaited = match settled {
        Some(ResultPayload::Raw(bytes)) => {
            rmp_serde::from_slice(&bytes).expect("the watermark answer decodes")
        }
        other => panic!("AwaitWatermark answered {other:?}"),
    };
    assert!(
        settled.reached,
        "the catalog probe did not settle within 30 seconds"
    );
    let probe = native
        .call(
            TABLE_GRAPH,
            &context,
            schema_context(SchemaContextIntent::Definition),
            None,
        )
        .await;
    assert!(probe.error.is_none(), "schema context: {:?}", probe.error);
    let answer = |response: medulla_protocol_types::protocol::Response| match response.result {
        Some(ResultPayload::Raw(bytes)) => {
            rmp_serde::from_slice::<SchemaContextAnswer>(&bytes).expect("schema context answer")
        }
        other => panic!("SchemaContext answered {other:?} / {:?}", response.error),
    };
    let definition = answer(
        native
            .call(
                TABLE_GRAPH,
                &context,
                schema_context(SchemaContextIntent::Definition),
                None,
            )
            .await,
    );
    let mut columns: Vec<String> = definition.columns.iter().map(|c| c.name.clone()).collect();
    columns.sort();
    assert_eq!(
        columns,
        [
            "birth_year",
            "first_name",
            "gender",
            "gid",
            "handle",
            "surname"
        ]
    );
    let joins = answer(
        native
            .call(
                TABLE_GRAPH,
                &context,
                schema_context(SchemaContextIntent::Joins),
                None,
            )
            .await,
    );
    let joined: std::collections::BTreeSet<&str> = joins
        .joins
        .iter()
        .map(|join| join.joined_table.rsplit('.').next().unwrap_or(""))
        .collect();
    assert!(
        joined.contains("family") && joined.contains("child_ref"),
        "joins: {joined:?}"
    );
    let owner = answer(
        native
            .call(
                TABLE_GRAPH,
                &context,
                schema_context(SchemaContextIntent::Owner),
                None,
            )
            .await,
    );
    assert_eq!(owner.owner, ADMIN, "the registering agent owns the source");
    let ontology = native
        .call(
            TABLE_GRAPH,
            &context,
            schema_context(SchemaContextIntent::Ontology),
            None,
        )
        .await;
    assert!(
        format!("{:?}", ontology.error).contains(SCHEMA_CONTEXT_UNMAPPED),
        "no graph view reads the table: {:?}",
        ontology.error
    );
    let impact = answer(
        native
            .call(
                TABLE_GRAPH,
                &context,
                schema_context(SchemaContextIntent::Impact),
                None,
            )
            .await,
    );
    assert_eq!(
        impact.change_impact,
        "no downstream column reads this table"
    );

    let client = pg(&engine_a, TABLE_GRAPH).await;
    let expected = text_rows(
        &client,
        "SELECT surname, count(*) FROM person GROUP BY surname ORDER BY surname",
    )
    .await;
    let sparql = native
        .ok(
            TABLE_GRAPH,
            &context,
            Method::SparqlVirtual {
                query: "PREFIX gramps: <https://example.invalid/gramps#> SELECT ?surname \
                        (COUNT(?p) AS ?n) WHERE { ?p a gramps:Person ; gramps:surname ?surname } \
                        GROUP BY ?surname ORDER BY ?surname"
                    .to_string(),
                mapping: PERSON_MAPPING.to_string(),
                tables: vec![
                    "person".to_string(),
                    "family".to_string(),
                    "child_ref".to_string(),
                ],
                external_sources: Vec::new(),
            },
        )
        .await;
    let result: medulla_protocol_types::protocol::SparqlResult = match sparql {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("sparql result"),
        other => panic!("SparqlVirtual answered {other:?}"),
    };
    let virtual_rows: Vec<String> = result
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| plain(cell.as_ref().map_or("", |term| term.value.as_str())))
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect();
    assert_eq!(
        virtual_rows, expected,
        "the virtual graph answers statement 8"
    );
    write_artifact(
        "p5",
        &json!({
            "phase_exit": valid_exit(GrampsPilotMilestone::P5SchemaUnderstanding),
            "source_id": SOURCE_ID,
            "sparql_rows": virtual_rows,
            "schema_context": {
                "columns": columns,
                "joins": joined,
                "owner": owner.owner,
                "ontology": SCHEMA_CONTEXT_UNMAPPED,
                "impact": impact.change_impact,
            },
        }),
    );

    // Path one, continued: stop A and restore the bundle in place into a new directory.
    drop((client, client_a, native));
    engine_a.stop();
    let restored_persist = root.join("restored").join("persist");
    std::fs::create_dir_all(&restored_persist).expect("restored dir");
    let status = std::process::Command::new(server_bin())
        .args(["restore", "--bundle"])
        .arg(&bundle)
        .arg("--persist-dir")
        .arg(&restored_persist)
        .args(["--shards", &shards.to_string()])
        .status()
        .expect("run the offline restore");
    assert!(status.success(), "offline restore");
    let restored_addr = test_support::ephemeral_listener_addr().await;
    let restored_dir = root.join("restored");
    let engine_r = RunningServer::start(&restored_dir, &restored_persist, &restored_addr).await;
    assert_restored(&engine_r, &graph_before, &tables_before, true, "native").await;
    engine_r.stop();

    // Path two: COPY out of a source engine, COPY in to a second fresh engine.
    let (engine_s, _) = fresh_engine(&root, "s").await;
    let mut native_s = Native::connect(&engine_s.socket).await;
    load_baseline(&mut native_s, &engine_s).await;
    let source_client = pg(&engine_s, TABLE_GRAPH).await;
    assert_eq!(
        &tables_digest(&source_client).await,
        &tables_before,
        "dump source"
    );
    let mut dumps = Vec::new();
    for table in ["person", "family", "child_ref"] {
        let stream = source_client
            .copy_out(format!("COPY {table} TO STDOUT").as_str())
            .await
            .unwrap_or_else(|error| panic!("COPY {table} TO STDOUT: {error}"));
        let chunks: Vec<bytes::Bytes> = stream.try_collect().await.expect("copy data");
        dumps.push(chunks.concat());
    }
    drop((source_client, native_s));
    engine_s.stop();
    let (engine_c, _) = fresh_engine(&root, "c").await;
    let mut native_c = Native::connect(&engine_c.socket).await;
    ensure_admin(&mut native_c).await;
    let reload = pg(&engine_c, TABLE_GRAPH).await;
    for sql in &family_tree_corpus(&family_tree())[..3] {
        reload
            .simple_query(sql)
            .await
            .unwrap_or_else(|error| panic!("dump schema statement: {error}"));
    }
    for (table, data) in ["person", "family", "child_ref"].into_iter().zip(dumps) {
        let sink = reload
            .copy_in::<_, bytes::Bytes>(format!("COPY {table} FROM STDIN").as_str())
            .await
            .unwrap_or_else(|error| panic!("COPY {table} FROM STDIN: {error}"));
        futures_util::pin_mut!(sink);
        sink.send(bytes::Bytes::from(data))
            .await
            .expect("copy rows");
        sink.finish().await.expect("copy finish");
    }
    assert_eq!(
        &tables_digest(&reload).await,
        &tables_before,
        "sql dump: table digest"
    );
    drop((reload, native_c));
    engine_c.stop();

    // Path three: SIGKILL while the last AddTriples batch of a second import is in flight.
    let (engine_k, persist_k) = fresh_engine(&root, "k").await;
    let mut native_k = Native::connect(&engine_k.socket).await;
    load_baseline(&mut native_k, &engine_k).await;
    native_k
        .ok(
            "p4-kill",
            &all,
            Method::CreateGraph {
                graph_name: "p4-kill".to_string(),
                graph_type: GraphType::Global,
            },
        )
        .await;
    let mut batches = family_tree().ntriples_batches(500);
    let last = batches.pop().expect("at least one batch");
    let previous: usize = batches.iter().map(|batch| batch.lines().count()).sum();
    let last_size = last.lines().count();
    for batch in batches {
        native_k
            .ok(
                "p4-kill",
                &all,
                Method::AddTriples {
                    turtle: String::new(),
                    ntriples: batch,
                },
            )
            .await;
    }
    let mut racing = Native::connect(&engine_k.socket).await;
    racing
        .send(
            "p4-kill",
            &all,
            Method::AddTriples {
                turtle: String::new(),
                ntriples: last.clone(),
            },
            None,
        )
        .await;
    let dir_k = root.join("k");
    let addr_k = engine_k.pg_addr.clone();
    drop((native_k, racing));
    engine_k.kill();
    let engine_k = RunningServer::start(&dir_k, &persist_k, &addr_k).await;
    let mut native_k = Native::connect(&engine_k.socket).await;
    ensure_admin(&mut native_k).await;
    let held = match native_k.ok("p4-kill", &all, Method::GetRdf).await {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice::<String>(&bytes)
            .expect("GetRdf answers one string")
            .lines()
            .count(),
        other => panic!("GetRdf answered {other:?}"),
    };
    assert!(
        held == previous || held == previous + last_size,
        "a commit is all or nothing: {held} triples after the kill"
    );
    native_k
        .ok(
            "p4-kill",
            &all,
            Method::AddTriples {
                turtle: String::new(),
                ntriples: last,
            },
        )
        .await;
    assert_eq!(
        graph_digest(&mut native_k, "p4-kill").await,
        graph_before,
        "the re-sent batch completes the uninterrupted import"
    );
    assert_restored(&engine_k, &graph_before, &tables_before, true, "kill").await;
    drop(native_k);
    engine_k.stop();

    write_artifact(
        "p4",
        &json!({
            "phase_exit": valid_exit(GrampsPilotMilestone::P4RestoreVerification),
            "graph_digest": graph_before,
            "table_digest": tables_before,
            "paths": ["native", "sql_dump", "kill"],
        }),
    );
    let _ = std::fs::remove_dir_all(&root);
}
