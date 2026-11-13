//! MEDULLA-DECISION-ENGINE-R172.12.3: an applied connector-pack projection
//! upserts the pack's surface records into the resident capability index.
//!
//! The proof needs the pack import and projection of `medulla-h-admin` and the
//! capability feed of `medulla-h-decide`, which are sibling handler crates, so
//! it is hosted here where both are linked. Every pack request goes through the
//! real signed dispatch; the projection worker and the feed are swept exactly
//! as their spawned loops sweep them.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use medulla_catalog_types::connector_pack::{
    ConnectorPackBindRequest, ConnectorPackImportRequest, ConnectorPackIndex, ConnectorPackOp,
    McpCatalogSnapshotBinding, PackAnnotations, PackArchiveRef, PackEntry, PackEntryKind,
    PackImportReceipt, PackImportResult, PackProducer, PackSection, CONNECTOR_PACK_SCHEMA_VERSION,
};
use medulla_h_decide::decide::capability_feed::{shared_capability_index, sweep, Reembedder};
use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use medulla_server_api::state::ServerState;
use medulla_server_auth::auth::{dispatch_test_on_heap, scoped_test_request_on, ScopedTestCaller};
use medulla_server_blob::blob::BlobStateExt;
use medulla_types::contract::{BoundedVec, Digest256, OpaqueId, ResourceId, TenantId};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

const SECRET: &str = "pack-feed-served-fixture-secret"; // sanitizer:ignore
/// The fixed tenant the served test auth policy expects.
const TENANT: &str = "tenant-shared";
/// A tenant that imported nothing.
const OTHER_TENANT: &str = "tenant-other";
/// The System-role agent the harness isolation registers.
const ADMIN: &str = "pack-feed-served-admin";
const COMMONS: &str = "__commons__";
const CONNECTOR: &str = "r172-12-3";
const ORDER: &str = "https://example.org/Order";
const SHIP: &str = "https://example.org/Ship";
const DIM: usize = 3;

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// `alpha` texts on the first axis, everything else on the second.
struct AxisEmbedder;

impl Reembedder for AxisEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Ok(texts
            .iter()
            .map(|text| {
                if text.contains("alpha") {
                    vec![1.0, 0.0, 0.0]
                } else {
                    vec![0.0, 1.0, 0.0]
                }
            })
            .collect())
    }
}

struct Served {
    _dir: tempfile::TempDir,
    state: Arc<RwLock<ServerState>>,
}

impl Served {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut state = medulla_server::seams_install::new_state_for_test(
            SECRET,
            ServerState::test_isolation(ADMIN),
        );
        state.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        let blob_dir = dir.path().join("blob");
        std::fs::create_dir_all(&blob_dir).unwrap();
        state.set_blob_cursors(Some(medulla_server_ops::api::ingest::cursors_for_test(
            blob_dir.to_str().unwrap(),
        )));
        let graphs = dir.path().join("graphs");
        std::fs::create_dir_all(&graphs).unwrap();
        state.persistence = Some(Arc::new(
            medulla_persistence::redb_backend::RedbBackend::open(
                graphs.to_string_lossy().into_owned(),
                64,
            )
            .expect("open the served graph backend"),
        ));
        Self {
            _dir: dir,
            state: Arc::new(RwLock::new(state)),
        }
    }

    /// Declare the commons graph's active embedding generation, the space the
    /// feed embeds under.
    async fn declare_generation(&self) -> String {
        let mut guard = self.state.write().await;
        if guard.registry.get(COMMONS).is_none() {
            guard
                .registry
                .create_graph(COMMONS, GraphType::Team, None)
                .unwrap();
        }
        let space = medulla_index_types::embedding::EmbeddingSpaceRef::pinned(
            "m", "1", "sha256:w", "sha256:p", DIM, false,
        )
        .unwrap();
        let digest = space.digest.clone();
        guard
            .registry
            .get(COMMONS)
            .unwrap()
            .core
            .semantic_store
            .write()
            .declare_space(space)
            .unwrap();
        digest
    }

    async fn call(&self, key: &str, method: Method) -> Response {
        let id = next_id();
        let request = scoped_test_request_on(
            SECRET,
            id,
            COMMONS,
            method,
            ScopedTestCaller {
                principal: ADMIN,
                tenant: TENANT,
                scopes: &["*"],
                nonce: &format!("pack-feed-nonce-{id}"),
                idempotency_key: key,
            },
        );
        dispatch_test_on_heap(&self.state, request).await
    }

    async fn pack(&self, key: &str, op: ConnectorPackOp) -> Response {
        self.call(key, Method::ConnectorPack { op: Box::new(op) })
            .await
    }

    async fn upload(&self, archive: &[u8]) -> String {
        let tag = format!("{}:{}", hex::encode(Sha256::digest(archive)), next_id());
        let cursor: u64 = ok(
            "BlobBegin",
            self.call(
                &format!("upload:{tag}:begin"),
                Method::BlobBegin {
                    chunk_size: 1 << 20,
                },
            )
            .await,
        );
        let chunk: serde_json::Value = ok(
            "BlobChunkPut",
            self.call(
                &format!("upload:{tag}:chunk"),
                Method::BlobChunkPut {
                    cursor,
                    data: archive.to_vec(),
                },
            )
            .await,
        );
        drop(chunk);
        ok(
            "BlobCommit",
            self.call(
                &format!("upload:{tag}:commit"),
                Method::BlobCommit { cursor },
            )
            .await,
        )
    }

    async fn bind(&self) {
        let key = format!("bind:{CONNECTOR}");
        let _: serde_json::Value = ok(
            "Bind",
            self.pack(
                &key,
                ConnectorPackOp::Bind {
                    request: ConnectorPackBindRequest {
                        context: context(&key),
                        connector: ResourceId::new(CONNECTOR).unwrap(),
                        importer: format!(
                            "principal:sha256:{}",
                            hex::encode(Sha256::digest(ADMIN.as_bytes()))
                        ),
                    },
                },
            )
            .await,
        );
    }

    /// Upload and import `pack` as the first head of the connector.
    async fn import(&self, pack: (ConnectorPackIndex, Vec<u8>)) -> PackImportReceipt {
        let (mut index, archive) = pack;
        index.archive.blob_digest = self.upload(&archive).await;
        let key = format!(
            "connector-pack:{CONNECTOR}:import:{digest}:0",
            digest = index.pack_digest
        );
        let response = self
            .pack(
                &key,
                ConnectorPackOp::Import {
                    request: Box::new(ConnectorPackImportRequest {
                        context: context(&key),
                        index,
                        expected_head: None,
                        allow_mass_withdrawal: false,
                    }),
                },
            )
            .await;
        match ok("Import", response) {
            PackImportResult::Imported { receipt } => *receipt,
            other => panic!("import did not land: {other:?}"),
        }
    }

    /// Run the projection worker until its outbox is drained.
    async fn project(&self) {
        let subscribed = AtomicBool::new(false);
        while medulla_h_admin::connector_pack_projection::worker::sweep(&self.state, &subscribed)
            .await
            .unwrap()
        {}
    }

    /// Run the capability feed over `tenant`'s outbox until it is drained.
    async fn feed(&self, tenant: &str) {
        let embedder: Arc<dyn Reembedder> = Arc::new(AxisEmbedder);
        let subscribed = AtomicBool::new(false);
        while sweep(&self.state, tenant, &embedder, &subscribed)
            .await
            .unwrap()
        {}
    }
}

fn ok<T: DeserializeOwned>(label: &str, response: Response) -> T {
    assert!(
        response.error.is_none(),
        "{label} was refused: {response:?}"
    );
    let value = match response.result {
        Some(ResultPayload::Raw(bytes)) => {
            return rmp_serde::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("{label} result does not decode: {error}"));
        }
        Some(ResultPayload::Count(count)) => serde_json::json!(count),
        Some(ResultPayload::String(text)) => serde_json::json!(text),
        Some(ResultPayload::Bool(flag)) => serde_json::json!(flag),
        Some(ResultPayload::Json(value)) => value,
        other => panic!("{label} answered an unexpected payload shape: {other:?}"),
    };
    serde_json::from_value(value)
        .unwrap_or_else(|error| panic!("{label} result does not decode: {error}"))
}

/// A request-body context shell; the engine re-binds every field of it.
fn context(key: &str) -> medulla_agent_types::agent_library::AgentLibraryMutationContext {
    medulla_agent_types::agent_library::AgentLibraryMutationContext {
        request_id: 1,
        principal: "bound-by-engine".to_string(),
        caller_principal: "bound-by-engine".to_string(),
        attempt_nonce: medulla_types::contract::Nonce::from_bytes([0; 32]),
        tenant_id: TENANT.to_string(),
        actor_scope: "bound-by-engine".to_string(),
        purpose_id: "bound-by-engine".to_string(),
        policy_revision: "bound-by-engine".to_string(),
        policy_digest: format!("sha256:{}", "0".repeat(64)),
        policy_decision_id: "bound-by-engine".to_string(),
        idempotency_key: key.to_string(),
        expected_revision: None,
        trace_id: None,
        created_at_ms: 1,
    }
}

fn section(archive: &mut Vec<u8>, bytes: &[u8]) -> PackSection {
    let section = PackSection {
        offset: archive.len() as u64,
        length: bytes.len() as u64,
        sha256: Digest256::from_bytes(Sha256::digest(bytes).into()),
    };
    archive.extend_from_slice(bytes);
    section
}

fn entry(
    archive: &mut Vec<u8>,
    kind: PackEntryKind,
    uri: String,
    name: &str,
    media_type: &str,
    body: &[u8],
    schema: Option<&[u8]>,
) -> PackEntry {
    PackEntry {
        kind,
        uri,
        name: name.to_string(),
        media_type: media_type.to_string(),
        body: section(archive, body),
        input_schema: schema.map(|schema| section(archive, schema)),
        output_schema: None,
        annotations: PackAnnotations {
            read_only_hint: (kind == PackEntryKind::Tool).then_some(true),
            ..PackAnnotations::default()
        },
        references: BoundedVec::new(Vec::new()).unwrap(),
    }
}

/// A pack of two tools and the ontology that declares what they act on: `a`
/// operates on `Order`, `b` executes a method of class `Ship`.
fn build_pack() -> (ConnectorPackIndex, Vec<u8>) {
    let mut archive = Vec::new();
    let server_body =
        format!(r#"{{"contract_version":"1","instructions":"Test.","name":"{CONNECTOR}"}}"#);
    let server = entry(
        &mut archive,
        PackEntryKind::McpServer,
        format!("mcp-server://{CONNECTOR}"),
        CONNECTOR,
        "application/json",
        server_body.as_bytes(),
        None,
    );
    let schema = br#"{"properties":{"q":{"type":"string"}},"type":"object"}"#;
    let ontology = format!(
        "@prefix cp: <https://knuckles-team.github.io/ontology/connector-pack#> .\n\
         @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
         <{ORDER}> a owl:Class ;\n\
         rdfs:subClassOf <https://knuckles-team.github.io/ontology/core#Agreement> .\n\
         <{SHIP}> a owl:Class ;\n\
         rdfs:subClassOf <https://knuckles-team.github.io/ontology/core#Agreement> .\n\
         <tool://{CONNECTOR}/a> cp:operatesOn <{ORDER}> .\n\
         <tool://{CONNECTOR}/b> cp:executes <{SHIP}> .\n"
    );
    let mut entries = vec![
        entry(
            &mut archive,
            PackEntryKind::Tool,
            format!("tool://{CONNECTOR}/a"),
            "a",
            "application/json",
            br#"{"description":"alpha lookup","name":"a"}"#,
            Some(schema),
        ),
        entry(
            &mut archive,
            PackEntryKind::Tool,
            format!("tool://{CONNECTOR}/b"),
            "b",
            "application/json",
            br#"{"description":"dispatch shipment","name":"b"}"#,
            Some(schema),
        ),
        entry(
            &mut archive,
            PackEntryKind::Ontology,
            format!("ontology://{CONNECTOR}/surface.ttl"),
            "surface.ttl",
            "text/turtle",
            ontology.as_bytes(),
            None,
        ),
    ];
    entries.sort_by(|left, right| left.uri.cmp(&right.uri));
    let mut index = ConnectorPackIndex {
        schema_version: CONNECTOR_PACK_SCHEMA_VERSION,
        connector: ResourceId::new(CONNECTOR).unwrap(),
        server,
        server_package_version: "1.0.0".to_string(),
        archive: PackArchiveRef {
            blob_digest: String::new(),
            length: archive.len() as u64,
            sha256: Digest256::from_bytes(Sha256::digest(&archive).into()),
        },
        entries: BoundedVec::new(entries).unwrap(),
        producer: PackProducer {
            name: "pack-feed-served-tests".to_string(),
            version: "1".to_string(),
        },
        catalog: McpCatalogSnapshotBinding {
            configuration_revision: 1,
            catalog_generation: 1,
            snapshot_digest: Digest256::from_bytes([1; 32]),
            child_connection_generation: 1,
            authorization_scope_digest: Digest256::from_bytes([2; 32]),
        },
        pack_digest: Digest256::from_bytes([0; 32]),
    };
    index.pack_digest = medulla_catalog_types::connector_pack::digest::pack_digest(&index).unwrap();
    (index, archive)
}

fn tool_id(name: &str) -> OpaqueId {
    OpaqueId::new(format!("mcp:{CONNECTOR}/tool/{name}")).unwrap()
}

// spec: MEDULLA-DECISION-ENGINE-R172.12.3
#[tokio::test]
async fn projection_commit_upserts_pack_records() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let served = Served::new();
    let version = served.declare_generation().await;
    served.bind().await;
    let receipt = served.import(build_pack()).await;
    let tenant = TenantId::new(TENANT).unwrap();
    let index = shared_capability_index(&*served.state.read().await);

    // The import alone makes the tools designatable by component, without any
    // declared class: the pack facts arrive with the projection commit.
    served.feed(TENANT).await;
    {
        let held = index.shard(Some(&tenant)).unwrap().snapshot();
        let record = held.record(&tool_id("a")).unwrap();
        assert!(record.applies_to.is_empty() && record.executes.is_empty());
        assert!(record.pack.is_none());
    }

    // The projection commit leaves its surface row; the feed upserts it.
    served.project().await;
    served.feed(TENANT).await;
    let held = index.shard(Some(&tenant)).unwrap().snapshot();
    let a = held.record(&tool_id("a")).unwrap();
    assert_eq!(
        a.applies_to.iter().map(String::as_str).collect::<Vec<_>>(),
        [ORDER]
    );
    assert!(a.executes.is_empty());
    let origin = a.pack.as_ref().expect("the record names its pack");
    assert_eq!(origin.connector, CONNECTOR);
    assert_eq!(origin.binding_revision, receipt.binding_revision);
    assert_eq!(origin.pack_digest, receipt.pack_digest.to_hex());
    let b = held.record(&tool_id("b")).unwrap();
    assert_eq!(
        b.executes.iter().map(String::as_str).collect::<Vec<_>>(),
        [SHIP]
    );
    assert!(b.applies_to.is_empty());
    assert_eq!(b.pack.as_ref().unwrap().connector, CONNECTOR);
    // Embedded under the active generation, and queryable by vector: the
    // alpha tool is the nearest to the alpha axis.
    assert_eq!((a.embedding_version.as_str(), a.dim), (version.as_str(), 3));
    let pool = held.filtered_pool(
        &Default::default(),
        &Default::default(),
        &Default::default(),
    );
    let best = held.search(&[1.0, 0.0, 0.0], &pool, 1, 64);
    assert_eq!(
        held.record_at(best[0].0).unwrap().entity_id,
        tool_id("a"),
        "the imported tool is queryable"
    );

    // Cross-tenant negative: another tenant's feed finds nothing of the pack.
    served.feed(OTHER_TENANT).await;
    let other = TenantId::new(OTHER_TENANT).unwrap();
    let live = index
        .shard(Some(&other))
        .map(|shard| shard.snapshot().live_count())
        .unwrap_or(0);
    assert_eq!(live, 0, "another tenant sees none of the pack's records");
}
