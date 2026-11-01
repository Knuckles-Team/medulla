//! Terminal work-item retry through the store's mutation-batch commit path.
//! It lives with the facade because the terminal fixture method is shared with
//! the transport retry tests in `crate::test_support`.

use medulla_mutation_types::mutation_scope::DurabilityDomain;
use medulla_mutation_types::mutation_scope::MutationSurface;
use medulla_protocol_types::mutation_batch::VersionExpectation;
use medulla_protocol_types::mutation_batch::{MutationBatch, MutationOperation};
use medulla_protocol_types::protocol::Method;
use medulla_store::redb_store::mutation_fixtures::{
    batch, commit_at, commit_native_claim, open, ready_work_item_method,
};
use medulla_store::redb_store::{
    decode_durable, fixture_operation_envelope, read_mutation_graph_version, read_one_node,
    DurableCrypto,
};

fn temp_path(tag: &str) -> std::path::PathBuf {
    medulla_store::redb_store::temp_path("medulla-mutation-batch", tag)
}

#[test]
fn terminal_work_item_retry_replays_and_conflicting_payload_fails_closed() {
    let path = temp_path("work-item-terminal-replay");
    let db = open(&path);

    let mut seed = batch("work-item-terminal-seed", "work-item-terminal-seed-key");
    seed.operations = vec![MutationOperation {
        ordinal: 0,
        surface: MutationSurface::Graph,
        domain: DurabilityDomain::GraphRows,
        method: ready_work_item_method("work-1", 3),
    }];
    seed.reseal_envelope(medulla_types::contract::Digest256::from_bytes([1_u8; 32]))
        .expect("terminal seed fixture reseals its final body");
    commit_at(&db, &seed, None).unwrap();
    let claimed = commit_native_claim(
        &db,
        "work-item-terminal-claim",
        "work-item-terminal-claim-key",
        4,
        Some("work-1"),
        "worker-a",
        0,
        60_000,
        64,
    );
    assert!(claimed.claimed);
    assert_eq!(claimed.lease_epoch, Some(1));
    assert_eq!(claimed.fencing_token, Some(1));

    let terminal_method = medulla_server_api::test_support::terminal_method(1, 1);
    let mut terminal = batch(
        "work:terminal-stable-batch",
        "work-idem:terminal-stable-key",
    );
    // The attempt metadata this used to re-stamp -- request id, purpose,
    // policy fingerprint, trace id -- is either gone or structurally outside
    // the stable replay identity now, so a fixture that wants a distinct
    // request simply re-mints the envelope for it.
    terminal.envelope = fixture_operation_envelope(
        &terminal.identity,
        &format!("principal:sha256:{}", "a".repeat(64)),
        777,
        "work-idem:terminal-stable-key",
    );
    let remint_attempt = |batch: &mut MutationBatch, nonce_byte: u8, created_at_ms: u64| {
        let medulla_protocol_types::mutation_batch::MutationEnvelope::Operation(operation) =
            &mut batch.envelope
        else {
            panic!("authenticated replay fixture must carry an operation envelope");
        };
        operation.authority.nonce = medulla_types::contract::Nonce::from_bytes([nonce_byte; 32]);
        operation.authority.context_digest = operation
            .authority
            .recompute_context_digest()
            .expect("fixture authority context remains valid after nonce rotation");
        batch.created_at_ms = created_at_ms;
    };
    // `CommitWorkItemResult` is a `native_terminal_work_item_cas` batch:
    // `check_occ_version_and_fence` never checks its expectation against
    // the authoritative version (the WorkItem lease/fencing token is its
    // real CAS guard), but `VersionExpectation` no longer has a "none"
    // arm to encode that -- 5 is simply the actual current graph version
    // at this point (seed 3->4, claim 4->5), matching real state rather
    // than an invented placeholder.
    terminal.version_expectation = VersionExpectation::Graph(5);
    terminal.operations = vec![MutationOperation {
        ordinal: 0,
        surface: MutationSurface::Job,
        domain: DurabilityDomain::ControlPlane,
        method: terminal_method,
    }];
    terminal.outbox[0].key = terminal.batch_id.clone();
    terminal
        .reseal_envelope(medulla_types::contract::Digest256::from_bytes([1_u8; 32]))
        .expect("terminal fixture reseals its final body");

    let first = commit_at(&db, &terminal, None).unwrap();
    assert!(!first.replayed);
    assert_eq!(read_mutation_graph_version(&db, "graph-a").unwrap(), 6);

    let consumed_nonce = commit_at(&db, &terminal, None).unwrap_err();
    assert!(
        consumed_nonce.contains("REPLAY_NONCE_CONSUMED"),
        "{consumed_nonce}"
    );

    // A fresh transport request is normalized to the same durable request id
    // before this kernel sees it. Re-mint its attempt nonce while preserving
    // the stable operation key and body, then replay the stored result.
    let mut retry = terminal.clone();
    retry.envelope = fixture_operation_envelope(
        &retry.identity,
        &format!("principal:sha256:{}", "a".repeat(64)),
        778,
        "work-idem:terminal-stable-key",
    );
    retry
        .reseal_envelope(medulla_types::contract::Digest256::from_bytes([1_u8; 32]))
        .expect("terminal retry fixture reseals its final body");
    assert_ne!(
        retry
            .envelope
            .operation()
            .expect("retry carries an operation envelope")
            .authority
            .nonce,
        terminal
            .envelope
            .operation()
            .expect("original carries an operation envelope")
            .authority
            .nonce,
        "a retry must use a fresh attempt nonce"
    );
    retry.created_at_ms = 200;
    let replay = commit_at(&db, &retry, None).unwrap();
    assert!(replay.replayed);
    assert_eq!(read_mutation_graph_version(&db, "graph-a").unwrap(), 6);

    let mut conflicting_payload = retry.clone();
    let Method::CommitWorkItemResult { result_ref, .. } =
        &mut conflicting_payload.operations[0].method
    else {
        unreachable!();
    };
    *result_ref = Some("result:sha256:different".into());
    remint_attempt(&mut conflicting_payload, 0x43, 300);
    conflicting_payload
        .reseal_envelope(medulla_types::contract::Digest256::from_bytes([1_u8; 32]))
        .unwrap();
    let error = commit_at(&db, &conflicting_payload, None).unwrap_err();
    assert!(error.contains("IDEMPOTENCY_CONFLICT"));

    // Planted known-bad input: the same key under a DIFFERENT actor. The
    // actor is inside the stable operation identity, so this is a named
    // conflict rather than a silent replay -- the cross-actor
    // replay-ownership property the M1 review raised as a P1.
    let mut conflicting_authority = retry;
    let key = conflicting_authority.idempotency_key().to_string();
    let identity = conflicting_authority.identity.clone();
    conflicting_authority.envelope = fixture_operation_envelope(
        &identity,
        &format!("principal:sha256:{}", "b".repeat(64)),
        42,
        &key,
    );
    conflicting_authority
        .reseal_envelope(medulla_types::contract::Digest256::from_bytes([1_u8; 32]))
        .expect("conflicting authority fixture reseals its final body");
    let error = commit_at(&db, &conflicting_authority, None).unwrap_err();
    assert!(error.contains("IDEMPOTENCY_CONFLICT"));
    assert_eq!(read_mutation_graph_version(&db, "graph-a").unwrap(), 6);

    let stored = read_one_node(&db, "graph-a", "work-1", DurableCrypto::none())
        .unwrap()
        .unwrap();
    let stored: serde_json::Value = decode_durable(&stored).unwrap();
    assert_eq!(stored["status"], "succeeded");
    assert_eq!(stored["result_ref"], "result:sha256:one");
    drop(db);
    let _ = std::fs::remove_file(path);
}
