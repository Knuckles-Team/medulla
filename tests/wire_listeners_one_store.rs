//! The four direct wire listeners refuse unauthenticated sessions and unsafe
//! startup policy without changing stored state (MEDULLA-DURABLE-KERNEL-R860.5).
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

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

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
        .err()
        .expect("pgwire refuses a wrong password");
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
