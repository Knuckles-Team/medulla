//! The four direct wire listeners refuse unauthenticated sessions and unsafe
//! startup policy without changing stored state (MEDULLA-DURABLE-KERNEL-R860.5),
//! and one `medulla-server` binary serves all four over one store: a standard client
//! writes one record through each protocol and the served native method of each
//! modality reads it back, before and after a restart (MEDULLA-DURABLE-KERNEL-R860.6).
//!
//! Each listener is the real served entry (`serve` of pgwire, bolt_wire,
//! redis_wire, s3) over one shared `ServerState`. A session that presents no
//! valid credential receives its protocol's refusal form, and every startup
//! policy refuses a non-loopback address without a configured credential.

#![cfg(all(
    feature = "pgwire",
    feature = "bolt-wire",
    feature = "redis-wire",
    feature = "s3-api"
))]

#[path = "common/bolt_client.rs"]
mod bolt_client;
mod common;
#[path = "common/test_support.rs"]
mod test_support;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use medulla_protocol_types::protocol::{Method, Request};
use medulla_wire::bolt_wire::packstream::{self, PackValue};

const AUTH_SECRET: &str = "test";
const USER: &str = "tester";

fn commons_node_count(state: &test_support::SharedState) -> usize {
    state
        .try_read()
        .expect("state is idle between sessions")
        .registry
        .get("__commons__")
        .expect("commons graph")
        .core
        .node_count()
}

async fn spawn<F, Fut>(serve: F) -> String
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = std::io::Result<()>> + Send + 'static,
{
    let addr = test_support::ephemeral_listener_addr().await;
    tokio::spawn(serve(addr.clone()));
    test_support::wait_for_listener_ready(&addr).await;
    addr
}

async fn read_some(stream: &mut TcpStream) -> Vec<u8> {
    let mut buf = vec![0u8; 8192];
    let n = tokio::time::timeout(std::time::Duration::from_secs(10), stream.read(&mut buf))
        .await
        .expect("listener answers")
        .expect("read");
    buf.truncate(n);
    buf
}

async fn bolt_unauthenticated_hello(addr: &str) -> u8 {
    let mut stream = TcpStream::connect(addr).await.expect("bolt connect");
    let mut handshake = vec![0x60, 0x60, 0xB0, 0x17];
    handshake.extend_from_slice(&[0, 0, 4, 4]);
    handshake.extend_from_slice(&[0u8; 12]);
    stream.write_all(&handshake).await.expect("handshake");
    let mut version = [0u8; 4];
    stream.read_exact(&mut version).await.expect("version");
    let hello = PackValue::Structure {
        tag: 0x01,
        fields: vec![PackValue::map(vec![(
            "user_agent",
            PackValue::String("test/1.0".into()),
        )])],
    };
    stream
        .write_all(&packstream::encode_chunked(&hello))
        .await
        .expect("hello");
    let reply = read_some(&mut stream).await;
    let messages = packstream::Dechunker::split_messages(&reply).expect("chunked reply");
    match packstream::decode(&messages[0]).expect("packstream") {
        PackValue::Structure { tag, .. } => tag,
        other => panic!("bolt reply is not a message: {other:?}"),
    }
}

async fn resp_unauthenticated_set(addr: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("resp connect");
    stream
        .write_all(b"*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$1\r\nv\r\n")
        .await
        .expect("set");
    String::from_utf8_lossy(&read_some(&mut stream).await).into_owned()
}

async fn s3_unsigned_put(addr: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("s3 connect");
    let host = addr;
    let request = format!(
        "PUT /bucket/key HTTP/1.1\r\nHost: {host}\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx"
    );
    stream.write_all(request.as_bytes()).await.expect("put");
    String::from_utf8_lossy(&read_some(&mut stream).await).into_owned()
}

// spec: MEDULLA-DURABLE-KERNEL-R860.5
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unauthenticated_sessions_refuse() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    std::env::set_var(medulla_wire_msg::s3::S3_ACCESS_KEY_ENV, "wire-access");
    std::env::set_var(medulla_wire_msg::s3::S3_SECRET_KEY_ENV, "wire-secret");
    let state = test_support::seeded_wire_state(
        AUTH_SECRET,
        USER,
        Some(test_support::sql_test_persist_dir("wire-refusals")),
        test_support::durable_persistence("wire-refusals-recovery-key"),
    );
    let before = commons_node_count(&state);

    let pg = test_support::spawn_pgwire_listener(
        state.clone(),
        medulla_wire::pgwire::PgWireAuthMode::Scram,
    )
    .await;
    let pg_refusal = test_support::connect_pgwire_with_password(&pg, "wrong", USER, "__commons__")
        .await
        .expect_err("pgwire refuses a wrong password");
    assert!(
        pg_refusal.as_db_error().is_some(),
        "PostgreSQL error response: {pg_refusal}"
    );

    let bolt_state = state.clone();
    let bolt =
        spawn(move |addr| async move { medulla_wire::bolt_wire::serve(&addr, bolt_state).await })
            .await;
    assert_eq!(
        bolt_unauthenticated_hello(&bolt).await,
        0x7F,
        "Bolt FAILURE"
    );

    let resp_state = state.clone();
    let resp =
        spawn(
            move |addr| async move { medulla_wire_msg::redis_wire::serve(&addr, resp_state).await },
        )
        .await;
    assert!(resp_unauthenticated_set(&resp).await.starts_with("-NOAUTH"));

    let s3_state = state.clone();
    let s3 =
        spawn(move |addr| async move { medulla_wire_msg::s3::serve(&addr, s3_state).await }).await;
    let s3_reply = s3_unsigned_put(&s3).await;
    assert!(s3_reply.contains("403") && s3_reply.contains("AccessDenied"));

    // Startup policy: a non-loopback address without a configured credential
    // is refused before bind.
    let remote = "0.0.0.0:0";
    assert!(medulla_wire::bolt_wire::validate_startup_policy(remote, "").is_err());
    assert!(medulla_wire_msg::redis_wire::validate_startup_policy(remote, "").is_err());
    assert!(medulla_wire::pgwire::prepare_startup_policy(
        remote,
        String::new(),
        medulla_wire::pgwire::PgWireAuthMode::Scram,
    )
    .await
    .is_err());
    std::env::remove_var(medulla_wire_msg::s3::S3_ACCESS_KEY_ENV);
    std::env::remove_var(medulla_wire_msg::s3::S3_SECRET_KEY_ENV);
    assert!(medulla_wire_msg::s3::validate_startup_policy(remote, "").is_err());

    assert_eq!(
        commons_node_count(&state),
        before,
        "a refusal changes no state"
    );
}

// ── MEDULLA-DURABLE-KERNEL-R860.6: one store behind four protocols ──────────────────

#[cfg(all(unix, feature = "kv", feature = "blob"))]
mod one_store {
    use super::*;

    /// The signer, and the one `System` identity every protocol session acts as.
    const ADMIN: &str = "wireadmin";
    const ADMIN_SIGNER_KEY: &str = "wire-one-store-signing-key";
    const S3_ACCESS: &str = "wire-one-store-access";
    const S3_SECRET: &str = "wire-one-store-secret";
    const PG_ID: &str = "pg-row";
    const BOLT_ID: &str = "bolt-node";
    const RESP_KEY: &str = "wire-key";
    const RESP_VALUE: &str = "wire-value";
    const S3_OBJECT: &[u8] = b"one store behind the S3 surface, read by digest";

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

    struct Addresses {
        pg: String,
        bolt: String,
        resp: String,
        s3: String,
    }

    /// One running `medulla-server` over `dir`, listening on all four protocols.
    struct RunningServer {
        child: std::process::Child,
        socket: std::path::PathBuf,
    }

    impl RunningServer {
        async fn start(dir: &std::path::Path, addresses: &Addresses) -> Self {
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
            serde_json::json!({"key": ADMIN_SIGNER_KEY, "allowed_roles": [], "may_grant_system": true}),
        );
            let signers = serde_json::Value::Object(signers);
            let mut command = std::process::Command::new(server_bin());
            command
                .arg("--socket-path")
                .arg(&socket)
                .env_clear()
                .env("PATH", std::env::var("PATH").unwrap_or_default())
                .env("GRAPH_SERVICE_AUTH_SECRET", AUTH_SECRET)
                .env("GRAPH_SERVICE_PERSIST_DIR", dir.join("persist"))
                .env("MEDULLA_SIGNER_KEYS_JSON", signers.to_string())
                .env(medulla_wire::pgwire::PGWIRE_ADDR_ENV, &addresses.pg)
                .env(medulla_wire::bolt_wire::BOLT_ADDR_ENV, &addresses.bolt)
                .env(
                    medulla_wire_msg::redis_wire::REDIS_ADDR_ENV,
                    &addresses.resp,
                )
                .env(medulla_wire_msg::s3::S3_ADDR_ENV, &addresses.s3)
                .env(medulla_wire_msg::s3::S3_ACCESS_KEY_ENV, S3_ACCESS)
                .env(medulla_wire_msg::s3::S3_SECRET_KEY_ENV, S3_SECRET)
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
            server.wait_ready(addresses).await;
            server
        }

        async fn wait_ready(&mut self, addresses: &Addresses) {
            for _ in 0..1500 {
                if let Some(status) = self.child.try_wait().expect("poll server") {
                    panic!("medulla-server exited before it listened: {status}");
                }
                let native = tokio::net::UnixStream::connect(&self.socket).await.is_ok();
                let mut wire = true;
                for addr in [
                    &addresses.pg,
                    &addresses.bolt,
                    &addresses.resp,
                    &addresses.s3,
                ] {
                    wire &= TcpStream::connect(addr).await.is_ok();
                }
                if native && wire {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            panic!("medulla-server did not open its native socket and four wire listeners");
        }

        /// SIGTERM, then wait for the process to exit.
        fn stop(mut self) {
            let status = std::process::Command::new("kill")
                .arg("-TERM")
                .arg(self.child.id().to_string())
                .status()
                .expect("kill");
            assert!(status.success(), "SIGTERM the server");
            let exit = self.child.wait().expect("server exit");
            let _ = exit;
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

        fn context(agent: &str, scopes: &[&str]) -> medulla_types::acl::RequestContextClaims {
            let mut claims = common::context_for(agent);
            claims.scopes = scopes.iter().map(|scope| scope.to_string()).collect();
            claims
        }

        async fn call(
            &mut self,
            context: &medulla_types::acl::RequestContextClaims,
            method: Method,
            signer: Option<&str>,
        ) -> medulla_protocol_types::protocol::Response {
            use hmac::Mac as _;
            use sha2::Digest as _;
            self.next_id += 1;
            self.sequence += 1;
            let id = self.next_id;
            let nonce = format!("one-store-{id}-{}", self.sequence);
            let idempotency_key = format!("one-store-request-{id}-{}", self.sequence);
            let graph = "__commons__";
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
                    let digest = sha2::Sha256::digest(bytes);
                    let mut mac =
                        hmac::Hmac::<sha2::Sha256>::new_from_slice(ADMIN_SIGNER_KEY.as_bytes())
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
            context: &medulla_types::acl::RequestContextClaims,
            method: Method,
        ) -> Option<medulla_protocol_types::protocol::ResultPayload> {
            let response = self.call(context, method, None).await;
            assert!(
                response.error.is_none(),
                "served method refused: {response:?}"
            );
            response.result
        }
    }

    async fn pg_write(addr: &str) {
        let client = test_support::connect_pgwire(addr, AUTH_SECRET, ADMIN, "__commons__")
            .await
            .expect("pgwire connect");
        client
        .simple_query(&format!(
            "INSERT INTO nodes (id, type, rank, _visibility, _owner) VALUES ('{PG_ID}', 'Agent', 7, 'public', '{ADMIN}')"
        ))
        .await
        .expect("pgwire INSERT");
    }

    async fn bolt_write(addr: &str) {
        let mut client = bolt_client::BoltClient::connect(addr)
            .await
            .expect("bolt connect");
        let session =
            common::signed_request_as(AUTH_SECRET, 1, "__commons__", ADMIN, Method::Health);
        match client.hello(&session).await.expect("hello") {
            bolt_client::BoltReply::Success(_) => {}
            other => panic!("Bolt HELLO refused: {other:?}"),
        }
        let rows = client
            .run_pull(&format!(
                "CREATE (n:Bolt {{id: '{BOLT_ID}'}}) RETURN n.id AS id"
            ))
            .await
            .expect("run/pull io")
            .unwrap_or_else(|failure| panic!("Bolt CREATE failed: {failure:?}"));
        assert_eq!(rows.len(), 1, "CREATE returns the new node: {rows:?}");
    }

    async fn resp_write(addr: &str) {
        let mut stream = TcpStream::connect(addr).await.expect("resp connect");
        let credential = medulla_wire_msg::redis_wire::derive_redis_password(AUTH_SECRET, ADMIN);
        for parts in [
            vec!["AUTH", ADMIN, credential.as_str()],
            vec!["SET", RESP_KEY, RESP_VALUE],
        ] {
            let mut frame = format!("*{}\r\n", parts.len());
            for part in &parts {
                frame.push_str(&format!("${}\r\n{part}\r\n", part.len()));
            }
            stream.write_all(frame.as_bytes()).await.expect("resp send");
            let reply = read_some(&mut stream).await;
            assert!(
                reply.starts_with(b"+OK"),
                "RESP {}: {:?}",
                parts[0],
                String::from_utf8_lossy(&reply)
            );
        }
    }

    /// A SigV4-signed S3 request; returns `(status, ETag, body)`.
    async fn s3_send(
        addr: &str,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> (u16, Option<String>, Vec<u8>) {
        use aws_credential_types::Credentials;
        use aws_sigv4::http_request::{
            sign, PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest,
            SigningSettings, UriPathNormalizationMode,
        };
        let url = format!("http://{addr}{path}");
        let identity = Credentials::new(S3_ACCESS, S3_SECRET, None, None, "one-store").into();
        let mut settings = SigningSettings::default();
        settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
        settings.percent_encoding_mode = PercentEncodingMode::Single;
        settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
        let params: aws_sigv4::http_request::SigningParams<'_> =
            aws_sigv4::sign::v4::SigningParams::builder()
                .identity(&identity)
                .region("us-east-1")
                .name("s3")
                .time(std::time::SystemTime::now())
                .settings(settings)
                .build()
                .expect("sigv4 params")
                .into();
        let signable = SignableRequest::new(
            method,
            url.as_str(),
            std::iter::empty::<(&str, &str)>(),
            SignableBody::Bytes(body),
        )
        .expect("signable request");
        let (instructions, _) = sign(signable, &params).expect("sign").into_parts();
        let mut request = reqwest::Client::new().request(
            reqwest::Method::from_bytes(method.as_bytes()).expect("method"),
            url.as_str(),
        );
        for (name, value) in instructions.headers() {
            request = request.header(name, value);
        }
        let response = request
            .body(body.to_vec())
            .send()
            .await
            .expect("s3 request");
        let status = response.status().as_u16();
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.trim_matches('"').to_string());
        (
            status,
            etag,
            response.bytes().await.expect("s3 body").to_vec(),
        )
    }

    async fn s3_write(addr: &str) -> String {
        let (status, _, _) = s3_send(addr, "PUT", "/onestore", b"").await;
        assert_eq!(status, 200, "create bucket");
        let (status, etag, _) = s3_send(addr, "PUT", "/onestore/object.bin", S3_OBJECT).await;
        assert_eq!(status, 200, "put object");
        etag.expect("the PUT answers the blob digest as its ETag")
    }

    /// Read each record back through the served native method of its modality.
    async fn read_native(socket: &std::path::Path, s3_digest: &str) {
        use medulla_protocol_types::protocol::ResultPayload;
        let mut native = Native::connect(socket).await;
        let admin = Native::context(ADMIN, &["*"]);

        // SQL row and Bolt node: GetNodes.
        let nodes = match native.ok(&admin, Method::GetNodes).await {
            Some(ResultPayload::NodeList(nodes)) => nodes,
            other => panic!("GetNodes answered {other:?}"),
        };
        let ids: Vec<&str> = nodes.iter().map(|(id, _)| id.as_str()).collect();
        assert!(ids.contains(&PG_ID), "the pgwire row is a node: {ids:?}");
        assert!(ids.contains(&BOLT_ID), "the Bolt node is a node: {ids:?}");
        // The modalities hold separate records.
        for foreign in [RESP_KEY, "onestore", "object.bin", s3_digest] {
            assert!(
                !ids.contains(&foreign),
                "{foreign} is not a graph node: {ids:?}"
            );
        }

        // RESP key: KvGet under the principal's pseudonymous actor.
        let actor = medulla_server_api::api::dispatch_support::pseudonymous_broker_actor(
            AUTH_SECRET,
            ADMIN,
        )
        .expect("actor reference");
        let resp_actor = Native::context(&actor, &["*"]);
        let kv_get = |key: &str| Method::KvGet {
            namespace: "resp".to_string(),
            key: key.to_string(),
            shared: false,
        };
        match native.ok(&resp_actor, kv_get(RESP_KEY)).await {
            Some(ResultPayload::Raw(bytes)) => {
                let entry: Option<medulla_protocol_types::storage_wire::KvEntry> =
                    rmp_serde::from_slice(&bytes).expect("KvGet result");
                assert_eq!(
                    entry.expect("the RESP key is stored").value,
                    RESP_VALUE.as_bytes()
                );
            }
            other => panic!("KvGet answered {other:?}"),
        }
        for foreign in [PG_ID, BOLT_ID] {
            match native.ok(&resp_actor, kv_get(foreign)).await {
                Some(ResultPayload::Raw(bytes)) => {
                    let entry: Option<medulla_protocol_types::storage_wire::KvEntry> =
                        rmp_serde::from_slice(&bytes).expect("KvGet result");
                    assert!(entry.is_none(), "{foreign} is not a key");
                }
                other => panic!("KvGet answered {other:?}"),
            }
        }

        // S3 object: BlobFetchBegin and BlobChunkGet under the S3 service principal.
        let s3_actor = Native::context("service:s3-api-client", &["*"]);
        let (cursor, chunks): (u64, u32) = match native
            .ok(
                &s3_actor,
                Method::BlobFetchBegin {
                    digest: s3_digest.to_string(),
                },
            )
            .await
        {
            Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("fetch cursor"),
            other => panic!("BlobFetchBegin answered {other:?}"),
        };
        let mut object = Vec::new();
        for idx in 0..chunks {
            match native
                .ok(&s3_actor, Method::BlobChunkGet { cursor, idx })
                .await
            {
                Some(ResultPayload::Raw(bytes)) => {
                    let chunk: serde_bytes::ByteBuf = rmp_serde::from_slice(&bytes).expect("chunk");
                    object.extend(chunk.into_vec());
                }
                other => panic!("BlobChunkGet answered {other:?}"),
            }
        }
        assert_eq!(
            object, S3_OBJECT,
            "the S3 object reads back through the blob methods"
        );
        native.ok(&s3_actor, Method::BlobFetchEnd { cursor }).await;
    }

    // spec: MEDULLA-DURABLE-KERNEL-R860.6
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn write_each_protocol_read_native() {
        let dir = test_support::fresh_dir("wire-one-store");
        let addresses = Addresses {
            pg: test_support::ephemeral_listener_addr().await,
            bolt: test_support::ephemeral_listener_addr().await,
            resp: test_support::ephemeral_listener_addr().await,
            s3: test_support::ephemeral_listener_addr().await,
        };

        let server = RunningServer::start(&dir, &addresses).await;

        // The one System identity every session acts as, registered through the
        // bootstrap gate.
        let mut native = Native::connect(&server.socket).await;
        let bootstrap = Native::context(ADMIN, &["security:bootstrap"]);
        let registered = native
            .call(
                &bootstrap,
                Method::RegisterIdentity {
                    agent_id: ADMIN.to_string(),
                    role: medulla_types::acl::AgentRole::System,
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
        drop(native);

        // A standard client writes one record through each protocol.
        pg_write(&addresses.pg).await;
        bolt_write(&addresses.bolt).await;
        resp_write(&addresses.resp).await;
        let s3_digest = s3_write(&addresses.s3).await;

        read_native(&server.socket, &s3_digest).await;

        // Restart the binary over the same data directory; all four read back again.
        server.stop();
        let server = RunningServer::start(&dir, &addresses).await;
        read_native(&server.socket, &s3_digest).await;
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
