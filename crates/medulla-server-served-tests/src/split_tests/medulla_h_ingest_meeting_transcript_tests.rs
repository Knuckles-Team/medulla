//! MEDULLA-MEDIA-VOCAB-R001.101.3 / .4 / .5 through the real request
//! boundary: signed `MeetingTranscriptRevision`, `MeetingSummaryContext` and
//! `MeetingSummaryCommit` requests under a tenant and principal context, each
//! committed through the one ChangeEnvelope authority, and every refusal
//! leaving the store unchanged.

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_protocol_types::protocol::{Method, Request, Response, ResultPayload};
use medulla_server_api::test_support::decode_raw_response as ok;
use medulla_server_auth::auth::compute_verified_envelope_token;
use medulla_server_auth::auth::VerifiedEnvelopeParams;
use medulla_types::acl::RequestContextClaims;
use medulla_types::contract::Digest256;
use medulla_types::meeting_transcript::{
    MeetingSummaryCommitReceipt, MeetingSummaryCommitRequest, MeetingSummaryContext,
    MeetingSummaryContextRequest, MeetingSummaryLimitPolicy, MeetingTranscriptRevision,
    MeetingTranscriptRevisionReceipt, SummaryAdmissionRef, SummaryCitation, SummaryClaim,
    SummaryReceipt, SummarySegmentText, TranscriptRevisionOp,
};

/// Synthetic HMAC fixture for `ServerState::new_for_test`; never a live credential.
const SECRET: &str = "meeting-transcript-served-fixture-secret"; // sanitizer:ignore
/// The fixed `cfg(test)` request-context tenant.
const TENANT: &str = "tenant-shared";
const PRINCIPAL: &str = "meeting-transcript-writer";
const MEETING: &str = "meeting-1";

fn state(dir: &std::path::Path) -> Arc<RwLock<medulla_server_api::state::ServerState>> {
    let mut state = medulla_server::seams_install::new_state_for_test(
        SECRET,
        medulla_isolation::isolation::IsolationLayer::new(),
    );
    state.persist_dir = Some(dir.to_string_lossy().into_owned());
    Arc::new(RwLock::new(state))
}

fn signed(id: u64, method: Method) -> Request {
    let context = RequestContextClaims {
        principal: PRINCIPAL.to_string(),
        tenant: TENANT.to_string(),
        audience: "medulla-test".to_string(),
        agent_id: PRINCIPAL.to_string(),
        scopes: vec!["*".to_string()],
        delegation: Vec::new(),
        policy_version: "policy-test".to_string(),
        ..RequestContextClaims::default()
    };
    let mut request = Request {
        id,
        graph: TENANT.to_string(),
        auth_token: String::new(),
        agent_id: Some(PRINCIPAL.to_string()),
        method,
    };
    request.auth_token = compute_verified_envelope_token(
        SECRET,
        &request,
        &VerifiedEnvelopeParams {
            context: &context,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_secs(),
            nonce: &format!("meeting-transcript-nonce-{id}"),
            idempotency_key: &format!("meeting-transcript-key-{id}"),
        },
    );
    request
}

async fn call(
    state: &Arc<RwLock<medulla_server_api::state::ServerState>>,
    id: u64,
    method: Method,
) -> Response {
    medulla_server_router::dispatch::request_boundary::dispatch(state, signed(id, method)).await
}

fn refusal(label: &str, response: &Response) -> String {
    assert!(
        response.result.is_none() || response.error.is_some(),
        "{label} was accepted"
    );
    format!("{:?} {:?}", response.error, response.error_detail)
}

/// Everything a refusal must leave untouched: the head row and every row of
/// the context, summary and claim types.
async fn snapshot(state: &Arc<RwLock<medulla_server_api::state::ServerState>>, id: u64) -> String {
    let mut out = format!(
        "{:?}",
        call(
            state,
            id,
            Method::GetNodeProperties {
                node_id: format!("meeting-transcript:{MEETING}"),
            },
        )
        .await
        .result
    );
    for (offset, label) in [
        "SummaryContext",
        "MeetingSummary",
        "SummaryClaim",
        "TranscriptSegment",
    ]
    .iter()
    .enumerate()
    {
        let rows = call(
            state,
            id + 1 + offset as u64,
            Method::GetNodesByLabel {
                label: (*label).to_string(),
                after: None,
                limit: 1_000,
            },
        )
        .await;
        out.push_str(&format!("|{label}:{:?}", rows.result));
    }
    out
}

fn text_of(segment: &str) -> String {
    format!("Segment {segment} says the plan ships on Friday with two reviewers.")
}

fn event(
    op: TranscriptRevisionOp,
    base: u64,
    sequence: u64,
    segments: &[&str],
    text: &str,
) -> Method {
    Method::MeetingTranscriptRevision {
        request: Box::new(MeetingTranscriptRevision {
            meeting_id: MEETING.into(),
            op,
            base_revision: base,
            new_revision: base + 1,
            monotonic_event_sequence: sequence,
            idempotency_key: format!("event-{sequence}"),
            ordered_segment_ids: segments.iter().map(|s| s.to_string()).collect(),
            start_ms: sequence * 1_000,
            end_ms: sequence * 1_000 + 900,
            speaker_ref: Some("speaker:a".into()),
            text_digest: Digest256::sha256(text.as_bytes()),
            provenance_ref: "recording:1".into(),
            finalization_digest: None,
        }),
    }
}

/// Append s1, append s2, correct s1, finalize. Returns the sealed digest.
async fn finalized_transcript(
    state: &Arc<RwLock<medulla_server_api::state::ServerState>>,
) -> Digest256 {
    let steps = [
        (
            TranscriptRevisionOp::Append,
            0,
            1,
            "s1",
            text_of("s1-draft"),
        ),
        (TranscriptRevisionOp::Append, 1, 2, "s2", text_of("s2")),
        (TranscriptRevisionOp::Correct, 2, 3, "s1", text_of("s1")),
    ];
    for (index, (op, base, sequence, segment, text)) in steps.into_iter().enumerate() {
        let receipt: MeetingTranscriptRevisionReceipt = ok(
            "revision",
            call(
                state,
                10 + index as u64,
                event(op, base, sequence, &[segment], &text),
            )
            .await,
        );
        assert_eq!(receipt.revision, base + 1);
        assert!(!receipt.replayed);
    }
    let sealed: MeetingTranscriptRevisionReceipt = ok(
        "finalize",
        call(
            state,
            20,
            event(TranscriptRevisionOp::Finalize, 3, 4, &[], ""),
        )
        .await,
    );
    assert_eq!(sealed.revision, 4);
    sealed
        .finalization_digest
        .expect("finalize answers the sealed digest")
}

fn admission() -> SummaryAdmissionRef {
    SummaryAdmissionRef {
        provider: "local".into(),
        model: "summarizer-1".into(),
        policy_revision: "policy-7".into(),
        admission_digest: SummaryAdmissionRef::digest_of("local", "summarizer-1", "policy-7"),
    }
}

fn context_request(digest: Digest256, policy: MeetingSummaryLimitPolicy) -> Method {
    Method::MeetingSummaryContext {
        request: Box::new(MeetingSummaryContextRequest {
            meeting_id: MEETING.into(),
            transcript_digest: digest,
            admission: Some(admission()),
            policy,
            segments: ["s1", "s2"]
                .iter()
                .map(|s| SummarySegmentText {
                    segment_id: s.to_string(),
                    text: text_of(s),
                })
                .collect(),
        }),
    }
}

const POLICY: MeetingSummaryLimitPolicy = MeetingSummaryLimitPolicy {
    max_spans: 8,
    max_segment_bytes: 40,
    max_tokens: 512,
};

// spec: MEDULLA-MEDIA-VOCAB-R001.101.3
#[tokio::test]
async fn served_append_correct_finalize() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let sealed = finalized_transcript(&state).await;

    // An exact replay answers the stored revision and writes nothing.
    let before = snapshot(&state, 100).await;
    let replay: MeetingTranscriptRevisionReceipt = ok(
        "replay",
        call(
            &state,
            110,
            event(TranscriptRevisionOp::Append, 1, 2, &["s2"], &text_of("s2")),
        )
        .await,
    );
    assert!(replay.replayed);
    assert_eq!(replay.revision, 2);
    assert_eq!(snapshot(&state, 120).await, before);

    // A stale base refuses by its code with an unchanged store.
    let stale = call(
        &state,
        130,
        event(TranscriptRevisionOp::Append, 1, 9, &["s3"], "x"),
    )
    .await;
    assert!(refusal("stale base", &stale).contains("TRANSCRIPT_BASE_CONFLICT"));
    assert_eq!(snapshot(&state, 140).await, before);

    // The sealed digest is read back separately from the stored head.
    let head = call(
        &state,
        150,
        Method::GetNodeProperties {
            node_id: format!("meeting-transcript:{MEETING}"),
        },
    )
    .await;
    let Some(ResultPayload::Raw(bytes)) = head.result else {
        panic!("the transcript head is stored");
    };
    let properties = medulla_types::msgpack::decode_property_value(&bytes).unwrap();
    assert_eq!(
        properties["final_digest"],
        serde_json::json!(sealed.to_hex())
    );
    assert_eq!(properties["final_revision"], serde_json::json!("4"));
}

// spec: MEDULLA-MEDIA-VOCAB-R001.101.4
#[tokio::test]
async fn context_is_bounded_and_refuses_before_write() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());

    // Not final yet.
    let one = event(TranscriptRevisionOp::Append, 0, 1, &["s1"], &text_of("s1"));
    let _: MeetingTranscriptRevisionReceipt = ok("append", call(&state, 1, one).await);
    let draft = Digest256::sha256(b"not sealed");
    let before = snapshot(&state, 200).await;
    let not_final = call(&state, 210, context_request(draft, POLICY)).await;
    assert!(refusal("not final", &not_final).contains("TRANSCRIPT_NOT_FINAL"));
    assert_eq!(snapshot(&state, 220).await, before);

    let two = event(TranscriptRevisionOp::Append, 1, 2, &["s2"], &text_of("s2"));
    let _: MeetingTranscriptRevisionReceipt = ok("append", call(&state, 2, two).await);
    let sealed: MeetingTranscriptRevisionReceipt = ok(
        "finalize",
        call(
            &state,
            3,
            event(TranscriptRevisionOp::Finalize, 2, 3, &[], ""),
        )
        .await,
    );
    let sealed = sealed.finalization_digest.unwrap();
    let before = snapshot(&state, 300).await;

    let zero = MeetingSummaryLimitPolicy {
        max_spans: 0,
        ..POLICY
    };
    let over = MeetingSummaryLimitPolicy {
        max_tokens: 8_193,
        ..POLICY
    };
    for (id, method, code) in [
        (310, context_request(sealed, zero), "SUMMARY_LIMIT_INVALID"),
        (320, context_request(sealed, over), "SUMMARY_LIMIT_EXCEEDED"),
        (330, context_request(draft, POLICY), "TRANSCRIPT_STALE"),
    ] {
        let response = call(&state, id, method).await;
        assert!(refusal(code, &response).contains(code), "{code}");
        assert_eq!(snapshot(&state, id + 5).await, before, "{code} wrote");
    }
    let Method::MeetingSummaryContext { mut request } = context_request(sealed, POLICY) else {
        unreachable!()
    };
    request.admission = None;
    let missing = call(&state, 340, Method::MeetingSummaryContext { request }).await;
    assert!(refusal("admission", &missing).contains("SUMMARY_ADMISSION_MISSING"));
    assert_eq!(snapshot(&state, 345).await, before);

    let context: MeetingSummaryContext = ok(
        "context",
        call(&state, 350, context_request(sealed, POLICY)).await,
    );
    assert_eq!(context.spans.len(), 2);
    assert!(context.spans.iter().all(|span| span.text.len() <= 40));
    assert_eq!(context.citations.len(), 2);
    assert_eq!(context.policy_revision, "policy-7");
    assert_ne!(
        snapshot(&state, 360).await,
        before,
        "the context node is written"
    );

    // A later revision makes the sealed digest stale.
    let three = event(TranscriptRevisionOp::Append, 3, 4, &["s3"], "late");
    let _: MeetingTranscriptRevisionReceipt = ok("late append", call(&state, 370, three).await);
    let stale = call(&state, 380, context_request(sealed, POLICY)).await;
    assert!(refusal("stale", &stale).contains("TRANSCRIPT_STALE"));
}

fn receipt(context: &MeetingSummaryContext, provider: &str) -> SummaryReceipt {
    let mut receipt = SummaryReceipt {
        owner_requirement_id: "MEDULLA-MEDIA-VOCAB-R001.101.5".into(),
        receipt_id: "receipt-1".into(),
        receipt_digest: Digest256::sha256(b"placeholder"),
        provider: provider.into(),
        model: "summarizer-1".into(),
        policy_revision: "policy-7".into(),
        input_context_digest: context.context_digest,
        output_digest: Digest256::sha256(b"summary output"),
    };
    receipt.receipt_digest = receipt.expected_digest();
    receipt
}

fn commit_request(receipt: SummaryReceipt, span: &str, end: u32) -> Method {
    Method::MeetingSummaryCommit {
        request: Box::new(MeetingSummaryCommitRequest {
            meeting_id: MEETING.into(),
            summary_id: "summary-1".into(),
            claims: vec![SummaryClaim {
                claim_id: "c1".into(),
                text: "The plan ships on Friday.".into(),
                citations: vec![SummaryCitation {
                    span_id: span.into(),
                    start: 0,
                    end,
                }],
            }],
            receipt,
        }),
    }
}

// spec: MEDULLA-MEDIA-VOCAB-R001.101.5
#[tokio::test]
async fn commit_validates_receipt_and_citations() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let sealed = finalized_transcript(&state).await;
    let context: MeetingSummaryContext = ok(
        "context",
        call(&state, 400, context_request(sealed, POLICY)).await,
    );
    let span = context.spans[0].span_id.clone();
    let before = snapshot(&state, 410).await;

    let mut unknown = receipt(&context, "local");
    unknown.input_context_digest = Digest256::sha256(b"no such context");
    unknown.receipt_digest = unknown.expected_digest();
    let mut forged = receipt(&context, "local");
    forged.receipt_digest = Digest256::sha256(b"forged");
    for (id, method, code) in [
        (420, commit_request(unknown, &span, 10), "RECEIPT_UNKNOWN"),
        (
            430,
            commit_request(receipt(&context, "other"), &span, 10),
            "RECEIPT_MISMATCH",
        ),
        (440, commit_request(forged, &span, 10), "RECEIPT_MISMATCH"),
        (
            450,
            commit_request(receipt(&context, "local"), &span, 41),
            "CITATION_OUTSIDE_TEXT",
        ),
        (
            460,
            commit_request(receipt(&context, "local"), "s9#0", 10),
            "CITATION_OUTSIDE_TEXT",
        ),
    ] {
        let response = call(&state, id, method).await;
        assert!(refusal(code, &response).contains(code), "{code}");
        assert_eq!(snapshot(&state, id + 5).await, before, "{code} wrote");
    }

    let committed: MeetingSummaryCommitReceipt = ok(
        "commit",
        call(
            &state,
            470,
            commit_request(receipt(&context, "local"), &span, 10),
        )
        .await,
    );
    assert_eq!(committed.claim_count, 1);
    assert_eq!(committed.citation_count, 1);
    assert_eq!(committed.transcript_digest, sealed);
    assert_ne!(
        snapshot(&state, 480).await,
        before,
        "summary and claim nodes are written"
    );

    // A revision after the seal makes a later commit stale.
    let late = event(TranscriptRevisionOp::Append, 4, 5, &["s3"], "late");
    let _: MeetingTranscriptRevisionReceipt = ok("late", call(&state, 490, late).await);
    let after = snapshot(&state, 500).await;
    let stale = call(
        &state,
        510,
        commit_request(receipt(&context, "local"), &span, 10),
    )
    .await;
    assert!(refusal("stale", &stale).contains("TRANSCRIPT_STALE"));
    assert_eq!(snapshot(&state, 520).await, after);
}
