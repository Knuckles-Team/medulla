//! Round-trip and bound contract of the memory snapshot sync types
//! (MEDULLA-DURABLE-KERNEL-R085.1) and the pure compare-and-set core
//! (MEDULLA-DURABLE-KERNEL-R085.3).

use medulla_types::contract::{Digest256, ServerErrorCode};
use medulla_types::memory_sync::{
    decide, Decision, DocId, ListCursor, MemorySnapshotHead, MemorySyncConfig, MemorySyncRefusal,
    RequestId, Resolution, SnapshotProvenance, SyncAction, SyncOutcome, SyncRequest, SyncState,
    MAX_CONTENT_BYTES,
};
use serde::de::DeserializeOwned;
use serde::Serialize;

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T) {
    let json = serde_json::to_string(value).expect("serializes");
    let back: T = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(&back, value);
}

fn head(revision: u64, content: &str) -> MemorySnapshotHead {
    MemorySnapshotHead {
        doc_id: DocId::new("notes/today.md").unwrap(),
        revision,
        parent_revision: revision.checked_sub(1).filter(|parent| *parent > 0),
        content_digest: Digest256::sha256(content.as_bytes()),
        content_len: content.len() as u32,
        graph_incarnation: "inc-1".into(),
        provenance: SnapshotProvenance {
            actor: "principal:a".into(),
            policy_digest: Digest256::sha256(b"policy"),
            request_id: RequestId::new("req-1").unwrap(),
        },
        retain_until_ms: None,
    }
}

fn submit(expected: Option<u64>, content: &str) -> SyncRequest {
    SyncRequest {
        request_id: RequestId::new("req-2").unwrap(),
        doc_id: DocId::new("notes/today.md").unwrap(),
        action: SyncAction::Submit {
            expected_revision: expected,
            content: content.into(),
            content_digest: Digest256::sha256(content.as_bytes()),
        },
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R085.1
#[test]
fn bounded_contract_round_trips_and_refuses_invalid_fields() {
    let first = head(1, "one");
    first.validate().unwrap();
    round_trip(&first);
    round_trip(&head(2, "two"));
    round_trip(&ListCursor::new("cursor-token").unwrap());
    round_trip(&MemorySyncConfig::default());
    for state in [
        SyncState::InSync,
        SyncState::Pending,
        SyncState::Conflict,
        SyncState::Unavailable,
    ] {
        round_trip(&state);
    }
    let requests = [
        submit(None, "one"),
        SyncRequest {
            request_id: RequestId::new("req-3").unwrap(),
            doc_id: DocId::new("notes/today.md").unwrap(),
            action: SyncAction::ObserveFile {
                observed_digest: Digest256::sha256(b"one"),
            },
        },
        SyncRequest {
            request_id: RequestId::new("req-4").unwrap(),
            doc_id: DocId::new("notes/today.md").unwrap(),
            action: SyncAction::Resolve {
                candidate_revision: 3,
                expected_head_revision: 2,
                resolution: Resolution::Merged {
                    content: "merged".into(),
                    content_digest: Digest256::sha256(b"merged"),
                },
            },
        },
        SyncRequest {
            request_id: RequestId::new("req-5").unwrap(),
            doc_id: DocId::new("notes/today.md").unwrap(),
            action: SyncAction::Acknowledge {
                revision: 2,
                content_digest: Digest256::sha256(b"two"),
                observation_generation: 7,
            },
        },
    ];
    for request in &requests {
        request.validate().unwrap();
        round_trip(request);
    }
    let outcomes = [
        SyncOutcome::Accepted {
            head: first.clone(),
        },
        SyncOutcome::ConflictRetained {
            head: first.clone(),
            candidate_revision: 2,
        },
        SyncOutcome::Resolved {
            head: first.clone(),
        },
        SyncOutcome::Observed {
            state: SyncState::InSync,
            head_revision: 1,
            head_digest: first.content_digest,
        },
        SyncOutcome::Acknowledged {
            state: SyncState::Pending,
        },
        SyncOutcome::Refused {
            code: MemorySyncRefusal::NotAvailable,
        },
    ];
    for outcome in &outcomes {
        round_trip(outcome);
    }
    let refused = serde_json::to_value(&outcomes[5]).unwrap();
    assert_eq!(refused["code"], "MEMSYNC_NOT_AVAILABLE");
    assert!(refused.get("head").is_none(), "Refused holds no head field");

    // Each bound refuses.
    assert!(DocId::new("").is_err());
    assert!(DocId::new("Upper").is_err());
    assert!(DocId::new("a b").is_err());
    assert!(DocId::new("a".repeat(256)).is_ok());
    assert!(DocId::new("a".repeat(257)).is_err());
    assert!(RequestId::new("").is_err());
    assert!(RequestId::new("r".repeat(128)).is_ok());
    assert!(RequestId::new("r".repeat(129)).is_err());
    assert!(ListCursor::new("").is_err());
    assert!(serde_json::from_str::<DocId>("\"BAD\"").is_err());
    let mut too_long = first.clone();
    too_long.content_len = MAX_CONTENT_BYTES as u32 + 1;
    assert_eq!(
        too_long.validate().unwrap_err().code,
        MemorySyncRefusal::ContentTooLarge
    );
    let mut bad_parent = first.clone();
    bad_parent.parent_revision = Some(1);
    assert!(bad_parent.validate().is_err());
    let huge = "x".repeat(MAX_CONTENT_BYTES + 1);
    assert_eq!(
        submit(None, &huge).validate().unwrap_err().code,
        MemorySyncRefusal::ContentTooLarge
    );
    let mut mismatch = submit(None, "one");
    if let SyncAction::Submit { content, .. } = &mut mismatch.action {
        *content = "other".into();
    }
    assert_eq!(
        mismatch.validate().unwrap_err().code,
        MemorySyncRefusal::DigestMismatch
    );
    let mut config = MemorySyncConfig::default();
    config.validate().unwrap();
    config.cursor_ttl_s = 59;
    assert!(config.validate().is_err());
    config = MemorySyncConfig::default();
    config.max_revisions_per_doc = 4097;
    assert!(config.validate().is_err());
    assert!(serde_json::from_str::<SyncOutcome>(
        r#"{"outcome":"refused","code":"MEMSYNC_NOT_AVAILABLE","head_revision":1}"#
    )
    .is_err());
    assert!(
        serde_json::from_str::<SyncOutcome>(r#"{"outcome":"refused","code":"NOT_A_CODE"}"#)
            .is_err()
    );
}

#[test]
fn every_memory_sync_refusal_is_a_registered_server_error_code() {
    for code in MemorySyncRefusal::ALL {
        assert!(code.as_str().starts_with("MEMSYNC_"));
        assert!(ServerErrorCode::ALL.contains(&code.server_code()));
        round_trip(code);
    }
}

#[test]
fn decide_accepts_on_the_expected_revision_and_retains_stale_submits() {
    let current = head(2, "two");
    // Absent document, no expectation: first revision.
    assert!(matches!(
        decide(None, &submit(None, "one")),
        Decision::Accept {
            revision: 1,
            parent_revision: None,
            ..
        }
    ));
    // Expected equals head: next revision with the old head as parent.
    assert!(matches!(
        decide(Some(&current), &submit(Some(2), "three")),
        Decision::Accept {
            revision: 3,
            parent_revision: Some(2),
            ..
        }
    ));
    // Same digest as the head: accepted, no new revision.
    assert_eq!(
        decide(Some(&current), &submit(Some(2), "two")),
        Decision::Reply(SyncOutcome::Accepted {
            head: current.clone()
        })
    );
    // Stale: content retained, head unchanged.
    assert!(matches!(
        decide(Some(&current), &submit(Some(1), "stale")),
        Decision::RetainCandidate {
            head_revision: 2,
            ..
        }
    ));
    // Creation with a stale expectation is not available.
    assert_eq!(
        decide(None, &submit(Some(1), "x")),
        Decision::Reply(SyncOutcome::Refused {
            code: MemorySyncRefusal::NotAvailable
        })
    );
}
