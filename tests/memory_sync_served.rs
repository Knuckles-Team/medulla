//! MEDULLA-DURABLE-KERNEL-R085: served memory snapshot sync through the real
//! `dispatch` shell over a redb-backed `ServerState`.

#![cfg(feature = "redb")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_mutation_types::memory_sync::{
    DocId, MemorySnapshotRead, RequestId, Resolution, SyncAction, SyncOutcome, SyncRequest,
    SyncStanding,
};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_types::contract::Digest256;

const SECRET: &str = "memory-sync-served-secret";
const ENCRYPTION_KEY: &str = "memory-sync-served-encryption-key-0123456789";
const GRAPH: &str = "memsync";
const DOC: &str = "notes/a.md";

/// The tenant capacity every test of this binary runs under: the knob's lowest
/// bound, so the exhaustion refusal is reachable with one maximal document.
/// The first install wins, so every test installs before it dispatches.
fn install_knobs() {
    let args = vec!["memory_sync.capacity_bytes=1048576".to_string()];
    let loaded = medulla_server_api::knob_config::load_knobs(&args, |_: &str| None)
        .expect("the capacity knob loads");
    medulla_server_api::knob_config::install(loaded);
}

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
    install_knobs();
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
    install_knobs();
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
    install_knobs();
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
    install_knobs();
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

/// The head and the content of `DOC` as a client reads them.
async fn head_bytes(state: &test_support::SharedState, id: u64) -> (u64, String) {
    let read: MemorySnapshotRead = json_of(read(state, id, None).await);
    (read.head.revision, read.content)
}

// spec: MEDULLA-DURABLE-KERNEL-R085.5
#[tokio::test(flavor = "multi_thread")]
async fn denied_and_malformed_requests_have_no_effects() {
    install_knobs();
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "memory-sync-refusals",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let first: SyncOutcome = json_of(send(&state, 2, submit("n1", None, "one")).await);
    assert!(matches!(first, SyncOutcome::Accepted { ref head } if head.revision == 1));
    let before = head_bytes(&state, 3).await;

    // Malformed submits are refused by name and write nothing.
    let mismatch = sync(
        "n2",
        SyncAction::Submit {
            expected_revision: Some(1),
            content: "two".to_string(),
            content_digest: Digest256::sha256(b"not two"),
        },
    );
    assert!(error_of(&send(&state, 4, mismatch).await).contains("MEMSYNC_DIGEST_MISMATCH"));
    let big = "x".repeat(1_048_577);
    let too_large = sync(
        "n3",
        SyncAction::Submit {
            expected_revision: Some(1),
            content_digest: Digest256::sha256(big.as_bytes()),
            content: big,
        },
    );
    assert!(error_of(&send(&state, 5, too_large).await).contains("MEMSYNC_CONTENT_TOO_LARGE"));
    let bad_cursor = send(&state, 6, list(Some("not-a-cursor".to_string()))).await;
    assert!(error_of(&bad_cursor).contains("MEMSYNC_CURSOR_INVALID"));

    // A resolution of a candidate that does not exist answers the same bytes
    // whether the document exists or not.
    let resolve = |id: &str, doc: &str| {
        let mut method = sync(
            id,
            SyncAction::Resolve {
                candidate_revision: 9,
                expected_head_revision: 1,
                resolution: Resolution::KeepHead,
            },
        );
        if let Method::SyncMemorySnapshot { request, .. } = &mut method {
            request.doc_id = DocId::new(doc).expect("doc id");
        }
        method
    };
    let existing = send(&state, 7, resolve("n4", DOC)).await;
    let absent = send(&state, 8, resolve("n5", "notes/absent.md")).await;
    assert!(error_of(&existing).contains("MEMSYNC_NOT_AVAILABLE"), "{existing:?}");
    assert_eq!(error_of(&existing), error_of(&absent));
    assert_eq!(existing.error_detail, absent.error_detail);
    // A creation attempt that names a revision of an absent document is the same refusal.
    let mut create = submit("n6", Some(4), "new");
    if let Method::SyncMemorySnapshot { request, .. } = &mut create {
        request.doc_id = DocId::new("notes/absent.md").expect("doc id");
    }
    assert_eq!(error_of(&send(&state, 9, create).await), error_of(&absent));

    // A foreign tenant is refused and changes nothing.
    let mut foreign = common::context_for(common::TEST_AGENT);
    foreign.tenant = "tenant-foreign".to_string();
    let cross = test_support::dispatch(
        &state,
        common::signed_request_with_context(
            SECRET,
            10,
            GRAPH,
            foreign,
            submit("n7", Some(1), "foreign"),
        ),
    )
    .await;
    assert!(cross.error.is_some(), "{cross:?}");
    // A writer without the write scope is refused and changes nothing.
    let mut reader = common::context_for(common::TEST_AGENT);
    reader.scopes = vec!["memory:read".to_string()];
    let denied = test_support::dispatch(
        &state,
        common::signed_request_with_context(
            SECRET,
            11,
            GRAPH,
            reader,
            submit("n8", Some(1), "denied"),
        ),
    )
    .await;
    assert!(denied.error.is_some(), "{denied:?}");

    assert_eq!(head_bytes(&state, 12).await, before, "no refusal wrote anything");
    let none: Response = read(&state, 13, Some(2)).await;
    assert!(none.error.is_some(), "no second revision exists");
}

/// Reopen `dir` after the first engine let go of its durable file lock.
async fn restart(
    dir: &std::path::Path,
    backend: test_support::SharedPersistence,
    state: test_support::SharedState,
) -> test_support::SharedState {
    state.write().await.persistence = None;
    drop(state);
    test_support::reopen_after_sole_reference(
        backend,
        || Ok::<(), String>(()),
        "release the durable backend before the restart",
    )
    .await;
    test_support::durable_state_read_through(
        SECRET,
        common::current_isolation(),
        &dir.to_string_lossy(),
    )
    .await
}

fn acknowledge(request_id: &str, revision: u64, content: &str, generation: u64) -> Method {
    sync(
        request_id,
        SyncAction::Acknowledge {
            revision,
            content_digest: Digest256::sha256(content.as_bytes()),
            observation_generation: generation,
        },
    )
}

fn standing_of(outcome: SyncOutcome) -> SyncStanding {
    match outcome {
        SyncOutcome::Acknowledged { standing } | SyncOutcome::Observed { standing, .. } => standing,
        other => panic!("expected a standing, got {other:?}"),
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R085.6
#[tokio::test(flavor = "multi_thread")]
async fn provenance_and_materialization_ack_survive_restart() {
    install_knobs();
    let (dir, backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "memory-sync-ack",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let first: SyncOutcome = json_of(send(&state, 2, submit("a1", None, "one")).await);
    assert!(matches!(first, SyncOutcome::Accepted { .. }));
    // Matching revision, digest and generation is in sync; a wrong digest is pending.
    let ok = standing_of(json_of(send(&state, 3, acknowledge("a2", 1, "one", 5)).await));
    assert_eq!(ok, SyncStanding::InSync);
    let wrong = standing_of(json_of(send(&state, 4, acknowledge("a3", 1, "other", 6)).await));
    assert_eq!(wrong, SyncStanding::Pending);
    // An acknowledgment older than the latest observation is pending.
    let stale = standing_of(json_of(send(&state, 5, acknowledge("a4", 1, "one", 2)).await));
    assert_eq!(stale, SyncStanding::Pending);
    // An unresolved conflict is a conflict, whatever the acknowledgment says.
    let _: SyncOutcome = json_of(send(&state, 6, submit("a5", None, "rival")).await);
    let conflict = standing_of(json_of(send(&state, 7, acknowledge("a6", 1, "one", 9)).await));
    assert_eq!(conflict, SyncStanding::Conflict);
    let head: MemorySnapshotRead = json_of(read(&state, 8, None).await);
    assert_eq!(head.head.provenance.request_id.as_str(), "a1");
    assert_eq!(head.head.provenance.actor, common::TEST_AGENT);
    assert_eq!(head.head.parent_revision, None);

    let state = restart(&dir, backend, state).await;
    let after: MemorySnapshotRead = json_of(read(&state, 10, None).await);
    assert_eq!(after.head, head.head, "provenance survives the restart");
    // The retained candidate is durable, so the standing is still a conflict.
    let stale = standing_of(json_of(send(&state, 11, acknowledge("a7", 1, "one", 3)).await));
    assert_eq!(stale, SyncStanding::Conflict, "the conflict survives the restart");
}

// spec: MEDULLA-DURABLE-KERNEL-R085.7
#[tokio::test(flavor = "multi_thread")]
async fn conflicts_survive_gc_and_retention_refusal() {
    install_knobs();
    let (dir, backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "memory-sync-retention",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let first: SyncOutcome = json_of(send(&state, 2, submit("c1", None, "one")).await);
    assert!(matches!(first, SyncOutcome::Accepted { .. }));
    let stale: SyncOutcome = json_of(send(&state, 3, submit("c2", None, "rival")).await);
    let SyncOutcome::ConflictRetained {
        candidate_revision, ..
    } = stale
    else {
        panic!("a stale save is retained");
    };

    // A restart keeps the head and the unresolved candidate: resolving it still works.
    let state = restart(&dir, backend, state).await;
    let head: MemorySnapshotRead = json_of(read(&state, 4, None).await);
    assert_eq!((head.head.revision, head.content.as_str()), (1, "one"));
    let resolve = admitted(
        &state,
        sync(
            "c3",
            SyncAction::Resolve {
                candidate_revision,
                expected_head_revision: 1,
                resolution: Resolution::TakeCandidate,
            },
        ),
        "rival",
        1,
        medulla_decision_types::decision::memory_benchmark::ChangeKind::Consolidate,
    )
    .await;
    let resolved: SyncOutcome = json_of(send(&state, 5, resolve).await);
    assert!(matches!(resolved, SyncOutcome::Resolved { ref head } if head.revision == 2));

    // A document that fills the tenant's capacity is accepted; one more byte
    // anywhere is refused before any effect.
    let big = "x".repeat(1_048_576);
    let fill: SyncOutcome = json_of(send(&state, 6, submit("c4", Some(2), &big)).await);
    assert!(matches!(fill, SyncOutcome::Accepted { ref head } if head.revision == 3));
    let mut other = submit("c5", None, "tiny");
    if let Method::SyncMemorySnapshot { request, .. } = &mut other {
        request.doc_id = DocId::new("notes/b.md").expect("doc id");
    }
    let refused = send(&state, 7, other).await;
    assert!(
        error_of(&refused).contains("MEMSYNC_RETENTION_EXHAUSTED"),
        "{refused:?}"
    );
    let missing = send(
        &state,
        8,
        Method::ReadMemorySnapshot {
            doc_id: "notes/b.md".to_string(),
            revision: None,
        },
    )
    .await;
    assert!(missing.error.is_some(), "the refused write left no document");
    let after: MemorySnapshotRead = json_of(read(&state, 9, None).await);
    assert_eq!(after.head.revision, 3);
}

async fn send_as(
    state: &test_support::SharedState,
    agent: &str,
    id: u64,
    graph: &str,
    method: Method,
) -> Response {
    test_support::dispatch(
        state,
        common::signed_request_as(SECRET, id, graph, agent, method),
    )
    .await
}

// spec: MEDULLA-DURABLE-KERNEL-R085.8
#[tokio::test(flavor = "multi_thread")]
async fn complete_memory_sync_journey() {
    install_knobs();
    let (dir, backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "memory-sync-journey",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    // Two independent clients of one tenant.
    let (alpha, beta) = ("client-alpha", "client-beta");

    // Read and list an empty store.
    let absent = send_as(&state, alpha, 2, GRAPH, Method::ReadMemorySnapshot {
        doc_id: DOC.to_string(),
        revision: None,
    })
    .await;
    assert!(absent.error.is_some(), "nothing to read yet");
    let empty: medulla_mutation_types::memory_sync::MemorySnapshotList =
        json_of(send_as(&state, alpha, 3, GRAPH, list(None)).await);
    assert!(empty.heads.is_empty());

    // Two competing saves from the same base: one head, one retained candidate.
    let first: SyncOutcome =
        json_of(send_as(&state, alpha, 4, GRAPH, submit("j1", None, "alpha text")).await);
    assert!(matches!(first, SyncOutcome::Accepted { ref head } if head.revision == 1));
    let replay: SyncOutcome =
        json_of(send_as(&state, alpha, 5, GRAPH, submit("j1", None, "alpha text")).await);
    assert_eq!(replay, first, "a replayed request id answers the same outcome");
    let rival: SyncOutcome =
        json_of(send_as(&state, beta, 6, GRAPH, submit("j2", None, "beta text")).await);
    let SyncOutcome::ConflictRetained {
        candidate_revision, ..
    } = rival
    else {
        panic!("the second competing save is retained, not lost: {rival:?}");
    };

    // The conflict reads as such, and one resolution settles it once.
    let conflict = standing_of(json_of(
        send_as(&state, beta, 7, GRAPH, acknowledge("j3", 1, "alpha text", 1)).await,
    ));
    assert_eq!(conflict, SyncStanding::Conflict);
    let resolve = admitted(
        &state,
        sync(
            "j4",
            SyncAction::Resolve {
                candidate_revision,
                expected_head_revision: 1,
                resolution: Resolution::TakeCandidate,
            },
        ),
        "beta text",
        1,
        medulla_decision_types::decision::memory_benchmark::ChangeKind::Consolidate,
    )
    .await;
    let resolved: SyncOutcome = json_of(send_as(&state, alpha, 8, GRAPH, resolve.clone()).await);
    assert!(matches!(resolved, SyncOutcome::Resolved { ref head } if head.revision == 2));
    assert!(
        send_as(&state, beta, 9, GRAPH, resolve).await.error.is_some(),
        "a resolved candidate resolves once"
    );

    // Acknowledgment: in sync once revision, digest and generation match.
    let synced = standing_of(json_of(
        send_as(&state, beta, 10, GRAPH, acknowledge("j5", 2, "beta text", 2)).await,
    ));
    assert_eq!(synced, SyncStanding::InSync);

    // Negatives: another tenant, another graph, a tampered cursor, a writer
    // without the write scope. None changes the head.
    let mut foreign = common::context_for(beta);
    foreign.tenant = "tenant-foreign".to_string();
    let cross = test_support::dispatch(
        &state,
        common::signed_request_with_context(
            SECRET,
            11,
            GRAPH,
            foreign,
            submit("j6", Some(2), "foreign"),
        ),
    )
    .await;
    assert!(cross.error.is_some(), "{cross:?}");
    let wrong_graph = send_as(&state, alpha, 12, "other-graph", submit("j7", Some(2), "x")).await;
    assert!(wrong_graph.error.is_some(), "{wrong_graph:?}");
    let stale = send_as(&state, alpha, 13, GRAPH, list(Some("stale-cursor".to_string()))).await;
    assert!(error_of(&stale).contains("MEMSYNC_CURSOR_INVALID"));
    let mut reader = common::context_for(alpha);
    reader.scopes = vec!["memory:read".to_string()];
    let denied = test_support::dispatch(
        &state,
        common::signed_request_with_context(
            SECRET,
            14,
            GRAPH,
            reader,
            submit("j8", Some(2), "denied"),
        ),
    )
    .await;
    assert!(denied.error.is_some(), "{denied:?}");

    // Restart, then each client reads back the same history independently.
    let state = restart(&dir, backend, state).await;
    for (id, agent) in [(20, alpha), (21, beta)] {
        let head: MemorySnapshotRead = json_of(
            send_as(&state, agent, id, GRAPH, Method::ReadMemorySnapshot {
                doc_id: DOC.to_string(),
                revision: None,
            })
            .await,
        );
        assert_eq!((head.head.revision, head.content.as_str()), (2, "beta text"));
        let old: MemorySnapshotRead = json_of(
            send_as(&state, agent, id + 10, GRAPH, Method::ReadMemorySnapshot {
                doc_id: DOC.to_string(),
                revision: Some(1),
            })
            .await,
        );
        assert_eq!(old.content, "alpha text");
    }
    let listing: medulla_mutation_types::memory_sync::MemorySnapshotList =
        json_of(send_as(&state, beta, 40, GRAPH, Method::ListMemorySnapshots {
            prefix: "notes/".to_string(),
            limit: Some(10),
            cursor: None,
        })
        .await);
    assert_eq!(listing.heads.len(), 1);
    assert_eq!(listing.heads[0].revision, 2);
}
