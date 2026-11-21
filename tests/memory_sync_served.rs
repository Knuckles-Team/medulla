//! MEDULLA-DURABLE-KERNEL-R085: served memory snapshot sync through the real
//! `dispatch` shell over a redb-backed `ServerState`.

#![cfg(feature = "redb")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_mutation_types::memory_sync::{
    DocId, MemorySnapshotRead, RequestId, Resolution, SyncAction, SyncOutcome, SyncRequest,
};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_types::contract::Digest256;

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
            admit_receipt_digests: Vec::new(),
        }),
        stamp: None,
    }
}

/// Store the real receipts that admit `kind` of `content` over head `head`, and
/// name them in the request (MEDULLA-DURABLE-KERNEL-R863).
async fn admitted(
    state: &test_support::SharedState,
    mut method: Method,
    content: &str,
    head: u64,
    kind: medulla_decision_types::decision::memory_benchmark::ChangeKind,
) -> Method {
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;
    let store = state
        .write()
        .await
        .ensure_agent_library()
        .expect("the agent library store");
    let digests = medulla_h_decide::decide::stat_test_support::store_admit_receipts(
        &store,
        common::TEST_TENANT,
        GRAPH,
        DOC,
        head,
        &format!("sha256:{}", Digest256::sha256(content.as_bytes()).to_hex()),
        kind,
    )
    .expect("the admitting receipts");
    if let Method::SyncMemorySnapshot { request, .. } = &mut method {
        request.admit_receipt_digests = digests;
    }
    method
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
    let rewrite = admitted(
        &state,
        submit("r2", Some(1), "two"),
        "two",
        1,
        medulla_decision_types::decision::memory_benchmark::ChangeKind::Rewrite,
    )
    .await;
    let second: SyncOutcome = json_of(send(&state, 3, rewrite).await);
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

    let resolve = admitted(
        &state,
        sync(
            "r4",
            SyncAction::Resolve {
                candidate_revision,
                expected_head_revision: 2,
                resolution: Resolution::TakeCandidate,
            },
        ),
        "stale",
        2,
        medulla_decision_types::decision::memory_benchmark::ChangeKind::Consolidate,
    )
    .await;
    let resolved: SyncOutcome = json_of(send(&state, 5, resolve.clone()).await);
    assert!(matches!(resolved, SyncOutcome::Resolved { ref head } if head.revision == 3));
    let again = send(&state, 6, resolve).await;
    assert!(again.error.is_some(), "a resolved candidate resolves once");

    let old: MemorySnapshotRead = json_of(read(&state, 7, Some(1)).await);
    assert_eq!(old.content, "one");
    let head: MemorySnapshotRead = json_of(read(&state, 8, None).await);
    assert_eq!((head.head.revision, head.content.as_str()), (3, "stale"));
}

fn list(cursor: Option<String>) -> Method {
    Method::ListMemorySnapshots {
        prefix: "page/".to_string(),
        limit: Some(2),
        cursor,
    }
}

fn error_of(response: &Response) -> String {
    format!("{:?}", response.error.as_ref().expect("a refusal"))
}

// spec: MEDULLA-DURABLE-KERNEL-R085.2
#[tokio::test(flavor = "multi_thread")]
async fn authorized_read_list_and_cursor_isolation() {
    let mut isolation = common::current_isolation();
    isolation.register_agent(medulla_identity_types::acl::AgentIdentity {
        agent_id: "root".to_string(),
        role: medulla_identity_types::acl::AgentRole::System,
        teams: Vec::new(),
        roles: Vec::new(),
    });
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "memory-sync-list",
        isolation,
        GRAPH,
    )
    .await;
    for (id, doc) in [(10, "page/a.md"), (11, "page/b.md"), (12, "page/c.md")] {
        let method = Method::SyncMemorySnapshot {
            request: Box::new(SyncRequest {
                request_id: RequestId::new(format!("list-{doc}")).expect("request id"),
                doc_id: DocId::new(doc).expect("doc id"),
                action: SyncAction::Submit {
                    expected_revision: None,
                    content: doc.to_string(),
                    content_digest: Digest256::sha256(doc.as_bytes()),
                },
                admit_receipt_digests: Vec::new(),
            }),
            stamp: None,
        };
        let _: SyncOutcome = json_of(send(&state, id, method).await);
    }

    let first: medulla_mutation_types::memory_sync::MemorySnapshotList =
        json_of(send(&state, 20, list(None)).await);
    let names: Vec<&str> = first.heads.iter().map(|h| h.doc_id.as_str()).collect();
    assert_eq!(names, ["page/a.md", "page/b.md"]);
    let cursor = first
        .next_cursor
        .expect("a second page")
        .as_str()
        .to_string();
    let second: medulla_mutation_types::memory_sync::MemorySnapshotList =
        json_of(send(&state, 21, list(Some(cursor.clone()))).await);
    let names: Vec<&str> = second.heads.iter().map(|h| h.doc_id.as_str()).collect();
    assert_eq!(names, ["page/c.md"]);
    assert!(second.next_cursor.is_none());

    // The same cursor under another principal, or edited, is refused by name.
    let other = test_support::dispatch(
        &state,
        common::signed_request_as(SECRET, 22, GRAPH, "root", list(Some(cursor.clone()))),
    )
    .await;
    assert!(error_of(&other).contains("MEMSYNC_CURSOR_INVALID"));
    let flip = if cursor.ends_with('a') { "b" } else { "a" };
    let edited = format!("{}{flip}", &cursor[..cursor.len() - 1]);
    let tampered = send(&state, 23, list(Some(edited))).await;
    assert!(error_of(&tampered).contains("MEMSYNC_CURSOR_INVALID"));

    // Another tenant never resumes this tenant's listing.
    let mut foreign = common::context_for(common::TEST_AGENT);
    foreign.tenant = "tenant-foreign".to_string();
    let cross = test_support::dispatch(
        &state,
        common::signed_request_with_context(SECRET, 24, GRAPH, foreign, list(Some(cursor))),
    )
    .await;
    assert!(cross.error.is_some(), "a foreign tenant is refused");
}

// spec: MEDULLA-DURABLE-KERNEL-R085.4
#[tokio::test(flavor = "multi_thread")]
async fn wire_methods_reach_durable_gateway() {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "memory-sync-gateway",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    // A client that names the engine-internal store method with its own plan is
    // refused by name, and the forged row is never written.
    let forged = Method::SyncMemorySnapshotStore {
        request: Box::new(SyncRequest {
            request_id: RequestId::new("forged").expect("request id"),
            doc_id: DocId::new(DOC).expect("doc id"),
            action: SyncAction::Submit {
                expected_revision: None,
                content: "one".to_string(),
                content_digest: Digest256::sha256(b"one"),
            },
            admit_receipt_digests: Vec::new(),
        }),
        plan: Box::new(medulla_mutation_types::memory_sync::SnapshotPlan {
            base_revision: None,
            base_candidates: 0,
            writes: vec![medulla_mutation_types::memory_sync::SnapshotRowWrite {
                node_id: "memsnap:forged".to_string(),
                properties_msgpack: Vec::new(),
            }],
        }),
    };
    let refused = send(&state, 6, forged).await;
    assert!(
        error_of(&refused).contains("ENGINE_INTERNAL_METHOD"),
        "{refused:?}"
    );
    let forged_doc = send(
        &state,
        7,
        Method::ReadMemorySnapshot {
            doc_id: "forged".to_string(),
            revision: None,
        },
    )
    .await;
    assert!(forged_doc.error.is_some(), "the forged row was never written");
    let accepted = submit("wire", None, "one");
    let outcome: SyncOutcome = json_of(send(&state, 2, accepted).await);
    assert!(matches!(outcome, SyncOutcome::Accepted { ref head } if head.revision == 1));
    let head: MemorySnapshotRead = json_of(read(&state, 3, None).await);
    assert_eq!(head.content, "one");
    let revision: MemorySnapshotRead = json_of(read(&state, 4, Some(1)).await);
    assert_eq!(revision.head, head.head);
}

// spec: MEDULLA-DURABLE-KERNEL-R863
#[tokio::test(flavor = "multi_thread")]
async fn sync_rewrite_requires_a_current_admit_receipt() {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "memory-sync-gate",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let first: SyncOutcome = json_of(send(&state, 2, submit("g1", None, "one")).await);
    assert!(matches!(first, SyncOutcome::Accepted { ref head } if head.revision == 1));
    for (id, digests) in [
        (3, Vec::new()),
        (4, vec!["sha256:absent".to_string()]),
    ] {
        let mut method = submit(&format!("g{id}"), Some(1), "two");
        if let Method::SyncMemorySnapshot { request, .. } = &mut method {
            request.admit_receipt_digests = digests;
        }
        let refused = send(&state, id, method).await;
        assert!(
            error_of(&refused).contains("MEMORY_RECEIPT_REQUIRED"),
            "{refused:?}"
        );
    }
    let head: MemorySnapshotRead = json_of(read(&state, 5, None).await);
    assert_eq!((head.head.revision, head.content.as_str()), (1, "one"));
}
