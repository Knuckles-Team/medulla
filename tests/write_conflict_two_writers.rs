//! Served write-conflict proof over the authenticated dispatch and redb path.
//!
//! A stale `ApplyChangeEnvelope` version claim reaches the resolver under every
//! strategy, refuses with typed codes that change nothing, carries its
//! evidence, and two concurrent writers produce one commit and one typed
//! conflict with no silent overwrite.

#![cfg(all(feature = "server", feature = "security", feature = "redb"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::{SystemTime, UNIX_EPOCH};

use medulla_core::registry::GraphRegistry;
use medulla_mutation_types::mutation_scope::{DurabilityDomain, MutationSurface};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::change_envelope::{
    ChangeEnvelopeDraft, ChangeMutationDraft, ContentVersion, ContentVersionPosition,
    MaterialOperation, PolicyRecord, PrivacyAttestation, CHANGE_ENVELOPE_VERSION,
};
use medulla_protocol_types::mutation_batch::MutationOperation;
use medulla_protocol_types::protocol::{Method, Request, Response, ResultPayload};
use medulla_protocol_types::result_contract::transactions::ChangeEnvelopeApplied;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_auth::auth::{compute_verified_envelope_token, VerifiedEnvelopeParams};
use medulla_types::acl::RequestContextClaims;
use medulla_types::{ConflictWinner, MergeStrategy};

const SECRET: &str = "write-conflict-two-writers-secret";
const GRAPH: &str = "writeconflicttwowriters";
const TENANT: &str = "integration-test-tenant";
const AUDIENCE: &str = "medulla-integration-tests";
const POLICY_VERSION: &str = "integration-test-policy-v1";

type Props = BTreeMap<String, serde_json::Value>;

struct Fixture {
    state: test_support::SharedState,
    dir: String,
    backend: Arc<RedbBackend>,
}

impl Fixture {
    async fn new(label: &str, strategy: MergeStrategy) -> Self {
        let dir = std::env::temp_dir()
            .join(format!(
                "{label}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock after epoch")
                    .as_nanos()
            ))
            .to_string_lossy()
            .into_owned();
        let backend = Arc::new(RedbBackend::open(dir.clone(), 8192).expect("open redb backend"));
        let persistence: Arc<dyn PersistenceBackend> = backend.clone();
        let mut registry = GraphRegistry::new();
        registry
            .create_graph(
                GRAPH,
                medulla_protocol_types::protocol::GraphType::Commons,
                None,
            )
            .expect("create graph");
        let state = test_support::state_with_registry(
            SECRET,
            common::current_isolation(),
            registry,
            Some(dir.clone()),
            Some(persistence),
        );
        state.write().await.write_conflict_strategy = strategy;
        Self {
            state,
            dir,
            backend,
        }
    }

    async fn version(&self) -> u64 {
        let state = self.state.read().await;
        state.registry.get(GRAPH).expect("graph").core.version()
    }

    async fn node(&self, node_id: &str) -> Option<Props> {
        let state = self.state.read().await;
        let core = state.registry.get(GRAPH).expect("graph").core.clone();
        core.get_node_properties(node_id)
            .map(|bytes| rmp_serde::from_slice(&bytes).expect("decode node properties"))
    }

    async fn envelope_is_stored(&self, envelope_id: &str) -> bool {
        self.backend
            .read_change_envelope(&medulla_kernel::persist::sanitize(GRAPH), envelope_id)
            .await
            .expect("read envelope ledger")
            .is_some()
    }

    async fn send(&self, draft: ChangeEnvelopeDraft) -> Response {
        send(&self.state, draft).await
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.backend.shutdown();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn props(pairs: &[(&str, i64)]) -> Props {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), serde_json::Value::from(*value)))
        .collect()
}

/// One draft whose operations are `AddNode` of each `(node, properties)`,
/// pinned to the graph version `expected`.
fn draft(envelope_id: &str, nodes: &[(&str, &Props)], expected: u64) -> ChangeEnvelopeDraft {
    // Each envelope versions its own content object, so two writers never
    // chain through one object's content-version history.
    let object = format!("object:{envelope_id}");
    let object = object.as_str();
    ChangeEnvelopeDraft {
        schema_version: CHANGE_ENVELOPE_VERSION,
        envelope_id: envelope_id.into(),
        mutation: ChangeMutationDraft {
            batch_id: format!("batch:{envelope_id}"),
            placement_epoch: 0,
            expected_graph_version: Some(expected),
            fencing_token: None,
            operations: nodes
                .iter()
                .enumerate()
                .map(|(ordinal, (node_id, properties))| MutationOperation {
                    ordinal: ordinal as u32,
                    surface: MutationSurface::Graph,
                    domain: DurabilityDomain::GraphRows,
                    method: Method::AddNode {
                        node_id: (*node_id).into(),
                        properties_msgpack: rmp_serde::to_vec_named(properties)
                            .expect("encode properties"),
                    },
                })
                .collect(),
            outbox: Vec::new(),
        },
        privacy: PrivacyAttestation {
            policy_version: "privacy-v1".into(),
            sanitizer_version: "sanitizer-v1".into(),
            sanitized_payload_digest: "c".repeat(64),
        },
        material_class: Default::default(),
        content_version: ContentVersion {
            object_id: object.into(),
            digest_algorithm: "sha256".into(),
            digest: "a".repeat(64),
            previous_digest: None,
            source_version: ContentVersionPosition::Sequence(1),
        },
        cursor: None,
        lineage: Vec::new(),
        policies: vec![PolicyRecord {
            policy_id: format!("policy:{envelope_id}"),
            operation: MaterialOperation::Upsert,
            object_id: object.into(),
            tenant: TENANT.into(),
            classification: "internal".into(),
            policy_version: "policy-v1".into(),
            subject_set_digest: "b".repeat(64),
            retention_policy: String::new(),
            legal_hold: false,
        }],
        evidence: Vec::new(),
        features: Vec::new(),
        blobs: Vec::new(),
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs()
}

fn signed_request(id: u64, method: Method, idempotency_key: &str) -> Request {
    common::configure_authority();
    static NONCE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let principal = common::TEST_AGENT;
    let context = RequestContextClaims {
        principal: principal.to_string(),
        tenant: TENANT.to_string(),
        audience: AUDIENCE.to_string(),
        agent_id: principal.to_string(),
        roles: Vec::new(),
        scopes: vec!["*".to_string()],
        policy_version: POLICY_VERSION.to_string(),
        delegation: Vec::new(),
        node: None,
        priority: None,
    };
    let mut request = Request {
        id,
        graph: GRAPH.to_string(),
        auth_token: String::new(),
        agent_id: Some(principal.to_string()),
        method,
    };
    let nonce = format!(
        "client-wire-{}-{}",
        std::process::id(),
        NONCE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    request.auth_token = compute_verified_envelope_token(
        SECRET,
        &request,
        &VerifiedEnvelopeParams {
            context: &context,
            timestamp: now_secs(),
            nonce: &nonce,
            idempotency_key,
        },
    );
    request
}

async fn send(state: &test_support::SharedState, draft: ChangeEnvelopeDraft) -> Response {
    static REQUEST_IDS: AtomicU64 = AtomicU64::new(1);
    let envelope_id = draft.envelope_id.clone();
    let id = REQUEST_IDS.fetch_add(1, Ordering::Relaxed);
    let request = signed_request(
        id,
        Method::ApplyChangeEnvelopeDraft {
            draft: Box::new(draft),
        },
        &format!("key:{envelope_id}"),
    );
    test_support::dispatch(state, request).await
}

fn applied(response: &Response) -> ChangeEnvelopeApplied {
    assert_eq!(response.error, None, "{response:?}");
    let Some(ResultPayload::Json(value)) = response.result.clone() else {
        panic!("a commit answers a JSON result: {response:?}");
    };
    serde_json::from_value(value).expect("decode ChangeEnvelopeApplied")
}

fn refusal(response: &Response) -> (&str, &str) {
    (
        response.error.as_deref().expect("a refusal carries a code"),
        response.error_detail.as_deref().unwrap_or_default(),
    )
}

const STRATEGIES: [MergeStrategy; 3] = [
    MergeStrategy::Refuse,
    MergeStrategy::PreferNewer,
    MergeStrategy::FieldUnion,
];

// spec: MEDULLA-DURABLE-KERNEL-R084.3
#[tokio::test]
async fn stale_envelope_reaches_the_resolver() {
    for strategy in STRATEGIES {
        let fixture = Fixture::new("write-conflict-resolver", strategy).await;
        let v0 = fixture.version().await;
        let seed = fixture
            .send(draft("seed", &[("doc:1", &props(&[("a", 1)]))], v0))
            .await;
        applied(&seed);
        assert!(fixture.version().await > v0);

        // The stale writer still claims `v0`.
        let stale = fixture
            .send(draft("stale", &[("doc:1", &props(&[("b", 2)]))], v0))
            .await;
        match strategy {
            MergeStrategy::Refuse => {
                assert_eq!(refusal(&stale).0, "WRITE_CONFLICT", "{stale:?}");
                assert_eq!(fixture.node("doc:1").await, Some(props(&[("a", 1)])));
            }
            MergeStrategy::PreferNewer => {
                applied(&stale);
                assert_eq!(fixture.node("doc:1").await, Some(props(&[("b", 2)])));
            }
            MergeStrategy::FieldUnion => {
                applied(&stale);
                assert_eq!(
                    fixture.node("doc:1").await,
                    Some(props(&[("a", 1), ("b", 2)]))
                );
            }
        }
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R084.4
#[tokio::test]
async fn conflict_codes_leave_state_unchanged() {
    // Refuse: WRITE_CONFLICT with the three values.
    let fixture = Fixture::new("write-conflict-refuse", MergeStrategy::Refuse).await;
    let v0 = fixture.version().await;
    applied(
        &fixture
            .send(draft("seed", &[("doc:1", &props(&[("a", 1)]))], v0))
            .await,
    );
    let v1 = fixture.version().await;
    let stale = fixture
        .send(draft("stale", &[("doc:1", &props(&[("b", 2)]))], v0))
        .await;
    let (code, detail) = refusal(&stale);
    assert_eq!(code, "WRITE_CONFLICT", "{stale:?}");
    assert!(
        detail.contains(&format!("expected {v0}, actual {v1}, scope {GRAPH}")),
        "{detail}"
    );
    assert_eq!(fixture.node("doc:1").await, Some(props(&[("a", 1)])));
    assert_eq!(fixture.version().await, v1);
    assert!(!fixture.envelope_is_stored("stale").await);
    drop(fixture);

    // FieldUnion: a field collision and a not-yet-existing node are unmergeable.
    let fixture = Fixture::new("write-conflict-union", MergeStrategy::FieldUnion).await;
    let v0 = fixture.version().await;
    applied(
        &fixture
            .send(draft("seed", &[("doc:1", &props(&[("a", 1)]))], v0))
            .await,
    );
    let v1 = fixture.version().await;
    for (id, nodes) in [
        ("collide", vec![("doc:1", props(&[("a", 9)]))]),
        ("absent", vec![("doc:2", props(&[("a", 1)]))]),
    ] {
        let borrowed: Vec<(&str, &Props)> = nodes.iter().map(|(n, p)| (*n, p)).collect();
        let response = fixture.send(draft(id, &borrowed, v0)).await;
        let (code, detail) = refusal(&response);
        assert_eq!(code, "WRITE_CONFLICT_UNMERGEABLE", "{id}: {response:?}");
        assert!(
            detail.contains(&format!("expected {v0}, actual {v1}, scope {GRAPH}")),
            "{detail}"
        );
        assert_eq!(fixture.node("doc:1").await, Some(props(&[("a", 1)])));
        assert!(fixture.node("doc:2").await.is_none());
        assert_eq!(fixture.version().await, v1);
        assert!(!fixture.envelope_is_stored(id).await);
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R084.5
#[tokio::test]
async fn conflict_evidence_names_scope_strategy_winner() {
    let moved = |strategy: MergeStrategy, winner: ConflictWinner| {
        medulla_kernel::metrics::write_conflict_count(strategy.label(), winner.label())
    };

    // One refusal.
    let refuse_before = moved(MergeStrategy::Refuse, ConflictWinner::Existing);
    let fixture = Fixture::new("write-conflict-evidence-refuse", MergeStrategy::Refuse).await;
    let v0 = fixture.version().await;
    applied(
        &fixture
            .send(draft("seed", &[("doc:1", &props(&[("a", 1)]))], v0))
            .await,
    );
    let stale = fixture
        .send(draft("stale", &[("doc:1", &props(&[("b", 2)]))], v0))
        .await;
    let (code, detail) = refusal(&stale);
    assert_eq!(code, "WRITE_CONFLICT", "{stale:?}");
    let actual = fixture.version().await;
    assert_eq!(
        detail,
        format!("expected {v0}, actual {actual}, scope {GRAPH}"),
        "the refusal detail is exactly the spec text"
    );
    drop(fixture);

    // One merge.
    let merge_before = moved(MergeStrategy::FieldUnion, ConflictWinner::Merged);
    let fixture = Fixture::new("write-conflict-evidence-merge", MergeStrategy::FieldUnion).await;
    let v0 = fixture.version().await;
    applied(
        &fixture
            .send(draft("seed", &[("doc:1", &props(&[("a", 1)]))], v0))
            .await,
    );
    let merged = applied(
        &fixture
            .send(draft("stale", &[("doc:1", &props(&[("b", 2)]))], v0))
            .await,
    );
    let evidence = merged.write_conflict.expect("a merge carries its evidence");
    assert_eq!(evidence.conflict_scope, GRAPH);
    assert_eq!(evidence.strategy, MergeStrategy::FieldUnion);
    assert_eq!(evidence.winner, ConflictWinner::Merged);

    #[cfg(feature = "metrics")]
    {
        assert!(moved(MergeStrategy::Refuse, ConflictWinner::Existing) > refuse_before);
        assert!(moved(MergeStrategy::FieldUnion, ConflictWinner::Merged) > merge_before);
    }
    #[cfg(not(feature = "metrics"))]
    let _ = (refuse_before, merge_before);
}

// spec: MEDULLA-DURABLE-KERNEL-R084.6
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_writers_one_commit_one_typed_conflict() {
    let fixture = Fixture::new("write-conflict-two-writers", MergeStrategy::Refuse).await;
    for round in 0..50 {
        let v = fixture.version().await;
        let (node_a, node_b) = (format!("round{round}:a"), format!("round{round}:b"));
        let (id_a, id_b) = (format!("round{round}-a"), format!("round{round}-b"));
        let (draft_a, draft_b) = (
            draft(&id_a, &[(&node_a, &props(&[("w", 1)]))], v),
            draft(&id_b, &[(&node_b, &props(&[("w", 2)]))], v),
        );
        let (state_a, state_b) = (fixture.state.clone(), fixture.state.clone());
        let writer_a = tokio::spawn(async move { send(&state_a, draft_a).await });
        let writer_b = tokio::spawn(async move { send(&state_b, draft_b).await });
        let responses = [
            writer_a.await.expect("writer a"),
            writer_b.await.expect("writer b"),
        ];
        let committed: Vec<_> = responses.iter().filter(|r| r.error.is_none()).collect();
        let conflicts: Vec<_> = responses.iter().filter(|r| r.error.is_some()).collect();
        assert_eq!(committed.len(), 1, "round {round}: {responses:?}");
        assert_eq!(conflicts.len(), 1, "round {round}: {responses:?}");
        let (code, detail) = refusal(conflicts[0]);
        assert_eq!(code, "WRITE_CONFLICT", "round {round}");
        let actual = fixture.version().await;
        assert!(actual > v, "round {round}: the winner advanced the version");
        assert!(
            detail.contains(&format!("expected {v}, actual {actual}, scope {GRAPH}")),
            "round {round}: {detail}"
        );
        // No silent overwrite: exactly one of the two nodes exists.
        let present = [fixture.node(&node_a).await, fixture.node(&node_b).await];
        assert_eq!(
            present.iter().flatten().count(),
            1,
            "round {round}: {present:?}"
        );
    }

    // FieldUnion: two writers on disjoint properties of one node both commit.
    drop(fixture);
    let fixture = Fixture::new(
        "write-conflict-two-writers-union",
        MergeStrategy::FieldUnion,
    )
    .await;
    for round in 0..50 {
        let v = fixture.version().await;
        let node = format!("shared{round}");
        let (draft_a, draft_b) = (
            draft(&format!("u{round}-a"), &[(&node, &props(&[("x", 1)]))], v),
            draft(&format!("u{round}-b"), &[(&node, &props(&[("y", 2)]))], v),
        );
        let (state_a, state_b) = (fixture.state.clone(), fixture.state.clone());
        let writer_a = tokio::spawn(async move { send(&state_a, draft_a).await });
        let writer_b = tokio::spawn(async move { send(&state_b, draft_b).await });
        for response in [
            writer_a.await.expect("writer a"),
            writer_b.await.expect("writer b"),
        ] {
            applied(&response);
        }
        assert_eq!(
            fixture.node(&node).await,
            Some(props(&[("x", 1), ("y", 2)])),
            "round {round}"
        );
    }
}
