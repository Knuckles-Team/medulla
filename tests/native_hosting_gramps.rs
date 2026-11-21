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

#![cfg(all(unix, feature = "pgwire", feature = "rdf"))]

mod common;
#[path = "support/family_tree.rs"]
mod family_tree;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_catalog_types::gramps_baseline_digest::GrampsBaselineDigest;
use medulla_catalog_types::gramps_pilot_phase::{GrampsPilotMilestone, PhaseExitArtifact};
use medulla_protocol_types::protocol::{GraphType, Method, Request, ResultPayload};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use family_tree::{family_tree, family_tree_corpus, PHASE_EXIT_INVALID};

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

/// One running `medulla-server` over `dir` with its native socket and pgwire listener.
struct RunningServer {
    child: std::process::Child,
    socket: std::path::PathBuf,
}

impl RunningServer {
    async fn start(dir: &std::path::Path, pg_addr: &str) -> Self {
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
        let mut command = std::process::Command::new(server_bin());
        command
            .arg("--socket-path")
            .arg(&socket)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("GRAPH_SERVICE_AUTH_SECRET", AUTH_SECRET)
            .env("GRAPH_SERVICE_PERSIST_DIR", dir.join("persist"))
            .env("MEDULLA_SIGNER_KEYS_JSON", signers.to_string())
            .env(medulla_wire::pgwire::PGWIRE_ADDR_ENV, pg_addr)
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
        std::fs::create_dir_all(dir.join("persist")).expect("persist dir");
        let child = command.spawn().expect("spawn medulla-server");
        let mut server = Self { child, socket };
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

    fn context(scopes: &[&str]) -> medulla_identity_types::acl::RequestContextClaims {
        let mut claims = common::context_for(ADMIN);
        claims.scopes = scopes.iter().map(|scope| scope.to_string()).collect();
        claims
    }

    async fn call(
        &mut self,
        graph: &str,
        context: &medulla_identity_types::acl::RequestContextClaims,
        method: Method,
        signer: Option<&str>,
    ) -> medulla_protocol_types::protocol::Response {
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
                let bytes = medulla_protocol_types::protocol::build_context_operation_signature_bytes(
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
        context: &medulla_identity_types::acl::RequestContextClaims,
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

// spec: MEDULLA-UNIFIED-DATA-PLANE-R026.2.2
#[tokio::test(flavor = "multi_thread")]
async fn p0_p1_p2_exit_artifacts() {
    let dir = test_support::fresh_dir("native-hosting-gramps");
    let pg_addr = test_support::ephemeral_listener_addr().await;
    let server = RunningServer::start(&dir, &pg_addr).await;

    // The one System identity every session acts as, registered through the bootstrap gate.
    let mut native = Native::connect(&server.socket).await;
    let bootstrap = Native::context(&["security:bootstrap"]);
    let registered = native
        .call(
            "__commons__",
            &bootstrap,
            Method::RegisterIdentity {
                agent_id: ADMIN.to_string(),
                role: medulla_identity_types::acl::AgentRole::System,
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
    assert_eq!(export.lines().count(), 1_019, "the export holds every triple");
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
