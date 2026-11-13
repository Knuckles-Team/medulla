//! The RESP listener owns no store: every string command enters the served KV
//! methods through the dispatch with the verified context of the principal its
//! `AUTH` resolved (MEDULLA-DURABLE-KERNEL-R860.3).
//!
//! The real listener (`redis_wire::serve`) runs over one `ServerState` that holds
//! the engine's KV store. A plain RESP client writes with `SET`; the test reads the
//! value back through the served `KvGet` method under the same principal, covers
//! `DEL`, `EXISTS`, `MGET` and `INCR`, asserts the refused families change nothing,
//! and proves a second principal never sees the first principal's keys.

#![cfg(all(feature = "redis-wire", feature = "kv"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use medulla_protocol_types::protocol::{Method, Request, ResultPayload};
use medulla_protocol_types::storage_wire::KvEntry;
use medulla_server_blob::kv::{KvStateExt, KvStore};

const AUTH_SECRET: &str = "resp-roundtrip-secret";

#[derive(Debug, PartialEq)]
enum Reply {
    Simple(String),
    Error(String),
    Int(i64),
    Bulk(Option<Vec<u8>>),
    Array(Vec<Reply>),
}

fn read_reply<'a>(
    reader: &'a mut BufReader<TcpStream>,
) -> Pin<Box<dyn Future<Output = Reply> + Send + 'a>> {
    Box::pin(async move {
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("reply line");
        let line = line.trim_end_matches("\r\n").to_string();
        let (kind, rest) = line.split_at(1);
        match kind {
            "+" => Reply::Simple(rest.to_string()),
            "-" => Reply::Error(rest.to_string()),
            ":" => Reply::Int(rest.parse().expect("integer reply")),
            "$" => {
                let length: i64 = rest.parse().expect("bulk length");
                if length < 0 {
                    return Reply::Bulk(None);
                }
                let mut body = vec![0u8; length as usize + 2];
                reader.read_exact(&mut body).await.expect("bulk body");
                body.truncate(length as usize);
                Reply::Bulk(Some(body))
            }
            "*" => {
                let count: usize = rest.parse().expect("array length");
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(read_reply(reader).await);
                }
                Reply::Array(items)
            }
            other => panic!("unexpected RESP type {other:?}"),
        }
    })
}

struct Client {
    reader: BufReader<TcpStream>,
}

impl Client {
    async fn connect(addr: &str, principal: &str) -> Self {
        let stream = TcpStream::connect(addr).await.expect("resp connect");
        let mut client = Client {
            reader: BufReader::new(stream),
        };
        let credential =
            medulla_wire_msg::redis_wire::derive_redis_password(AUTH_SECRET, principal);
        assert_eq!(
            client.call(&["AUTH", principal, &credential]).await,
            Reply::Simple("OK".into())
        );
        client
    }

    async fn call(&mut self, parts: &[&str]) -> Reply {
        let mut frame = format!("*{}\r\n", parts.len());
        for part in parts {
            frame.push_str(&format!("${}\r\n{part}\r\n", part.len()));
        }
        self.reader
            .get_mut()
            .write_all(frame.as_bytes())
            .await
            .expect("send command");
        read_reply(&mut self.reader).await
    }
}

fn bulk(text: &str) -> Reply {
    Reply::Bulk(Some(text.as_bytes().to_vec()))
}

/// `KvGet` through the served dispatch under `principal`'s verified context.
async fn kv_get(state: &test_support::SharedState, principal: &str, key: &str) -> Option<KvEntry> {
    let actor = medulla_server_api::api::dispatch_support::pseudonymous_broker_actor(
        AUTH_SECRET,
        principal,
    )
    .expect("actor reference");
    let request = Request {
        id: 9_000_000_000 + key.len() as u64,
        graph: "__commons__".to_string(),
        auth_token: String::new(),
        agent_id: None,
        method: Method::KvGet {
            namespace: "resp".to_string(),
            key: key.to_string(),
            shared: false,
        },
    };
    let response = Box::pin(
        medulla_server_router::dispatch::request_boundary::dispatch_authenticated_resp_actor(
            state, request, &actor,
        ),
    )
    .await;
    assert!(response.error.is_none(), "KvGet: {:?}", response.error);
    match response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("KvGet result"),
        other => panic!("expected a raw KvGet result, got {other:?}"),
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R860.3
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn set_get_maps_to_kv_methods() {
    let dir = test_support::sql_test_persist_dir("resp-roundtrip");
    let state = test_support::seeded_wire_state(AUTH_SECRET, "alice", Some(dir.clone()), None);
    state
        .write()
        .await
        .set_kv_store(Some(Arc::new(KvStore::open(Some(&dir)).expect("kv store"))));

    let addr = test_support::ephemeral_listener_addr().await;
    let serve_state = state.clone();
    let serve_addr = addr.clone();
    tokio::spawn(
        async move { medulla_wire_msg::redis_wire::serve(&serve_addr, serve_state).await },
    );
    test_support::wait_for_listener_ready(&addr).await;

    // No session binds a principal before AUTH, and a bad credential binds nothing.
    let mut anonymous = Client {
        reader: BufReader::new(TcpStream::connect(&addr).await.expect("connect")),
    };
    assert!(matches!(
        anonymous.call(&["SET", "k", "v"]).await,
        Reply::Error(e) if e.starts_with("NOAUTH")
    ));
    assert!(matches!(
        anonymous.call(&["AUTH", "alice", "wrong"]).await,
        Reply::Error(e) if e.starts_with("WRONGPASS")
    ));
    assert!(matches!(
        anonymous.call(&["GET", "k"]).await,
        Reply::Error(e) if e.starts_with("NOAUTH")
    ));

    let mut alice = Client::connect(&addr, "alice").await;

    // SET is one KvPut: the served KvGet under the same principal reads it back.
    assert_eq!(
        alice.call(&["SET", "k", "v1"]).await,
        Reply::Simple("OK".into())
    );
    let stored = kv_get(&state, "alice", "k")
        .await
        .expect("KvGet finds the SET value");
    assert_eq!(stored.value, b"v1");
    assert!(stored.version > 0);
    assert_eq!(alice.call(&["GET", "k"]).await, bulk("v1"));

    // EXISTS and MGET are one KvGet per key.
    assert_eq!(
        alice.call(&["SET", "k2", "v2"]).await,
        Reply::Simple("OK".into())
    );
    assert_eq!(
        alice.call(&["EXISTS", "k", "k2", "absent"]).await,
        Reply::Int(2)
    );
    assert_eq!(
        alice.call(&["MGET", "k", "absent", "k2"]).await,
        Reply::Array(vec![bulk("v1"), Reply::Bulk(None), bulk("v2")])
    );

    // DEL is KvDelete and counts the keys that existed.
    assert_eq!(alice.call(&["DEL", "k", "absent"]).await, Reply::Int(1));
    assert_eq!(alice.call(&["GET", "k"]).await, Reply::Bulk(None));
    assert!(kv_get(&state, "alice", "k").await.is_none());
    assert_eq!(alice.call(&["EXISTS", "k"]).await, Reply::Int(0));

    // INCR is a KvGet then a KvCas on the read revision.
    assert_eq!(alice.call(&["INCR", "n"]).await, Reply::Int(1));
    assert_eq!(alice.call(&["INCR", "n"]).await, Reply::Int(2));
    assert_eq!(
        alice.call(&["SET", "n", "41"]).await,
        Reply::Simple("OK".into())
    );
    assert_eq!(alice.call(&["INCR", "n"]).await, Reply::Int(42));
    assert_eq!(
        kv_get(&state, "alice", "n").await.expect("counter").value,
        b"42"
    );
    assert_eq!(
        alice.call(&["SET", "word", "abc"]).await,
        Reply::Simple("OK".into())
    );
    assert_eq!(
        alice.call(&["INCR", "word"]).await,
        Reply::Error("ERR value is not an integer".into())
    );
    assert_eq!(
        kv_get(&state, "alice", "word").await.expect("word").value,
        b"abc"
    );

    // The families the served KV methods cannot honour are refused and change nothing.
    for command in [
        &["HSET", "h", "f", "v"][..],
        &["LPUSH", "l", "v"],
        &["SADD", "s", "v"],
        &["ZADD", "z", "1", "v"],
        &["EXPIRE", "k2", "100"],
    ] {
        assert_eq!(
            alice.call(command).await,
            Reply::Error("ERR RESP_COMMAND_UNSUPPORTED".into()),
            "{command:?}"
        );
    }
    for key in ["h", "l", "s", "z"] {
        assert!(
            kv_get(&state, "alice", key).await.is_none(),
            "{key} stays absent"
        );
    }
    assert_eq!(
        alice.call(&["EXISTS", "h", "l", "s", "z"]).await,
        Reply::Int(0)
    );
    assert_eq!(alice.call(&["GET", "k2"]).await, bulk("v2"));

    // A second principal has its own private keyspace.
    let mut bob = Client::connect(&addr, "bob").await;
    assert_eq!(bob.call(&["GET", "k2"]).await, Reply::Bulk(None));
    assert_eq!(bob.call(&["EXISTS", "k2", "n"]).await, Reply::Int(0));
    assert!(kv_get(&state, "bob", "k2").await.is_none());
    assert_eq!(
        bob.call(&["SET", "k2", "bobs"]).await,
        Reply::Simple("OK".into())
    );
    assert_eq!(bob.call(&["GET", "k2"]).await, bulk("bobs"));
    assert_eq!(alice.call(&["GET", "k2"]).await, bulk("v2"));
    assert_eq!(bob.call(&["DEL", "n"]).await, Reply::Int(0));
    assert_eq!(alice.call(&["GET", "n"]).await, bulk("42"));
}
