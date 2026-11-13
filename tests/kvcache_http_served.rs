//! MEDULLA-DURABLE-KERNEL-R083.4: the served `/kv/<hash>` block routes over a real
//! socket, behind the bearer guard, shared by two clients.

#![cfg(all(feature = "kvcache-server", feature = "redb"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_server_api::knob_config::{self, KnobConfig};
use medulla_server_blob::kv::{KvStateExt, KvStore};
use medulla_wire_msg::kvcache_http::{
    serve_with_security, KVCACHE_BACKEND_ENV, KVCACHE_MODEL_ID_HEADER,
    KVCACHE_MODEL_VERSION_HEADER, KVCACHE_TOKEN_ENV,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SECRET: &str = "kvcache-http-served-secret";
const TOKEN: &str = "kvcache-http-served-bearer";

/// One response: the status line's code, and the body bytes.
struct Reply {
    status: u16,
    body: Vec<u8>,
}

/// The tier budgets every test here runs under: two 1500-byte blocks fit HOT, two more
/// fit WARM, and the rest spill to the COLD namespace.
const TIER_BYTES: u64 = 4096;

/// Start the served KV-cache listener over a fresh server state with the durable,
/// tiered backend on a real `kv.redb`; returns its address.
async fn serve() -> String {
    std::env::set_var(KVCACHE_TOKEN_ENV, TOKEN);
    std::env::set_var(KVCACHE_BACKEND_ENV, "durable");
    knob_config::install(KnobConfig {
        kvcache_hot_bytes: TIER_BYTES,
        kvcache_warm_bytes: TIER_BYTES,
        ..KnobConfig::default()
    });
    let state = test_support::durable_state(SECRET, common::current_isolation());
    let dir = std::env::temp_dir().join(format!(
        "medulla-kvcache-served-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let kv = KvStore::open(Some(dir.to_str().expect("utf-8 temp path"))).expect("open kv store");
    state
        .write()
        .await
        .set_kv_store(Some(std::sync::Arc::new(kv)));
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
        call(&addr, "PUT", "prefix-1", &model_v1, &page)
            .await
            .status,
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

/// A page whose consecutive bytes differ, so the warm codec cannot shrink it.
fn page(seed: u8) -> Vec<u8> {
    (0..1500usize)
        .map(|i| (usize::from(seed) * 31 + i * 7) as u8)
        .collect()
}

/// The `/kv/stats` tier counters of the durable backend.
async fn tiers(addr: &str) -> serde_json::Value {
    let reply = call(addr, "GET", "stats", &[], b"").await;
    assert_eq!(reply.status, 200);
    let stats: serde_json::Value = serde_json::from_slice(&reply.body).expect("stats json");
    assert_eq!(stats["backend"], "durable");
    stats["tiers"].clone()
}

// spec: MEDULLA-DURABLE-KERNEL-R083.5
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_budget_put_demotes_and_get_promotes() {
    let addr = serve().await;
    for seed in 0..6u8 {
        let put = call(&addr, "PUT", &format!("tier-{seed}"), &[], &page(seed)).await;
        assert_eq!(put.status, 201);
    }
    let after_puts = tiers(&addr).await;
    assert!(after_puts["demotions_to_warm"].as_u64().unwrap() > 0);
    assert!(after_puts["demotions_to_cold"].as_u64().unwrap() > 0);
    assert!(after_puts["cold_entries"].as_u64().unwrap() > 0);
    assert!(after_puts["hot_bytes"].as_u64().unwrap() <= TIER_BYTES);

    // Every block reads back byte-identical from whichever rung it sits on.
    for seed in 0..6u8 {
        let read = call(&addr, "GET", &format!("tier-{seed}"), &[], b"").await;
        assert_eq!(read.status, 200);
        assert_eq!(read.body, page(seed), "block {seed} survived the ladder");
    }
    let after_gets = tiers(&addr).await;
    assert!(
        after_gets["promotions"].as_u64().unwrap() > after_puts["promotions"].as_u64().unwrap(),
        "a hit pages a demoted block back up"
    );
}

// spec: MEDULLA-DURABLE-KERNEL-R083.6
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn version_bump_refreshes_importance_and_central_block_outlives_peripheral() {
    let addr = serve().await;
    // The graph write hook bumps the version and sends the topology: four leaves cite a hub.
    let topology =
        br#"{"edges":[["leaf-0","hub"],["leaf-1","hub"],["leaf-2","hub"],["leaf-3","hub"]]}"#;
    assert_eq!(
        call(&addr, "PUT", "version/1", &[], topology).await.status,
        200
    );
    assert_eq!(call(&addr, "PUT", "hub", &[], &page(99)).await.status, 201);
    for seed in 0..4u8 {
        let put = call(&addr, "PUT", &format!("leaf-{seed}"), &[], &page(seed)).await;
        assert_eq!(put.status, 201);
    }

    // The hub is still HOT: reading it pages nothing up.
    let before_hub = tiers(&addr).await["promotions"].as_u64().unwrap();
    assert_eq!(call(&addr, "GET", "hub", &[], b"").await.body, page(99));
    let after_hub = tiers(&addr).await["promotions"].as_u64().unwrap();
    assert_eq!(after_hub, before_hub, "the central block never left HOT");

    // A peripheral block went down the ladder: reading it pages it back up.
    let mut paged_up = 0;
    for seed in 0..4u8 {
        let before = tiers(&addr).await["promotions"].as_u64().unwrap();
        let read = call(&addr, "GET", &format!("leaf-{seed}"), &[], b"").await;
        assert_eq!(read.body, page(seed));
        paged_up += tiers(&addr).await["promotions"].as_u64().unwrap() - before;
    }
    assert!(
        paged_up > 0,
        "peripheral blocks were demoted and paged back up"
    );

    // A malformed topology refuses instead of silently keeping the old scores.
    assert_eq!(
        call(&addr, "PUT", "version/2", &[], b"not json")
            .await
            .status,
        400
    );
}
