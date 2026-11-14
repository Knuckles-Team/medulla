//! MEDULLA-FEDERATED-QUERY-R081.8 and R083.3: the served retrieval round with two
//! tenants in ONE store (TENANT-SCOPE-TEST-SHAPE).
//!
//! A deployment admits one tenant at the door, so the isolation proof stands the
//! store up once, writes both tenants' documents into it, and dispatches each
//! tenant's verified context through the real dispatch chokepoint
//! (`dispatch_verified_request`). Each tenant's feedback and arbitration read only
//! its own documents, and a round writes nothing. The door itself is checked
//! separately: a carrier signed for a foreign tenant is refused
//! `AUTH_TENANT_MISMATCH`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use medulla_exec::algebra::{Op, Plan};
use medulla_protocol_types::protocol::{GraphType, Method, Request, Response, ResultPayload};
use medulla_result_types::retrieval_arbitration::{
    AnswerGrounding, ArbitrationChoice, Candidate, Citation, Claim, SupportState,
};
use medulla_result_types::retrieval_round::{
    sha256_hex_of, ArbitrationPart, BudgetUsage, CallerLimits, RoundFeedback, RoundRequest,
    RoundResult, StopRequest,
};
use medulla_result_types::retrieval_stop::CalibrationTable;
use medulla_result_types::retrieval_verdict::{
    DocumentRelevance, FeedbackLimits, FeedbackTier, RetrievalFeedback, TermFeedback,
    RETRIEVAL_FEEDBACK_INVALID,
};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_server_auth::auth::{dispatch_test_on_heap, scoped_test_request_on, ScopedTestCaller};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

const SECRET: &str = "retrieval-round-two-tenant-secret"; // sanitizer:ignore
/// The fixed tenant the served test auth policy admits at the door.
const TENANT: &str = "tenant-shared";
/// A second tenant whose documents live in the same store.
const OTHER_TENANT: &str = "tenant-other";
/// The System-role agent the harness isolation registers.
const ADMIN: &str = "retrieval-round-two-tenant-admin";
const COMMONS: &str = "__commons__";

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

struct Served {
    _dir: tempfile::TempDir,
    state: Arc<RwLock<ServerState>>,
}

impl Served {
    /// One store holding `tenant-shared`'s documents `doc-a-*` and `tenant-other`'s
    /// documents `doc-b-*`, each placed in its tenant's graph by `_tenant`.
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut state = medulla_server::seams_install::new_state_for_test(
            SECRET,
            ServerState::test_isolation(ADMIN),
        );
        state.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        let graphs = dir.path().join("graphs");
        std::fs::create_dir_all(&graphs).unwrap();
        state.persistence = Some(Arc::new(
            medulla_persistence::redb_backend::RedbBackend::open(
                graphs.to_string_lossy().into_owned(),
                64,
            )
            .expect("open the served graph backend"),
        ));
        let served = Self {
            _dir: dir,
            state: Arc::new(RwLock::new(state)),
        };
        {
            let mut guard = served.state.write().await;
            if guard.registry.get(COMMONS).is_none() {
                guard
                    .registry
                    .create_graph(COMMONS, GraphType::Team, None)
                    .unwrap();
            }
        }
        for (prefix, tenant) in [("a", TENANT), ("b", OTHER_TENANT)] {
            for index in 0..3 {
                served
                    .add_doc(
                        &format!("doc-{prefix}-{index}"),
                        &format!("entry {prefix}{index}"),
                        tenant,
                    )
                    .await;
            }
        }
        served
    }

    async fn add_doc(&self, id: &str, text: &str, tenant: &str) {
        let call_id = next_id();
        let request = scoped_test_request_on(
            SECRET,
            call_id,
            COMMONS,
            Method::AddNode {
                node_id: id.to_string(),
                properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                    "type": "Doc", "text": text, "_tenant": tenant,
                }))
                .unwrap(),
            },
            ScopedTestCaller {
                principal: ADMIN,
                tenant: TENANT,
                scopes: &["*"],
                nonce: &format!("two-tenant-nonce-{call_id}"),
                idempotency_key: &format!("two-tenant-seed-{call_id}"),
            },
        );
        let response = dispatch_test_on_heap(&self.state, request).await;
        assert!(
            response.error.is_none(),
            "seed failed: {:?}",
            response.error
        );
    }

    /// The commons graph's version: a round that wrote nothing leaves it alone.
    async fn version(&self) -> u64 {
        let guard = self.state.read().await;
        guard.registry.get(COMMONS).unwrap().core.version()
    }

    /// One round as `tenant`'s verified context, through the dispatch chokepoint.
    async fn round_as(&self, tenant: &str, round: RoundRequest) -> Response {
        let request = Request {
            id: next_id(),
            graph: COMMONS.into(),
            auth_token: String::new(),
            agent_id: Some(ADMIN.into()),
            method: Method::RetrievalRound {
                request: Box::new(round),
            },
        };
        let verified =
            VerifiedRequestContext::verified_for_test_with_scopes(ADMIN, tenant, &["kg:read"]);
        Box::pin(
            medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
                &self.state,
                request,
                verified,
            ),
        )
        .await
    }

    /// The same round signed for `tenant` and sent through the door.
    async fn round_at_door(&self, tenant: &str, round: RoundRequest) -> Response {
        let call_id = next_id();
        let request = scoped_test_request_on(
            SECRET,
            call_id,
            COMMONS,
            Method::RetrievalRound {
                request: Box::new(round),
            },
            ScopedTestCaller {
                principal: ADMIN,
                tenant,
                scopes: &["*"],
                nonce: &format!("two-tenant-door-nonce-{call_id}"),
                idempotency_key: &format!("two-tenant-door-{call_id}"),
            },
        );
        dispatch_test_on_heap(&self.state, request).await
    }
}

fn request() -> RoundRequest {
    RoundRequest {
        plan: Plan::new(vec![Op::Scan {
            label: "Doc".into(),
        }]),
        corpus_revision: "corpus-1".into(),
        model_revision: "model-1".into(),
        sufficiency: None,
        selection: None,
        hierarchy: None,
        stop: None,
        state: None,
        feedback: None,
        arbitration: None,
    }
}

fn decode(response: Response) -> RoundResult {
    assert!(
        response.error.is_none(),
        "round failed: {:?}",
        response.refusal_text()
    );
    match response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value).expect("typed result"),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("typed result"),
        other => panic!("unexpected payload {other:?}"),
    }
}

fn row_ids(result: &RoundResult) -> Vec<String> {
    let mut ids: Vec<String> = result.rows.iter().map(|row| row.id.clone()).collect();
    ids.sort();
    ids
}

fn refusal(response: &Response) -> String {
    assert!(response.error.is_some(), "expected a refusal");
    response.refusal_text().unwrap_or_default()
}

fn feedback_part(document: &str) -> RoundFeedback {
    RoundFeedback {
        feedback: RetrievalFeedback {
            original_query: "replica recovery".into(),
            round: 1,
            corpus_revision: "corpus-1".into(),
            model_revision: "model-1".into(),
            perspectives: Vec::new(),
            documents: vec![DocumentRelevance {
                document_id: document.into(),
                relevance: 0.9,
            }],
            terms: vec![TermFeedback {
                term: "snapshot".into(),
                tier: FeedbackTier::Reinforce,
                weight: 0.6,
                evidence_document_ids: vec![document.into()],
            }],
            critique: None,
            limits: FeedbackLimits {
                max_rounds: 4,
                max_queries: 2,
                max_terms: 8,
                max_tokens: 1000,
                max_inference_calls: 2,
            },
        },
        prior_terms: Default::default(),
        strategy: None,
    }
}

fn stop_part() -> StopRequest {
    StopRequest {
        calibration: CalibrationTable {
            rows: Default::default(),
        },
        history: Default::default(),
        prior_row_ids: Default::default(),
        prior_quality: None,
        usage: BudgetUsage {
            tokens_used: 10,
            inference_calls_used: 0,
            cost_used: 0,
        },
        limits: CallerLimits::default(),
    }
}

fn feedback_round(document: &str) -> RoundRequest {
    let mut round = request();
    round.feedback = Some(feedback_part(document));
    round.stop = Some(stop_part());
    round
}

fn citation(document_id: &str) -> Citation {
    Citation {
        document_id: document_id.into(),
        start_byte: 0,
        end_byte: 5,
        expected_digest: hex::encode(Sha256::digest(b"entry")),
    }
}

fn candidate(grounding: AnswerGrounding, confidence: f64, cited: Option<&str>) -> Candidate {
    Candidate {
        answer: "entry".into(),
        confidence,
        grounding,
        claims: cited
            .map(|document| {
                vec![Claim {
                    text: "claim".into(),
                    citation: Some(citation(document)),
                }]
            })
            .unwrap_or_default(),
        support_state: SupportState::Unknown,
    }
}

fn arbitration_round(retrieved_cites: &str) -> RoundRequest {
    let mut round = request();
    round.arbitration = Some(ArbitrationPart {
        prior: candidate(AnswerGrounding::Parametric, 0.9, None),
        retrieved: candidate(AnswerGrounding::Retrieved, 0.9, Some(retrieved_cites)),
        query_digest: sha256_hex_of(&["what is entry"]),
    });
    round
}

// spec: MEDULLA-FEDERATED-QUERY-R081.8
#[tokio::test(flavor = "multi_thread")]
async fn feedback_reads_only_the_callers_tenant_in_one_store() {
    let served = Served::new().await;
    let version = served.version().await;

    // Each tenant's round returns only its own documents, and validated feedback over
    // its own document yields a continuation.
    let a = decode(served.round_as(TENANT, feedback_round("doc-a-0")).await);
    assert_eq!(row_ids(&a), ["doc-a-0", "doc-a-1", "doc-a-2"]);
    assert_eq!(
        a.feedback.expect("tenant A continuation").next_query,
        "replica recovery snapshot"
    );
    let b = decode(
        served
            .round_as(OTHER_TENANT, feedback_round("doc-b-0"))
            .await,
    );
    assert_eq!(row_ids(&b), ["doc-b-0", "doc-b-1", "doc-b-2"]);
    assert_eq!(
        b.feedback.expect("tenant B continuation").next_query,
        "replica recovery snapshot"
    );

    // Feedback naming the other tenant's document never reads it: the document is not
    // among the rows this tenant's round returned, so the feedback is refused and no
    // continuation is described.
    let a_over_b = served.round_as(TENANT, feedback_round("doc-b-0")).await;
    assert!(refusal(&a_over_b).starts_with(RETRIEVAL_FEEDBACK_INVALID));
    assert!(a_over_b.result.is_none(), "nothing of B reaches A");
    let b_over_a = served
        .round_as(OTHER_TENANT, feedback_round("doc-a-0"))
        .await;
    assert!(refusal(&b_over_a).starts_with(RETRIEVAL_FEEDBACK_INVALID));
    assert!(b_over_a.result.is_none(), "nothing of A reaches B");

    // The door: a carrier signed for a foreign tenant is refused before any handler,
    // and the owner's answer is unchanged.
    let foreign = served
        .round_at_door(OTHER_TENANT, feedback_round("doc-b-0"))
        .await;
    assert!(refusal(&foreign).contains("AUTH_TENANT_MISMATCH"));
    let again = decode(served.round_as(TENANT, feedback_round("doc-a-0")).await);
    assert_eq!(row_ids(&again), ["doc-a-0", "doc-a-1", "doc-a-2"]);

    // Feedback is a read: no round wrote to the shared store.
    assert_eq!(served.version().await, version, "a round writes nothing");
}

// spec: MEDULLA-FEDERATED-QUERY-R083.3
#[tokio::test(flavor = "multi_thread")]
async fn arbitration_verifies_only_the_callers_tenant_in_one_store() {
    let served = Served::new().await;
    let version = served.version().await;

    // Each tenant's retrieved candidate, citing its own document, is supported and
    // wins over the unsupported prior.
    let a = decode(served.round_as(TENANT, arbitration_round("doc-a-0")).await);
    let verdict = a.arbitration.expect("tenant A verdict");
    assert_eq!(verdict.choice, ArbitrationChoice::Retrieved);
    assert_eq!(verdict.retrieved.support_state, SupportState::Supported);
    assert_eq!(verdict.supporting_span_ids, vec!["doc-a-0:0-5".to_string()]);
    let b = decode(
        served
            .round_as(OTHER_TENANT, arbitration_round("doc-b-0"))
            .await,
    );
    let verdict = b.arbitration.expect("tenant B verdict");
    assert_eq!(verdict.choice, ArbitrationChoice::Retrieved);
    assert_eq!(verdict.supporting_span_ids, vec!["doc-b-0:0-5".to_string()]);

    // A citation of the other tenant's document is outside the caller's snapshot, so it is
    // unknown for the caller: the verifier reads that snapshot only, and both
    // candidates abstain. The verdict names no span of the other tenant.
    for (tenant, foreign) in [(TENANT, "doc-b-0"), (OTHER_TENANT, "doc-a-0")] {
        let crossed = decode(served.round_as(tenant, arbitration_round(foreign)).await);
        let verdict = crossed.arbitration.expect("verdict");
        assert_eq!(verdict.choice, ArbitrationChoice::Abstain);
        assert_eq!(verdict.retrieved.support_state, SupportState::Unknown);
        assert!(verdict.supporting_span_ids.is_empty());
    }

    // The door: a foreign-tenant carrier is refused and the owner's verdict is unchanged.
    let foreign = served
        .round_at_door(OTHER_TENANT, arbitration_round("doc-b-0"))
        .await;
    assert!(refusal(&foreign).contains("AUTH_TENANT_MISMATCH"));
    assert!(foreign.result.is_none(), "no verdict leaks past the door");
    let again = decode(served.round_as(TENANT, arbitration_round("doc-a-0")).await);
    assert_eq!(
        again.arbitration.expect("verdict").choice,
        ArbitrationChoice::Retrieved
    );

    assert_eq!(served.version().await, version, "a round writes nothing");
}
