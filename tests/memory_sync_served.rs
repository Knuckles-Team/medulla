//! MEDULLA-DURABLE-KERNEL-R085: served memory snapshot sync through the real
//! `dispatch` shell over a redb-backed `ServerState`.

#![cfg(feature = "redb")]

mod common;
#[cfg(feature = "blob")]
#[path = "common/failable_blob.rs"]
mod failable_blob;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_mutation_types::memory_sync::{
    DocId, MemorySnapshotRead, RequestId, Resolution, SyncAction, SyncOutcome, SyncReceipt,
    SyncRequest, SyncStanding,
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
            doc_kind: Default::default(),
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
                doc_kind: Default::default(),
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
            doc_kind: Default::default(),
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
    assert!(
        forged_doc.error.is_some(),
        "the forged row was never written"
    );
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
    for (id, digests) in [(3, Vec::new()), (4, vec!["sha256:absent".to_string()])] {
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
    assert!(
        error_of(&existing).contains("MEMSYNC_NOT_AVAILABLE"),
        "{existing:?}"
    );
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

    assert_eq!(
        head_bytes(&state, 12).await,
        before,
        "no refusal wrote anything"
    );
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
    let ok = standing_of(json_of(
        send(&state, 3, acknowledge("a2", 1, "one", 5)).await,
    ));
    assert_eq!(ok, SyncStanding::InSync);
    let wrong = standing_of(json_of(
        send(&state, 4, acknowledge("a3", 1, "other", 6)).await,
    ));
    assert_eq!(wrong, SyncStanding::Pending);
    // An acknowledgment older than the latest observation is pending.
    let stale = standing_of(json_of(
        send(&state, 5, acknowledge("a4", 1, "one", 2)).await,
    ));
    assert_eq!(stale, SyncStanding::Pending);
    // An unresolved conflict is a conflict, whatever the acknowledgment says.
    let _: SyncOutcome = json_of(send(&state, 6, submit("a5", None, "rival")).await);
    let conflict = standing_of(json_of(
        send(&state, 7, acknowledge("a6", 1, "one", 9)).await,
    ));
    assert_eq!(conflict, SyncStanding::Conflict);
    let head: MemorySnapshotRead = json_of(read(&state, 8, None).await);
    assert_eq!(head.head.provenance.request_id.as_str(), "a1");
    assert_eq!(head.head.provenance.actor, common::TEST_AGENT);
    assert_eq!(head.head.parent_revision, None);

    let state = restart(&dir, backend, state).await;
    let after: MemorySnapshotRead = json_of(read(&state, 10, None).await);
    assert_eq!(after.head, head.head, "provenance survives the restart");
    // The retained candidate is durable, so the standing is still a conflict.
    let stale = standing_of(json_of(
        send(&state, 11, acknowledge("a7", 1, "one", 3)).await,
    ));
    assert_eq!(
        stale,
        SyncStanding::Conflict,
        "the conflict survives the restart"
    );
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
    #[cfg(feature = "blob")]
    let blob = failable_blob::attach(&state, &dir).await;
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
    assert!(
        missing.error.is_some(),
        "the refused write left no document"
    );
    let after: MemorySnapshotRead = json_of(read(&state, 9, None).await);
    assert_eq!(after.head.revision, 3);

    // A collector pass reclaims nothing the snapshots hold: the resolved
    // revision's bytes and the capacity-filling head are still in the CAS.
    #[cfg(feature = "blob")]
    {
        use medulla_server_blob::blob::ChunkStore;
        blob.sweep().expect("the collector pass");
        for content in ["rival", big.as_str()] {
            let digest = Digest256::sha256(content.as_bytes()).to_hex();
            assert!(
                blob.chunk(&digest).is_some(),
                "a collector pass reclaimed held snapshot content"
            );
        }
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R085.4
#[cfg(feature = "blob")]
#[tokio::test(flavor = "multi_thread")]
async fn a_storage_failure_refuses_before_commit_and_a_holder_failure_warns() {
    install_knobs();
    let (dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "memory-sync-failable",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let blob = failable_blob::attach(&state, &dir).await;

    // The content cannot reach the CAS: the sync refuses before anything commits.
    blob.fail_put(true);
    let refused = send(&state, 2, submit("f1", None, "one")).await;
    assert!(
        error_of(&refused).contains("MEMSYNC_STORAGE_UNAVAILABLE"),
        "{refused:?}"
    );
    assert!(
        read(&state, 3, None).await.error.is_some(),
        "nothing committed"
    );

    // The content reaches the CAS but the holder cannot be registered: the write
    // is committed, so the answer is a success carrying the receipt warning.
    blob.fail_put(false);
    blob.fail_holder(true);
    let receipt: SyncReceipt = json_of(send(&state, 4, submit("f2", None, "one")).await);
    assert!(matches!(receipt.outcome, SyncOutcome::Accepted { ref head } if head.revision == 1));
    assert_eq!(receipt.warnings, vec!["MEMSYNC_HOLDER_PENDING".to_string()]);
    let head: MemorySnapshotRead = json_of(read(&state, 5, None).await);
    assert_eq!((head.head.revision, head.content.as_str()), (1, "one"));

    // A clean sync carries no warning.
    blob.fail_holder(false);
    let rewrite = admitted(
        &state,
        submit("f3", Some(1), "two"),
        "two",
        1,
        medulla_decision_types::decision::memory_benchmark::ChangeKind::Rewrite,
    )
    .await;
    let clean: SyncReceipt = json_of(send(&state, 6, rewrite).await);
    assert!(clean.warnings.is_empty());
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
    let absent = send_as(
        &state,
        alpha,
        2,
        GRAPH,
        Method::ReadMemorySnapshot {
            doc_id: DOC.to_string(),
            revision: None,
        },
    )
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
    assert_eq!(
        replay, first,
        "a replayed request id answers the same outcome"
    );
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
        send_as(
            &state,
            beta,
            7,
            GRAPH,
            acknowledge("j3", 1, "alpha text", 1),
        )
        .await,
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
        send_as(&state, beta, 9, GRAPH, resolve)
            .await
            .error
            .is_some(),
        "a resolved candidate resolves once"
    );

    // Acknowledgment: in sync once revision, digest and generation match.
    let synced = standing_of(json_of(
        send_as(
            &state,
            beta,
            10,
            GRAPH,
            acknowledge("j5", 2, "beta text", 2),
        )
        .await,
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
    let stale = send_as(
        &state,
        alpha,
        13,
        GRAPH,
        list(Some("stale-cursor".to_string())),
    )
    .await;
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
            send_as(
                &state,
                agent,
                id,
                GRAPH,
                Method::ReadMemorySnapshot {
                    doc_id: DOC.to_string(),
                    revision: None,
                },
            )
            .await,
        );
        assert_eq!(
            (head.head.revision, head.content.as_str()),
            (2, "beta text")
        );
        let old: MemorySnapshotRead = json_of(
            send_as(
                &state,
                agent,
                id + 10,
                GRAPH,
                Method::ReadMemorySnapshot {
                    doc_id: DOC.to_string(),
                    revision: Some(1),
                },
            )
            .await,
        );
        assert_eq!(old.content, "alpha text");
    }
    let listing: medulla_mutation_types::memory_sync::MemorySnapshotList = json_of(
        send_as(
            &state,
            beta,
            40,
            GRAPH,
            Method::ListMemorySnapshots {
                prefix: "notes/".to_string(),
                limit: Some(10),
                cursor: None,
            },
        )
        .await,
    );
    assert_eq!(listing.heads.len(), 1);
    assert_eq!(listing.heads[0].revision, 2);
}

// ---- MEDULLA-DECISION-ENGINE-R149: a saved search state over the served sync ----

/// A search-state body at `state_revision` for run `run`.
fn search_state_body(run: &str, state_revision: u64, marker: &str) -> String {
    serde_json::json!({
        "search_run_id": run,
        "state_revision": state_revision,
        "transcript": [{"turn": marker}],
        "evidence": [{"url": "https://example.test/a", "marker": marker}],
        "constraints": ["no paid sources"],
        "open_claims": [{"claim": "unverified", "marker": marker}],
    })
    .to_string()
}

fn submit_state(request_id: &str, expected: Option<u64>, content: &str) -> Method {
    let mut method = submit(request_id, expected, content);
    if let Method::SyncMemorySnapshot { request, .. } = &mut method {
        request.doc_kind = medulla_mutation_types::memory_sync::MemoryDocKind::SearchState;
    }
    method
}

/// Save revision `revision` of the search state of `run`, with the receipts a
/// rewrite over an existing head needs.
async fn save_state(
    state: &test_support::SharedState,
    id: u64,
    run: &str,
    revision: u64,
) -> (SyncOutcome, String) {
    let body = search_state_body(run, revision, &format!("m{revision}"));
    let expected = revision.checked_sub(1).filter(|parent| *parent > 0);
    let mut method = submit_state(&format!("s{revision}-{id}"), expected, &body);
    if let Some(head) = expected {
        method = admitted(
            state,
            method,
            &body,
            head,
            medulla_decision_types::decision::memory_benchmark::ChangeKind::Rewrite,
        )
        .await;
    }
    (json_of(send(state, id, method).await), body)
}

// spec: MEDULLA-DECISION-ENGINE-R149.3
#[tokio::test(flavor = "multi_thread")]
async fn search_state_wire_reaches_the_gateway() {
    install_knobs();
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "memory-sync-search-state",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let (first, body) = save_state(&state, 2, "run-1", 1).await;
    assert!(matches!(first, SyncOutcome::Accepted { ref head } if head.revision == 1));
    let stored: MemorySnapshotRead = json_of(read(&state, 3, None).await);
    assert_eq!(stored.content, body, "the stored bytes are the sent bytes");
    assert_eq!(stored.head.revision, 1);
    assert_eq!(
        stored.head.doc_kind,
        medulla_mutation_types::memory_sync::MemoryDocKind::SearchState
    );

    // Another tenant reads the document as absent and cannot overwrite it.
    let mut foreign = common::context_for(common::TEST_AGENT);
    foreign.tenant = "tenant-foreign".to_string();
    let theirs = test_support::dispatch(
        &state,
        common::signed_request_with_context(
            SECRET,
            4,
            GRAPH,
            foreign.clone(),
            Method::ReadMemorySnapshot {
                doc_id: DOC.to_string(),
                revision: None,
            },
        ),
    )
    .await;
    assert!(
        theirs.error.is_some(),
        "a foreign tenant reads nothing: {theirs:?}"
    );
    let overwrite = test_support::dispatch(
        &state,
        common::signed_request_with_context(
            SECRET,
            5,
            GRAPH,
            foreign,
            submit_state("s-foreign", Some(1), &search_state_body("run-1", 2, "x")),
        ),
    )
    .await;
    assert!(overwrite.error.is_some(), "{overwrite:?}");
    assert_eq!(head_bytes(&state, 6).await, (1, body));
}

// spec: MEDULLA-DECISION-ENGINE-R149.4
#[tokio::test(flavor = "multi_thread")]
async fn incomplete_state_refuses() {
    install_knobs();
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "memory-sync-search-state-refusals",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let (first, body) = save_state(&state, 2, "run-2", 1).await;
    assert!(matches!(first, SyncOutcome::Accepted { .. }));
    let before = head_bytes(&state, 3).await;

    let without = |field: &str| {
        let mut value: serde_json::Value = serde_json::from_str(&body).unwrap();
        value["state_revision"] = 2.into();
        value.as_object_mut().unwrap().remove(field);
        value.to_string()
    };
    let mut cases: Vec<(String, &str)> = [
        "search_run_id",
        "transcript",
        "evidence",
        "constraints",
        "open_claims",
    ]
    .iter()
    .map(|field| (without(field), "SEARCH_STATE_INCOMPLETE"))
    .collect();
    cases.push(("[1,2]".to_string(), "SEARCH_STATE_MALFORMED"));
    cases.push((
        search_state_body("", 2, "m").to_string(),
        "SEARCH_STATE_MALFORMED",
    ));
    cases.push((
        search_state_body("run-2", 7, "m").to_string(),
        "SEARCH_STATE_REVISION_MISMATCH",
    ));
    let oversized = serde_json::json!({
        "search_run_id": "run-2",
        "state_revision": 2,
        "transcript": ["x".repeat(1_048_576)],
        "evidence": [],
        "constraints": [],
        "open_claims": [],
    })
    .to_string();
    cases.push((oversized, "SEARCH_STATE_TOO_LARGE"));

    for (index, (content, code)) in cases.iter().enumerate() {
        let method = admitted(
            &state,
            submit_state(&format!("bad-{index}"), Some(1), content),
            content,
            1,
            medulla_decision_types::decision::memory_benchmark::ChangeKind::Rewrite,
        )
        .await;
        let refused = send(&state, 10 + index as u64, method).await;
        assert!(
            error_of(&refused).contains(code),
            "{index}: {code}: {refused:?}"
        );
        assert_eq!(
            head_bytes(&state, 40).await,
            before,
            "{code} leaves the head"
        );
    }

    // A stale expected revision keeps the head and answers the R085 conflict.
    let (second, _) = save_state(&state, 60, "run-2", 2).await;
    assert!(matches!(second, SyncOutcome::Accepted { ref head } if head.revision == 2));
    let stale: SyncOutcome = json_of(
        send(
            &state,
            61,
            submit_state("stale", Some(1), &search_state_body("run-2", 2, "late")),
        )
        .await,
    );
    assert!(matches!(stale, SyncOutcome::ConflictRetained { ref head, .. } if head.revision == 2));

    // A foreign tenant's invalid write is refused and changes nothing.
    let mut foreign = common::context_for(common::TEST_AGENT);
    foreign.tenant = "tenant-foreign".to_string();
    let cross = test_support::dispatch(
        &state,
        common::signed_request_with_context(
            SECRET,
            62,
            GRAPH,
            foreign,
            submit_state("foreign-bad", Some(2), "[]"),
        ),
    )
    .await;
    assert!(cross.error.is_some(), "{cross:?}");
    assert_eq!(head_bytes(&state, 63).await.0, 2);
}

// spec: MEDULLA-DECISION-ENGINE-R149.5
#[tokio::test(flavor = "multi_thread")]
async fn state_evidence_lists_four_digests() {
    install_knobs();
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "memory-sync-search-state-evidence",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let (saved, body) = save_state(&state, 2, "run-3", 1).await;
    assert!(matches!(saved, SyncOutcome::Accepted { .. }));
    let read: MemorySnapshotRead = json_of(read(&state, 3, None).await);
    let evidence = read
        .search_state
        .expect("a search state carries its evidence");
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    let digest = |field: &str| {
        let bytes = serde_json::to_vec(&value[field]).unwrap();
        Digest256::sha256(&bytes).to_hex()
    };
    assert_eq!(evidence.state_revision, 1);
    assert_eq!(evidence.transcript_digest, digest("transcript"));
    assert_eq!(evidence.evidence_digest, digest("evidence"));
    assert_eq!(evidence.constraints_digest, digest("constraints"));
    assert_eq!(evidence.open_claims_digest, digest("open_claims"));
    assert!(evidence.recovered_at_ms > 0);
    assert_eq!(
        evidence.premises,
        vec![
            "search:run=run-3".to_string(),
            "search:revision=1".to_string(),
            format!("search:transcript={}", digest("transcript")),
            format!("search:evidence={}", digest("evidence")),
            format!("search:constraints={}", digest("constraints")),
            format!("search:open_claims={}", digest("open_claims")),
        ]
    );

    // A plain note carries no search-state evidence.
    let note_doc = "notes/plain.md";
    let mut note = submit("plain", None, "text");
    if let Method::SyncMemorySnapshot { request, .. } = &mut note {
        request.doc_id = DocId::new(note_doc).unwrap();
    }
    let _: SyncOutcome = json_of(send(&state, 4, note).await);
    let plain: MemorySnapshotRead = json_of(
        send(
            &state,
            5,
            Method::ReadMemorySnapshot {
                doc_id: note_doc.to_string(),
                revision: None,
            },
        )
        .await,
    );
    assert!(plain.search_state.is_none());
}

// spec: MEDULLA-DECISION-ENGINE-R149.6
#[tokio::test(flavor = "multi_thread")]
async fn search_state_survives_context_eviction() {
    install_knobs();
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "memory-sync-search-state-eviction",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    // Client A saves a state at revisions 1 and 2, then drops its own copy.
    let (one, _) = save_state(&state, 2, "run-4", 1).await;
    assert!(matches!(one, SyncOutcome::Accepted { .. }));
    let (two, saved) = save_state(&state, 3, "run-4", 2).await;
    assert!(matches!(two, SyncOutcome::Accepted { ref head } if head.revision == 2));

    // Client B reads revision 2 and gets the bytes, the four arrays and the digests.
    let read: MemorySnapshotRead = json_of(read(&state, 4, Some(2)).await);
    assert_eq!(read.content, saved, "byte-identical readback");
    let envelope: medulla_work_types::memory::SearchStateEnvelope =
        serde_json::from_str(&read.content).unwrap();
    assert_eq!(envelope.transcript.len(), 1);
    assert_eq!(envelope.evidence.len(), 1);
    assert_eq!(envelope.constraints.len(), 1);
    assert_eq!(envelope.open_claims.len(), 1);
    let evidence = read.search_state.expect("evidence on the read");
    assert_eq!(
        evidence.transcript_digest,
        envelope.transcript_digest().to_hex()
    );
    assert_eq!(evidence.state_revision, 2);

    // A stale save against revision 1 answers the conflict outcome.
    let stale: SyncOutcome = json_of(
        send(
            &state,
            5,
            submit_state("late", Some(1), &search_state_body("run-4", 2, "late")),
        )
        .await,
    );
    assert!(matches!(stale, SyncOutcome::ConflictRetained { ref head, .. } if head.revision == 2));
}

// ---- MEDULLA-DECISION-ENGINE-R150: session notes captured through Learn, recalled with flags ----

const NOTE_TEXT: &str = "deploy runbook";

fn session_note(
    id: &str,
    run: &str,
    output: u8,
    source_revision: u64,
) -> medulla_work_types::memory::SessionNoteBody {
    medulla_work_types::memory::SessionNoteBody {
        schema_version: 1,
        note_id: id.to_string(),
        run_id: run.to_string(),
        session_id: "session-1".to_string(),
        task_id: "task-1".to_string(),
        user_id: "user-a".to_string(),
        input_digest: Digest256::from_bytes([1; 32]),
        output_digest: Digest256::from_bytes([output; 32]),
        source_revision,
        content: NOTE_TEXT.to_string(),
    }
}

/// Store the receipt of a run, as `RunOutcome` would, with its recorded success.
async fn store_run_receipt(state: &test_support::SharedState, run: &str, success: bool) {
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;
    let store = state
        .write()
        .await
        .ensure_agent_library()
        .expect("the agent library store");
    let receipt = medulla_decision_types::decision::statistical::run_credit::RunCreditReceipt {
        run_id: run.to_string(),
        decision_ids: medulla_types::contract::BoundedVec::new(Vec::new()).unwrap(),
        outcome_digest: "ab".repeat(32),
        positive_count: u32::from(success),
        negative_count: u32::from(!success),
        success,
        steps: medulla_types::contract::BoundedVec::new(Vec::new()).unwrap(),
        premises: medulla_types::contract::BoundedVec::new(Vec::new()).unwrap(),
        receipt_digest: format!("receipt-{run}"),
    };
    store
        .put_decision_artifacts(
            common::TEST_TENANT,
            &[(
                format!("run_credit/run/{run}"),
                medulla_persistence_records::decision_jobs::encode_artifact(&receipt).unwrap(),
            )],
        )
        .expect("store the run receipt");
}

fn capture(note: medulla_work_types::memory::SessionNoteBody) -> Method {
    Method::DecisionLog {
        op: Box::new(medulla_decision_types::decision::statistical::log::DecisionLogOp::Learn {
            tenant_id: common::TEST_TENANT.to_string(),
            write: medulla_decision_types::decision::statistical::retrieval::LearningWrite::RecordSessionNote {
                graph: GRAPH.to_string(),
                note: Box::new(note),
            },
        }),
    }
}

fn recall_notes(min_relevance: Option<f64>, current_revision: Option<u64>) -> Method {
    let query = medulla_server_ops::api::ingest::embedder::EmbedderConfig::from_env()
        .embed(&[NOTE_TEXT])
        .expect("the hash embedder answers")
        .remove(0);
    Method::MemoryRecall {
        request: Box::new(medulla_work_types::memory::MemoryRecallRequest {
            query_embedding: query,
            keywords: Vec::new(),
            tiers: Vec::new(),
            scope: None,
            as_of_ms: None,
            budget: 10_000,
            prime_session: None,
            include_plan: false,
            include_media: false,
            previous_manifest_digest: None,
            document_set_ids: Vec::new(),
            min_relevance,
            include_archived: false,
            current_revision,
            episode_scope: None,
            memory_scope: None,
        }),
    }
}

async fn session_note_state(label: &str) -> (std::path::PathBuf, test_support::SharedState) {
    install_knobs();
    let (dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        label,
        common::current_isolation(),
        GRAPH,
    )
    .await;
    (dir, state)
}

// spec: MEDULLA-DECISION-ENGINE-R150.3, MEDULLA-DECISION-ENGINE-R150.2
#[tokio::test(flavor = "multi_thread")]
async fn session_notes_recalled_with_flags() {
    let (_dir, state) = session_note_state("memory-sync-session-notes").await;
    store_run_receipt(&state, "run-1", true).await;
    store_run_receipt(&state, "run-2", true).await;
    store_run_receipt(&state, "run-bad", false).await;

    // Two notes of one input with different outputs conflict; a third names an older revision.
    for (id, note) in [
        (2, session_note("note-a", "run-1", 2, 5)),
        (3, session_note("note-b", "run-2", 3, 5)),
        (4, session_note("note-c", "run-1", 2, 4)),
    ] {
        let response = send(&state, id, capture(note)).await;
        assert!(response.error.is_none(), "{response:?}");
    }
    // A failed run captures nothing.
    let refused = send(&state, 5, capture(session_note("note-d", "run-bad", 9, 5))).await;
    assert!(
        error_of(&refused).contains("NOTE_RUN_NOT_SUCCESSFUL"),
        "{refused:?}"
    );

    let recalled: medulla_work_types::memory::MemoryRecallResult =
        json_of(send(&state, 6, recall_notes(None, Some(5))).await);
    let ids: Vec<&str> = recalled.notes.iter().map(|n| n.note_id.as_str()).collect();
    assert!(
        !ids.contains(&"note-d"),
        "the refused note was never stored"
    );
    let view = |id: &str| recalled.notes.iter().find(|n| n.note_id == id).expect(id);
    // The two notes of input one with different outputs conflict and are not applied.
    assert!(view("note-a").conflict && view("note-b").conflict);
    assert!(!view("note-a").applied && !view("note-b").applied);
    // The note recorded against revision 4 is stale at revision 5 and stays visible.
    assert!(view("note-c").stale && !view("note-a").stale);
    assert!(recalled
        .hits
        .iter()
        .any(|hit| hit.record.memory_id.as_str() == "note-c"));
    assert_eq!(view("note-a").relevance_q4, 10_000);

    // A relevance floor outside [0, 1] refuses by name.
    let out_of_range = send(&state, 7, recall_notes(Some(1.5), None)).await;
    assert!(
        error_of(&out_of_range).contains("NOTE_RELEVANCE_OUT_OF_RANGE"),
        "{out_of_range:?}"
    );

    // Another tenant recalls none of these notes.
    let mut foreign = common::context_for(common::TEST_AGENT);
    foreign.tenant = "tenant-foreign".to_string();
    let theirs = test_support::dispatch(
        &state,
        common::signed_request_with_context(SECRET, 8, GRAPH, foreign, recall_notes(None, None)),
    )
    .await;
    let seen: medulla_work_types::memory::MemoryRecallResult = match theirs.error {
        None => json_of(theirs),
        Some(_) => return,
    };
    assert!(seen.notes.is_empty() && seen.hits.is_empty());
}

// spec: MEDULLA-DECISION-ENGINE-R150.3
#[tokio::test(flavor = "multi_thread")]
async fn session_note_age_boundaries_follow_the_engine_clock() {
    const DAY_MS: u64 = 86_400_000;
    let (_dir, state) = session_note_state("memory-sync-session-note-age").await;
    store_run_receipt(&state, "run-1", true).await;
    let response = send(&state, 2, capture(session_note("note-aged", "run-1", 2, 5))).await;
    assert!(response.error.is_none(), "{response:?}");

    // Recall with the engine clock moved forward; no test sleeps on the wall clock.
    let recall_after = |id: u64, offset_ms: u64, include_archived: bool| {
        let state = &state;
        async move {
            let mut method = recall_notes(None, Some(5));
            if let Method::MemoryRecall { request } = &mut method {
                request.include_archived = include_archived;
            }
            let response = medulla_kernel::kernel::dispatch_support::with_engine_clock_offset(
                offset_ms,
                send(state, id, method),
            )
            .await;
            json_of::<medulla_work_types::memory::MemoryRecallResult>(response)
        }
    };

    // Under 7 days the note is fresh.
    let fresh = recall_after(3, 7 * DAY_MS - 3_600_000, false).await;
    assert!(!fresh.notes[0].stale && !fresh.notes[0].archived && fresh.notes[0].applied);

    // At 8 days it is stale, still applied and still visible.
    let stale = recall_after(4, 8 * DAY_MS, false).await;
    assert!(stale.notes[0].stale && !stale.notes[0].archived && stale.notes[0].applied);

    // Just under 14 days it is not archived; past 14 days it is archived and out of recall.
    let almost = recall_after(5, 14 * DAY_MS - 3_600_000, false).await;
    assert!(almost.notes[0].stale && !almost.notes[0].archived);
    let archived = recall_after(6, 14 * DAY_MS + 3_600_000, false).await;
    assert!(
        archived.notes.is_empty(),
        "an archived note stays out of recall"
    );
    assert!(!archived
        .hits
        .iter()
        .any(|hit| hit.record.memory_id.as_str() == "note-aged"));

    // The request that asks for archived notes sees it flagged and not applied; nothing is deleted.
    let kept = recall_after(7, 14 * DAY_MS + 3_600_000, true).await;
    assert!(kept.notes[0].archived && !kept.notes[0].applied && kept.notes[0].stale);
}

// spec: MEDULLA-DECISION-ENGINE-R150.5
#[tokio::test(flavor = "multi_thread")]
async fn session_note_evidence_lists_flags() {
    let (_dir, state) = session_note_state("memory-sync-session-note-evidence").await;
    store_run_receipt(&state, "run-1", true).await;
    store_run_receipt(&state, "run-2", true).await;
    let mut lone = session_note("note-lone", "run-1", 2, 5);
    lone.input_digest = Digest256::from_bytes([9; 32]);
    let response = send(&state, 2, capture(lone)).await;
    assert!(response.error.is_none(), "{response:?}");

    let recalled: medulla_work_types::memory::MemoryRecallResult =
        json_of(send(&state, 3, recall_notes(None, Some(5))).await);
    let view = &recalled.notes[0];
    assert!(view.applied && !view.stale && !view.conflict);
    let has = |fact: String| recalled.note_premises.contains(&fact);
    assert!(has("note:id=note-lone".to_string()));
    assert!(has("note:revision=1".to_string()));
    assert!(has("note:run=run-1".to_string()));
    assert!(has("note:relevance_q4=10000".to_string()));
    assert!(has("note:stale=false".to_string()));
    assert!(has("note:conflict=false".to_string()));
    assert!(recalled
        .note_premises
        .iter()
        .any(|p| p.starts_with("note:digest=")));
    assert!(recalled
        .note_premises
        .iter()
        .any(|p| p.starts_with("knob:memory.note_min_relevance=")));

    // A stale and a conflicting note are listed with their flags and no applied premises.
    for (id, note) in [
        (4, session_note("note-old", "run-1", 7, 4)),
        (5, session_note("note-rival-1", "run-1", 6, 5)),
        (6, session_note("note-rival-2", "run-2", 8, 5)),
    ] {
        let response = send(&state, id, capture(note)).await;
        assert!(response.error.is_none(), "{response:?}");
    }
    let again: medulla_work_types::memory::MemoryRecallResult =
        json_of(send(&state, 7, recall_notes(None, Some(5))).await);
    let flags = |id: &str| {
        again
            .notes
            .iter()
            .find(|n| n.note_id == id)
            .unwrap()
            .clone()
    };
    assert!(flags("note-old").stale);
    assert!(flags("note-rival-1").conflict && flags("note-rival-2").conflict);
    assert!(!again
        .note_premises
        .contains(&"note:id=note-rival-1".to_string()));
    assert!(again
        .note_premises
        .contains(&"note:id=note-lone".to_string()));
}
