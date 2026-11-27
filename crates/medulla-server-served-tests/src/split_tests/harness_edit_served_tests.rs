//! MEDULLA-DECISION-ENGINE-R173.8: only `HarnessEdit` sets `editStatus`, proven through the
//! real dispatch chain. A served `Create` writes the edit as `proposed`, a second `Create`
//! refuses `HARNESS_EDIT_EXISTS`, and a caller's `AddNode` of `editStatus` is refused and
//! leaves the stored status alone.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use medulla_server_api::state::ServerState;
use medulla_server_auth::auth::{dispatch_test_on_heap, scoped_test_request_on, ScopedTestCaller};
use medulla_types::edit_machine::EditEvent;
use medulla_work_types::harness_edit::HarnessEditOp;
use tokio::sync::RwLock;

const SECRET: &str = "harness-edit-served-secret"; // sanitizer:ignore
/// The fixed tenant the served test auth policy admits at the door.
const TENANT: &str = "tenant-shared";
const ADMIN: &str = "harness-edit-served-admin";
const COMMONS: &str = "__commons__";
const DIMENSION: &str = "dim:tokens";

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

struct Served {
    _dir: tempfile::TempDir,
    state: Arc<RwLock<ServerState>>,
}

impl Served {
    /// A durable served engine whose commons graph holds one dimension node.
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
        let seeded = served
            .call(Method::AddNode {
                node_id: DIMENSION.to_string(),
                properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                    "type": "Dimension",
                }))
                .unwrap(),
            })
            .await;
        assert!(seeded.error.is_none(), "seed failed: {:?}", seeded.error);
        served
    }

    /// `method` as the tenant's writer, through the dispatch chokepoint.
    async fn call(&self, method: Method) -> Response {
        let call_id = next_id();
        let request = scoped_test_request_on(
            SECRET,
            call_id,
            COMMONS,
            method,
            ScopedTestCaller {
                principal: ADMIN,
                tenant: TENANT,
                scopes: &["*"],
                nonce: &format!("harness-edit-served-nonce-{call_id}"),
                idempotency_key: &format!("harness-edit-served-{call_id}"),
            },
        );
        dispatch_test_on_heap(&self.state, request).await
    }

    async fn harness_edit(&self, op: HarnessEditOp) -> Response {
        self.call(Method::HarnessEdit { op: Box::new(op) }).await
    }

    async fn create(&self, edit_id: &str) -> Response {
        self.harness_edit(HarnessEditOp::Create {
            edit_id: edit_id.to_string(),
            dimension_iri: DIMENSION.to_string(),
            round: 4,
            policy_revision: Some(2),
        })
        .await
    }

    /// The stored properties of `node_id`, `None` when the node does not exist.
    async fn node(&self, node_id: &str) -> Option<serde_json::Value> {
        let response = self
            .call(Method::GetNodeProperties {
                node_id: node_id.to_string(),
            })
            .await;
        assert!(
            response.error.is_none(),
            "read refused: {:?}",
            response.error
        );
        match response.result.as_ref() {
            Some(ResultPayload::Raw(bytes)) => {
                rmp_serde::from_slice::<Option<serde_json::Value>>(bytes).expect("a node")
            }
            _ => None,
        }
    }
}

/// A caller's generic write of `editStatus` on `edit_id`.
fn caller_write(edit_id: &str, status: &str) -> Method {
    Method::AddNode {
        node_id: edit_id.to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
            "_harness_edit": true,
            "editStatus": status,
            "tenant_id": TENANT,
        }))
        .unwrap(),
    }
}

/// The answer of `response` as JSON text, whichever payload form carries it.
fn answer_text(label: &str, response: Response) -> String {
    assert!(
        response.error.is_none(),
        "{label} failed: {:?}",
        response.refusal_text()
    );
    match response.result {
        Some(ResultPayload::Raw(bytes)) => {
            let value: serde_json::Value =
                rmp_serde::from_slice(&bytes).expect("a typed MessagePack answer");
            value.to_string()
        }
        Some(other) => serde_json::to_string(&other).expect("a serializable answer"),
        None => String::new(),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R173.8
#[tokio::test(flavor = "multi_thread")]
async fn harness_edit_create_is_served() {
    let served = Served::new().await;
    let created = answer_text("Create", served.create("edit:new").await);
    assert!(created.contains("proposed"), "{created}");

    let properties = served.node("edit:new").await.expect("the edit node exists");
    assert_eq!(properties["editStatus"], "proposed");
    assert_eq!(properties["editRound"], 4);
    assert_eq!(properties["policyRevision"], 2);
    assert_eq!(properties["tenant_id"], TENANT);

    // The status then moves only by the machine: ship answers `shipped`.
    let shipped = answer_text(
        "Ship",
        served
            .harness_edit(HarnessEditOp::Transition {
                edit_id: "edit:new".to_string(),
                event: EditEvent::Ship,
            })
            .await,
    );
    assert!(shipped.contains("shipped"), "{shipped}");
}

// spec: MEDULLA-DECISION-ENGINE-R173.8
#[tokio::test(flavor = "multi_thread")]
async fn second_create_refuses_harness_edit_exists() {
    let served = Served::new().await;
    assert!(served.create("edit:twice").await.error.is_none());

    let second = served.create("edit:twice").await;
    let text = second.refusal_text().unwrap_or_default();
    assert!(text.contains("HARNESS_EDIT_EXISTS"), "{text}");

    // The refused create wrote nothing: the edit keeps its first round and status.
    let properties = served
        .node("edit:twice")
        .await
        .expect("the edit node exists");
    assert_eq!(properties["editStatus"], "proposed");
    assert_eq!(properties["editRound"], 4);
}

// spec: MEDULLA-DECISION-ENGINE-R173.8
#[tokio::test(flavor = "multi_thread")]
async fn served_add_node_of_edit_status_is_refused() {
    let served = Served::new().await;

    // A caller cannot create an edit through the generic route, `proposed` included.
    let created = served.call(caller_write("edit:generic", "proposed")).await;
    let text = created.refusal_text().unwrap_or_default();
    assert!(text.contains("EDIT_STATUS_ENGINE_OWNED"), "{text}");
    assert!(
        served.node("edit:generic").await.is_none(),
        "a refused write creates no node"
    );

    // Nor can it move the status of an edit the engine created.
    assert!(served.create("edit:owned").await.error.is_none());
    let moved = served.call(caller_write("edit:owned", "shipped")).await;
    let text = moved.refusal_text().unwrap_or_default();
    assert!(text.contains("EDIT_STATUS_ENGINE_OWNED"), "{text}");
    let properties = served
        .node("edit:owned")
        .await
        .expect("the edit node exists");
    assert_eq!(properties["editStatus"], "proposed");
}
