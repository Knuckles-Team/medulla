//! Wire protocol latency benchmark (MEDULLA-DURABLE-KERNEL-R042.5).
//!
//! Starts the four direct listeners (`pgwire`, `bolt_wire`, `redis_wire`, `s3`) over
//! one in-process `ServerState` and drives each with 64 concurrent sessions of the
//! standard client the round-trip tests use. Point-read p99 budgets: PostgreSQL
//! wire 2 ms, Bolt 3 ms, RESP 1 ms; S3-compatible GET of 1 MiB 10 ms. Writes one
//! receipt per protocol to `benches/receipts/<release>/<protocol>.json` through
//! `benches/common/claim.rs::write_receipt` and exits non-zero on a missed budget.
//! The nightly job runs it; it never gates a pull request.
//!
//! Run: cargo bench --features pgwire,bolt-wire,redis-wire,s3-api --bench wire_protocols

#[path = "common/claim.rs"]
mod claim;
#[path = "common/wire_receipt.rs"]
mod wire_receipt;

#[path = "../tests/common/bolt_client.rs"]
mod bolt_client;
#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/common/test_support.rs"]
mod test_support;

use std::future::Future;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{
    sign, PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest, SigningSettings,
    UriPathNormalizationMode,
};
use aws_sigv4::sign::v4;
use bolt_client::{BoltClient, BoltReply};
use medulla_protocol_types::protocol::Method;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use wire_receipt::WireCase;

const SECRET: &str = "wire-bench";
const USER: &str = "tester";
const SESSIONS: usize = 64;
const READS_PER_SESSION: usize = 40;
const S3_READS_PER_SESSION: usize = 8;
const S3_OBJECT_BYTES: usize = 1024 * 1024;
const S3_ACCESS_KEY: &str = "wire-bench-access";
const S3_SECRET_KEY: &str = "wire-bench-secret";

/// Run `SESSIONS` concurrent sessions; each returns its per-read latencies (ms).
async fn run_sessions<F, Fut>(session: F) -> Vec<f64>
where
    F: Fn(usize) -> Fut,
    Fut: Future<Output = Vec<f64>> + Send + 'static,
{
    let tasks: Vec<_> = (0..SESSIONS).map(|i| tokio::spawn(session(i))).collect();
    let mut samples = Vec::new();
    for task in tasks {
        samples.extend(task.await.expect("session task"));
    }
    samples
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000.0
}

async fn pgwire_case(state: test_support::SharedState) -> WireCase {
    let addr =
        test_support::spawn_pgwire_listener(state, medulla_wire::pgwire::PgWireAuthMode::Scram)
            .await;
    let samples = run_sessions(|_| {
        let addr = addr.clone();
        async move {
            let client = test_support::connect_pgwire(&addr, SECRET, USER, "__commons__")
                .await
                .expect("pgwire connect");
            let mut samples = Vec::with_capacity(READS_PER_SESSION);
            for _ in 0..READS_PER_SESSION {
                let start = Instant::now();
                client
                    .simple_query("SELECT id FROM nodes WHERE id = 'n2'")
                    .await
                    .expect("pgwire point read");
                samples.push(elapsed_ms(start));
            }
            samples
        }
    })
    .await;
    WireCase {
        protocol: "pgwire",
        operation: "point read",
        sessions: SESSIONS,
        p99_budget_ms: 2.0,
        samples_ms: samples,
    }
}

async fn bolt_case(state: test_support::SharedState) -> WireCase {
    let addr = test_support::ephemeral_listener_addr().await;
    let serve_addr = addr.clone();
    tokio::spawn(async move {
        let _ = medulla_wire::bolt_wire::serve(&serve_addr, state).await;
    });
    test_support::wait_for_listener_ready(&addr).await;
    let samples = run_sessions(|i| {
        let addr = addr.clone();
        async move {
            let mut client = BoltClient::connect(&addr).await.expect("bolt connect");
            let session = common::signed_request_as(
                SECRET,
                10_000 + i as u64,
                "__commons__",
                USER,
                Method::Health,
            );
            match client.hello(&session).await.expect("bolt hello") {
                BoltReply::Success(_) => {}
                other => panic!("bolt HELLO refused: {other:?}"),
            }
            let mut samples = Vec::with_capacity(READS_PER_SESSION);
            for _ in 0..READS_PER_SESSION {
                let start = Instant::now();
                client
                    .run_pull("MATCH (n) WHERE n.rank = 2 RETURN n.rank AS rank")
                    .await
                    .expect("bolt io")
                    .expect("bolt point read");
                samples.push(elapsed_ms(start));
            }
            samples
        }
    })
    .await;
    WireCase {
        protocol: "bolt",
        operation: "point read",
        sessions: SESSIONS,
        p99_budget_ms: 3.0,
        samples_ms: samples,
    }
}

async fn resp_roundtrip(stream: &mut TcpStream, command: &[&[u8]]) -> Vec<u8> {
    let mut frame = format!("*{}\r\n", command.len()).into_bytes();
    for part in command {
        frame.extend_from_slice(format!("${}\r\n", part.len()).as_bytes());
        frame.extend_from_slice(part);
        frame.extend_from_slice(b"\r\n");
    }
    stream.write_all(&frame).await.expect("resp write");
    let mut buf = vec![0u8; 8192];
    let n = stream.read(&mut buf).await.expect("resp read");
    buf.truncate(n);
    buf
}

async fn resp_case(state: test_support::SharedState) -> WireCase {
    let addr = test_support::ephemeral_listener_addr().await;
    let serve_addr = addr.clone();
    tokio::spawn(async move {
        let _ = medulla_wire_msg::redis_wire::serve(&serve_addr, state).await;
    });
    test_support::wait_for_listener_ready(&addr).await;
    let credential = medulla_wire_msg::redis_wire::derive_redis_password(SECRET, USER);
    let samples = run_sessions(|_| {
        let addr = addr.clone();
        let credential = credential.clone();
        async move {
            let mut stream = TcpStream::connect(&addr).await.expect("resp connect");
            let auth = resp_roundtrip(
                &mut stream,
                &[b"AUTH", USER.as_bytes(), credential.as_bytes()],
            )
            .await;
            assert!(auth.starts_with(b"+OK"), "resp AUTH: {auth:?}");
            let set = resp_roundtrip(&mut stream, &[b"SET", b"bench-key", b"bench-value"]).await;
            assert!(set.starts_with(b"+OK"), "resp SET: {set:?}");
            let mut samples = Vec::with_capacity(READS_PER_SESSION);
            for _ in 0..READS_PER_SESSION {
                let start = Instant::now();
                let reply = resp_roundtrip(&mut stream, &[b"GET", b"bench-key"]).await;
                samples.push(elapsed_ms(start));
                assert!(
                    reply.starts_with(b"$11\r\nbench-value"),
                    "resp GET: {reply:?}"
                );
            }
            samples
        }
    })
    .await;
    WireCase {
        protocol: "resp",
        operation: "point read",
        sessions: SESSIONS,
        p99_budget_ms: 1.0,
        samples_ms: samples,
    }
}

async fn s3_send(
    http: &reqwest::Client,
    base: &str,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, usize) {
    let url = format!("{base}{path}");
    let identity = Credentials::new(S3_ACCESS_KEY, S3_SECRET_KEY, None, None, "wire-bench").into();
    let mut settings = SigningSettings::default();
    settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
    settings.percent_encoding_mode = PercentEncodingMode::Single;
    settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
    let params: aws_sigv4::http_request::SigningParams<'_> = v4::SigningParams::builder()
        .identity(&identity)
        .region("us-east-1")
        .name("s3")
        .time(SystemTime::now())
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
    let (instructions, _signature) = sign(signable, &params).expect("sign").into_parts();
    let mut request = http.request(
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
    let bytes = response.bytes().await.expect("s3 body");
    (status, bytes.len())
}

async fn s3_case(state: test_support::SharedState) -> WireCase {
    std::env::set_var(medulla_wire_msg::s3::S3_ACCESS_KEY_ENV, S3_ACCESS_KEY);
    std::env::set_var(medulla_wire_msg::s3::S3_SECRET_KEY_ENV, S3_SECRET_KEY);
    let addr = test_support::ephemeral_listener_addr().await;
    let serve_addr = addr.clone();
    tokio::spawn(async move {
        let _ = medulla_wire_msg::s3::serve(&serve_addr, state).await;
    });
    test_support::wait_for_listener_ready(&addr).await;
    let base = Arc::new(format!("http://{addr}"));
    let http = reqwest::Client::new();
    let (status, _) = s3_send(&http, &base, "PUT", "/bench", b"").await;
    assert_eq!(status, 200, "s3 create bucket");
    let object = vec![0x5au8; S3_OBJECT_BYTES];
    let (status, _) = s3_send(&http, &base, "PUT", "/bench/object-1mib", &object).await;
    assert_eq!(status, 200, "s3 put 1 MiB object");
    let samples = run_sessions(|_| {
        let base = base.clone();
        let http = http.clone();
        async move {
            let mut samples = Vec::with_capacity(S3_READS_PER_SESSION);
            for _ in 0..S3_READS_PER_SESSION {
                let start = Instant::now();
                let (status, length) =
                    s3_send(&http, &base, "GET", "/bench/object-1mib", b"").await;
                samples.push(elapsed_ms(start));
                assert_eq!((status, length), (200, S3_OBJECT_BYTES), "s3 GET of 1 MiB");
            }
            samples
        }
    })
    .await;
    std::env::remove_var(medulla_wire_msg::s3::S3_ACCESS_KEY_ENV);
    std::env::remove_var(medulla_wire_msg::s3::S3_SECRET_KEY_ENV);
    WireCase {
        protocol: "s3",
        operation: "GET of 1 MiB",
        sessions: SESSIONS,
        p99_budget_ms: 10.0,
        samples_ms: samples,
    }
}

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .enable_all()
        .build()
        .expect("runtime");
    let cases = runtime.block_on(async {
        let state = test_support::seeded_wire_state(
            SECRET,
            USER,
            Some(test_support::sql_test_persist_dir("wire-protocols-bench")),
            test_support::durable_persistence("wire-protocols-bench-recovery-key"),
        );
        vec![
            pgwire_case(state.clone()).await,
            bolt_case(state.clone()).await,
            resp_case(state.clone()).await,
            s3_case(state).await,
        ]
    });
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/receipts");
    wire_receipt::write_wire_receipts(&root, env!("CARGO_PKG_VERSION"), &cases)
        .expect("write receipts");
    let mut missed = false;
    for case in &cases {
        println!(
            "wire_protocols {}: p50 {:.4} ms, p99 {:.4} ms (budget {}), {} sessions",
            case.protocol,
            case.p50_ms(),
            case.p99_ms(),
            case.p99_budget_ms,
            case.sessions
        );
        missed |= !case.within_budget();
    }
    if missed {
        eprintln!("wire_protocols: a p99 budget was missed");
        std::process::exit(1);
    }
}
