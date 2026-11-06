//! MEDULLA-DURABLE-KERNEL-R083.4: the served `/kv/<hash>` block routes over a real
//! socket, behind the bearer guard, shared by two clients.

#![cfg(all(feature = "kvcache-server", feature = "redb"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_wire_msg::kvcache_http::{
    serve_with_security, KVCACHE_MODEL_ID_HEADER, KVCACHE_MODEL_VERSION_HEADER, KVCACHE_TOKEN_ENV,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SECRET: &str = "kvcache-http-served-secret";
const TOKEN: &str = "kvcache-http-served-bearer";

/// One response: the status line's code, and the body bytes.
struct Reply {
    status: u16,
    body: Vec<u8>,
}

/// Start the served KV-cache listener over a fresh server state; returns its address.
async fn serve() -> String {
    std::env::set_var(KVCACHE_TOKEN_ENV, TOKEN);
    let state = test_support::durable_state(SECRET, common::current_isolation());
    let addr = test_support::ephemeral_listener_addr().await;
    let listen = addr.clone();
    tokio::spawn(async move {
        let _ = serve_with_security(&listen, state).await;
    });
    test_support::wait_for_listener_ready(&addr).await;
    addr
}

/// One authenticated request from one client over its own connection.
async fn call(
    addr: &str,
    method: &str,
    hash: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Reply {
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let mut head = format!(
        "{method} /kv/{hash} HTTP/1.1\r\nhost: x\r\nauthorization: Bearer {TOKEN}\r\n\
         content-length: {}\r\nconnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.expect("write head");
    stream.write_all(body).await.expect("write body");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read");
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("response head");
    let status = String::from_utf8_lossy(&raw[..split])
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .expect("status code");
    Reply {
        status,
        body: raw[split + 4..].to_vec(),
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R083.4
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_consumers_share_one_block_and_a_version_mismatch_refuses() {
    let addr = serve().await;
    let page = b"\x00kv-page\xffshared-by-two-clients".to_vec();
    let model_v1 = [
        (KVCACHE_MODEL_ID_HEADER, "model-a"),
        (KVCACHE_MODEL_VERSION_HEADER, "rev-1"),
    ];
    let model_v2 = [
        (KVCACHE_MODEL_ID_HEADER, "model-a"),
        (KVCACHE_MODEL_VERSION_HEADER, "rev-2"),
    ];

    // Client A puts the block; client B (its own connection) gets the same bytes.
    assert_eq!(
        call(&addr, "PUT", "prefix-1", &model_v1, &page).await.status,
        201
    );
    let read = call(&addr, "GET", "prefix-1", &model_v1, b"").await;
    assert_eq!(read.status, 200);
    assert_eq!(read.body, page);

    // A get under another model revision refuses and serves no bytes.
    let refused = call(&addr, "GET", "prefix-1", &model_v2, b"").await;
    assert_eq!(refused.status, 409);
    let body = String::from_utf8_lossy(&refused.body);
    assert!(body.contains("KV_MODEL_VERSION_MISMATCH"), "{body}");
    assert!(!refused.body.windows(page.len()).any(|w| w == page));

    // A put under another revision refuses and leaves the stored block untouched.
    let replaced = call(&addr, "PUT", "prefix-1", &model_v2, b"other bytes").await;
    assert_eq!(replaced.status, 409);
    assert_eq!(
        call(&addr, "GET", "prefix-1", &model_v1, b"").await.body,
        page
    );

    // A half-stated model identity is a bad request, not a silent pass.
    let half = call(
        &addr,
        "GET",
        "prefix-1",
        &[(KVCACHE_MODEL_ID_HEADER, "model-a")],
        b"",
    )
    .await;
    assert_eq!(half.status, 400);
}
