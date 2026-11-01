//! Shared fixtures for the served statistical-decision tests. The in-crate stat tests and the
//! facade tests that go through the full dispatcher both use them.

use medulla_server_api::blob::BlobStateExt;
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
use medulla_persistence::agent_component::test_component_draft;
use medulla_persistence::agent_fixtures::mutation_context;
use medulla_persistence::agent_library::AgentLibraryStateExt;
use medulla_persistence::agent_library::AgentLibraryStore;
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
        let dir = tempfile::tempdir().unwrap();
        let mut server = ServerState::new_for_test("decide-stat-test-secret", isolation);
        server.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        #[cfg(feature = "blob")]
        {
            let cas =
                medulla_server_api::blob::store::RedbChunkStore::open(dir.path().to_str().unwrap())
                    .unwrap();
            server.set_blob_cursors(Some(Arc::new(medulla_server_api::blob::BlobCursors::new(Arc::new(
                cas,
            )))));
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
        let nonce = self.nonce.get();
        self.nonce.set(nonce + 1);
        let (id, kind) = (draft.component_id.clone(), draft.kind);
        let request = AgentComponentPublishRequest {
            context: mutation_context(
                &self.store,
                TENANT,
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

    pub fn publish(
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
        .publish(
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

