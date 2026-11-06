//! MEDULLA-DURABLE-KERNEL-R085: served memory snapshot sync through the real
//! `dispatch` shell over a redb-backed `ServerState`.

#![cfg(feature = "redb")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_types::contract::Digest256;
use medulla_types::memory_sync::{
    DocId, MemorySnapshotRead, RequestId, Resolution, SyncAction, SyncOutcome, SyncRequest,
};

const SECRET: &str = "memory-sync-served-secret";
const ENCRYPTION_KEY: &str = "memory-sync-served-encryption-key-0123456789";
const GRAPH: &str = "memsync";
const DOC: &str = "notes/a.md";

async fn send(state: &test_support::SharedState, id: u64, method: Method) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method)).await
}

fn json_of<T: serde::de::DeserializeOwned>(response: Response) -> T {
    assert!(response.error.is_none(), "failed: {:?}", response.error);
    match response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value).expect("typed result"),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("typed result"),
        other => panic!("unexpected payload {other:?}"),
    }
}

fn sync(request_id: &str, action: SyncAction) -> Method {
    Method::SyncMemorySnapshot {
        request: Box::new(SyncRequest {
            request_id: RequestId::new(request_id).expect("request id"),
            doc_id: DocId::new(DOC).expect("doc id"),
            action,
        }),
        stamp: None,
    }
}

fn submit(request_id: &str, expected: Option<u64>, content: &str) -> Method {
    sync(
        request_id,
        SyncAction::Submit {
            expected_revision: expected,
            content: content.to_string(),
            content_digest: Digest256::sha256(content.as_bytes()),
        },
    )
}

async fn read(state: &test_support::SharedState, id: u64, revision: Option<u64>) -> Response {
    let doc_id = DOC.to_string();
    send(state, id, Method::ReadMemorySnapshot { doc_id, revision }).await
}

// spec: MEDULLA-DURABLE-KERNEL-R085.3
#[tokio::test(flavor = "multi_thread")]
async fn competing_saves_retain_both_and_resolve_once() {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "memory-sync-served",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let first: SyncOutcome = json_of(send(&state, 2, submit("r1", None, "one")).await);
    assert!(matches!(first, SyncOutcome::Accepted { ref head } if head.revision == 1));
    let second: SyncOutcome = json_of(send(&state, 3, submit("r2", Some(1), "two")).await);
    assert!(matches!(second, SyncOutcome::Accepted { ref head } if head.revision == 2));

    let stale: SyncOutcome = json_of(send(&state, 4, submit("r3", Some(1), "stale")).await);
    let SyncOutcome::ConflictRetained {
        head,
        candidate_revision,
    } = stale
    else {
        panic!("expected a retained conflict");
    };
    assert_eq!(head.revision, 2);

    let resolve = sync(
        "r4",
        SyncAction::Resolve {
            candidate_revision,
            expected_head_revision: 2,
            resolution: Resolution::TakeCandidate,
        },
    );
    let resolved: SyncOutcome = json_of(send(&state, 5, resolve.clone()).await);
    assert!(matches!(resolved, SyncOutcome::Resolved { ref head } if head.revision == 3));
    let again = send(&state, 6, resolve).await;
    assert!(again.error.is_some(), "a resolved candidate resolves once");

    let old: MemorySnapshotRead = json_of(read(&state, 7, Some(1)).await);
    assert_eq!(old.content, "one");
    let head: MemorySnapshotRead = json_of(read(&state, 8, None).await);
    assert_eq!((head.head.revision, head.content.as_str()), (3, "stale"));
}
