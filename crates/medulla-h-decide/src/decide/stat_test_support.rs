//! Shared fixtures for the served statistical-decision tests. The in-crate stat tests and the
//! facade tests that go through the full dispatcher both use them.

#[cfg(all(
    any(test, feature = "test-support"),
    feature = "blob",
    feature = "redb"
))]
use medulla_server_blob::blob::BlobStateExt;
use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_agent_types::{
    agent_component::AgentComponentKind, agent_component::AgentComponentPublishRequest,
    agent_component::ComponentDependency,
};
use medulla_decision_types::decision::statistical::body::encode_body;
use medulla_decision_types::decision::statistical::features::{
    FeatureKind, FeatureSchemaBody, FeatureSpec, MissingValue, FEATURE_SCHEMA_VERSION,
};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, QuestionSafety, StatisticalQuestion, TypedParam,
    TypedValue,
};
use medulla_decision_types::decision::{DecisionPolicyRef, LibraryCandidateScope};
use medulla_persistence_records::agent_component::test_component_draft;
use medulla_persistence_records::agent_fixtures::mutation_context;
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::state::ServerState;
use medulla_types::contract::BoundedVec;

pub const TENANT: &str = "tenant-shared";

pub struct Harness {
    pub _dir: tempfile::TempDir,
    pub state: Arc<RwLock<ServerState>>,
    pub store: Arc<AgentLibraryStore>,
    pub nonce: std::cell::Cell<u8>,
}

impl Harness {
    pub async fn new() -> Self {
        Self::with_isolation(medulla_isolation::isolation::IsolationLayer::new()).await
    }

    pub async fn with_isolation(isolation: medulla_isolation::isolation::IsolationLayer) -> Self {
        Self::open(tempfile::tempdir().unwrap(), isolation).await
    }

    /// Close this harness and open a fresh one over the same persisted store,
    /// as a restarted server would see it.
    pub async fn reopened(self) -> Self {
        let Self {
            _dir, state, store, ..
        } = self;
        drop(store);
        drop(state);
        Self::open(_dir, medulla_isolation::isolation::IsolationLayer::new()).await
    }

    async fn open(
        dir: tempfile::TempDir,
        isolation: medulla_isolation::isolation::IsolationLayer,
    ) -> Self {
        let mut server = ServerState::new_for_test("decide-stat-test-secret", isolation);
        server.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        #[cfg(feature = "blob")]
        {
            let cas = medulla_server_blob::blob::store::RedbChunkStore::open(
                dir.path().to_str().unwrap(),
            )
            .unwrap();
            server.set_blob_cursors(Some(Arc::new(medulla_server_blob::blob::BlobCursors::new(
                Arc::new(cas),
            ))));
        }
        let state = Arc::new(RwLock::new(server));
        let store = state.write().await.ensure_agent_library().unwrap();
        Self {
            _dir: dir,
            state,
            store,
            nonce: std::cell::Cell::new(1),
        }
    }

    pub fn commit(
        &self,
        draft: medulla_agent_types::agent_component::AgentComponentDraft,
        receipt: Option<String>,
    ) -> Result<ComponentDependency, String> {
        self.commit_in(TENANT, draft, receipt)
    }

    /// Publish `draft` into `tenant`'s library.
    pub fn commit_in(
        &self,
        tenant: &str,
        draft: medulla_agent_types::agent_component::AgentComponentDraft,
        receipt: Option<String>,
    ) -> Result<ComponentDependency, String> {
        let nonce = self.nonce.get();
        self.nonce.set(nonce + 1);
        let (id, kind) = (draft.component_id.clone(), draft.kind);
        let request = AgentComponentPublishRequest {
            context: mutation_context(
                &self.store,
                tenant,
                &format!("publish-{id}-{nonce}"),
                nonce,
                0,
                "agent-component:publish",
            ),
            component: draft,
            evaluation_receipt_digest: receipt,
        };
        let committed = self.store.publish_component(request)?;
        Ok(ComponentDependency {
            component_id: id,
            kind,
            definition_digest: committed.result.component.definition_digest,
        })
    }

    /// Retire component `id` of `tenant` at its current entry revision
    /// `expected_revision`, leaving a durable tombstone.
    pub fn retire_in(&self, tenant: &str, id: &str, expected_revision: u64) -> Result<(), String> {
        let nonce = self.nonce.get();
        self.nonce.set(nonce + 1);
        self.store
            .retire_component(
                medulla_agent_types::agent_component::AgentComponentRetireRequest {
                    context: mutation_context(
                        &self.store,
                        tenant,
                        &format!("retire-{id}-{nonce}"),
                        nonce,
                        expected_revision,
                        "agent-component:retire",
                    ),
                    component_id: id.to_string(),
                },
            )
            .map(|_| ())
    }

    pub fn publish_test_component(
        &self,
        id: &str,
        kind: AgentComponentKind,
        summary: &str,
        body: Option<&impl serde::Serialize>,
        receipt: Option<String>,
    ) -> Result<ComponentDependency, String> {
        let mut draft = test_component_draft(TENANT, id);
        draft.kind = kind;
        draft.summary = summary.to_string();
        draft.classification = vec!["eg:capability/retrieval/web-search".to_string()];
        if let Some(body) = body {
            let encoded = encode_body(body).unwrap();
            draft.content_digest = encoded.content_digest;
            draft.attributes = encoded.attributes;
        }
        self.commit(draft, receipt)
    }

    /// Publish a policy the way the assembly lane serves it: canonical JSON in
    /// one attribute, `content_digest` = the policy digest.
    pub fn publish_policy(
        &self,
        id: &str,
        policy: &medulla_decision_types::decision::DecisionPolicy,
    ) -> ComponentDependency {
        let mut draft = test_component_draft(TENANT, id);
        draft.kind = AgentComponentKind::DecisionPolicy;
        draft.content_digest = medulla_decision_types::decision::digest::policy_digest(policy);
        draft.attributes.insert(
            medulla_decision_types::decision::policy::DECISION_POLICY_ATTRIBUTE.to_string(),
            serde_json::to_string(policy).unwrap(),
        );
        self.commit(draft, None).unwrap()
    }
}

pub fn decode<T: serde::de::DeserializeOwned>(
    response: medulla_protocol_types::protocol::Response,
) -> Result<T, String> {
    if let Some(error) = response.error {
        return Err(error);
    }
    match response.result {
        Some(ResultPayload::Raw(bytes)) => Ok(rmp_serde::from_slice(&bytes).unwrap()),
        other => panic!("unexpected payload {other:?}"),
    }
}

pub fn schema() -> FeatureSchemaBody {
    FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![
            medulla_decision_types::decision_fixtures::decision::summary_text_feature(),
            FeatureSpec {
                name: "coverage".to_string(),
                kind: FeatureKind::CoverageFraction {
                    param: "needs".to_string(),
                },
                missing: MissingValue::Abstain,
            },
        ])
        .unwrap(),
    }
}

pub fn publish_route_schema(h: &Harness) -> (ComponentDependency, String) {
    let body = schema();
    let pin = h
        .publish_test_component(
            "schema-route",
            AgentComponentKind::FeatureSchema,
            "route features",
            Some(&body),
            None,
        )
        .unwrap();
    let digest = encode_body(&body).unwrap().content_digest;
    (pin, digest)
}

pub fn request(
    schema: &ComponentDependency,
    head: Option<ComponentDependency>,
    policy: DecisionPolicyRef,
    safety: QuestionSafety,
) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "route.tools".to_string(),
            kind: QuestionKind::Route,
            safety,
        },
        candidates: CandidateSource::AgentLibrary {
            scope: LibraryCandidateScope {
                kinds: BoundedVec::new(vec![AgentComponentKind::Tool]).unwrap(),
                classification_under: None,
            },
        },
        feature_schema: schema.clone(),
        head,
        policy,
        params: BoundedVec::new(vec![
            TypedParam {
                name: "needs".to_string(),
                value: TypedValue::IriList(
                    BoundedVec::new(vec!["eg:capability/retrieval".to_string()]).unwrap(),
                ),
            },
            TypedParam {
                name: "query".to_string(),
                value: TypedValue::Text("web search engine".to_string()),
            },
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

/// A declared option with no features.
pub fn declared_option(
    id: &str,
) -> medulla_decision_types::decision::statistical::declared::DeclaredOption {
    medulla_decision_types::decision::statistical::declared::DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::default(),
        texts: BoundedVec::default(),
    }
}

/// The route feature schema, published into `tenant`'s library.
pub fn publish_route_schema_in(h: &Harness, tenant: &str) -> ComponentDependency {
    let body = schema();
    let encoded = encode_body(&body).unwrap();
    let mut draft = test_component_draft(tenant, &format!("schema-route-{tenant}"));
    draft.kind = AgentComponentKind::FeatureSchema;
    draft.summary = "route features".to_string();
    draft.classification = vec!["eg:capability/retrieval/web-search".to_string()];
    draft.content_digest = encoded.content_digest;
    draft.attributes = encoded.attributes;
    h.commit_in(tenant, draft, None).unwrap()
}

/// A request for `kind` over one declared option and the given parameters.
pub fn declared_request(
    schema: &ComponentDependency,
    tenant: &str,
    kind: QuestionKind,
    option: &str,
    policy: DecisionPolicyRef,
    mut params: Vec<TypedParam>,
) -> DecideRequest {
    params.sort_by(|a, b| a.name.cmp(&b.name));
    DecideRequest {
        tenant_id: tenant.to_string(),
        question: StatisticalQuestion {
            question_id: format!("{kind:?}.test"),
            kind,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![declared_option(option)]).unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy,
        params: BoundedVec::new(params).unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

/// The capacity cell the generation-cycle fixture leases from.
pub const CYCLE_CELL: &str = "cell-embed";
/// The graph the cycle fixture's logical graph resolves to before its first generation.
pub const CYCLE_ACTIVE: &str = "kg-bench-cycle";
/// The shadow graph the cycle fixture re-embeds.
pub const CYCLE_SHADOW: &str = "kg-bench-cycle-2";
/// Width of the cycle fixture's embedding model.
pub const CYCLE_DIMS: usize = 384;

/// The digest of the fixture model's space at `revision`.
fn cycle_space(revision: &str) -> String {
    medulla_index_types::embedding::EmbeddingSpaceRef::pinned(
        "m", revision, "sha256:w", "sha256:p", CYCLE_DIMS, false,
    )
    .unwrap()
    .digest
}

/// A first generation request (R174.6 bench): the logical graph has no active
/// generation, so the cycle bootstraps; `nodes` Doc nodes sit unembedded in the
/// shadow graph and a durable capacity ledger with one embedding cell is
/// installed in the served state under `ledger`.
pub async fn generation_cycle_fixture(
    h: &Harness,
    ledger: &std::path::Path,
    nodes: usize,
) -> medulla_decision_types::decision::statistical::retrieval_generation::GenerationCycleRequest {
    use medulla_decision_types::decision::statistical::retrieval_generation::{
        GenerationCycleRequest, GenerationEvalRequest, ModelRef,
    };
    use medulla_mutation_types::capacity_lease::{CapacityCell, CapacityResourceClass};
    use medulla_protocol_types::protocol::Method;
    use medulla_server_api::api::persistence::PersistenceBackend;
    use medulla_work_types::native_control::{
        CapacityCellUpdateRequest, NativeControlSchemaVersion,
    };

    let backend = Arc::new(
        medulla_persistence::redb_backend::RedbBackend::open_with_shards(
            ledger.to_str().unwrap().to_string(),
            16,
            1,
        )
        .unwrap(),
    );
    backend
        .commit_capacity_lease(
            &medulla_kernel::persist::sanitize(CYCLE_ACTIVE),
            Method::UpdateCapacityCell {
                request: CapacityCellUpdateRequest {
                    schema_version: NativeControlSchemaVersion::V1,
                    cell: CapacityCell {
                        cell_id: CYCLE_CELL.into(),
                        parent_id: None,
                        resource_class: CapacityResourceClass::LlmEmbedding,
                        capacity: 8,
                        reserved_floor: 0,
                        epoch: 1,
                        policy_digest: "d".repeat(64),
                        updated_at_ms: 0,
                        throttle: None,
                    },
                    expected_epoch: None,
                    now_ms: 1,
                },
            },
        )
        .await
        .unwrap();
    let mut guard = h.state.write().await;
    guard.persistence = Some(backend);
    for name in [CYCLE_ACTIVE, CYCLE_SHADOW] {
        guard
            .registry
            .create_graph(
                name,
                medulla_protocol_types::protocol::GraphType::Team,
                None,
            )
            .unwrap();
    }
    let core = guard.registry.get(CYCLE_SHADOW).unwrap().core.clone();
    drop(guard);
    core.semantic_store
        .write()
        .declare_space(
            medulla_index_types::embedding::EmbeddingSpaceRef::pinned(
                "m", "2", "sha256:w", "sha256:p", CYCLE_DIMS, false,
            )
            .unwrap(),
        )
        .unwrap();
    for index in 0..nodes {
        let id = format!("doc-{index}");
        let props = serde_json::json!({"type": "Doc", "text": id});
        core.add_node(id, rmp_serde::to_vec_named(&props).unwrap());
    }
    GenerationCycleRequest {
        model_ref: ModelRef {
            id: "m".to_string(),
            revision: "2".to_string(),
            dimension: CYCLE_DIMS as u32,
        },
        evaluation: GenerationEvalRequest {
            logical: CYCLE_ACTIVE.to_string(),
            active_graph: CYCLE_ACTIVE.to_string(),
            active_space: "none".to_string(),
            shadow_graph: CYCLE_SHADOW.to_string(),
            shadow_space: cycle_space("2"),
            items: BoundedVec::default(),
            top_k: 5,
            min_eval_items: 5,
            max_score_psi_q16: 64 << 16,
        },
        capacity_cell: CYCLE_CELL.to_string(),
        max_batches: Some(1),
        index_adapter: None,
    }
}

/// One served submission of a generation cycle through `DecisionLog` (`Learn`):
/// the state and node checkpoint it reports, as `(state, reembedded_nodes)`.
pub async fn submit_generation_cycle(
    h: &Harness,
    request: &medulla_decision_types::decision::statistical::retrieval_generation::GenerationCycleRequest,
) -> Result<(String, u64), String> {
    use medulla_decision_types::decision::statistical::log::DecisionLogOp;
    use medulla_decision_types::decision::statistical::retrieval::{
        LearningRecorded, LearningWrite,
    };
    let verified =
        medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test_in_tenant(
            "decider", TENANT,
        );
    let op = DecisionLogOp::Learn {
        tenant_id: TENANT.to_string(),
        write: LearningWrite::RunGenerationCycle {
            request: Box::new(request.clone()),
        },
    };
    let response = super::log::handle_decision_log(&h.state, 9, &verified, op).await;
    match decode::<LearningRecorded>(response)? {
        LearningRecorded::Cycle(job) => {
            Ok((format!("{:?}", job.cycle.state()), job.reembedded_nodes))
        }
        other => Err(format!("a cycle answers with its job, not {other:?}")),
    }
}
