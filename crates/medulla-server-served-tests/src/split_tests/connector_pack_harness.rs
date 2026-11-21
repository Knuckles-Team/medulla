// The served connector-pack harness of the `SourceIngest` journeys: one state with a graph
// backend and a blob store, a connector pack whose streams map to ontology classes, and the
// signed and verified dispatches a journey needs. A journey names its streams and reads back
// through `GetNodeProperties` and UQL as a `kg:read` reader.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use medulla_catalog_types::connector_pack::{
    ConnectorPackBindRequest, ConnectorPackImportRequest, ConnectorPackIndex, ConnectorPackOp,
    McpCatalogSnapshotBinding, PackAnnotations, PackArchiveRef, PackEntry, PackEntryKind,
    PackImportResult, PackProducer, PackSection, CONNECTOR_PACK_SCHEMA_VERSION,
};
use medulla_protocol_types::protocol::{GraphType, Method, Request, Response, ResultPayload};
use medulla_query_types::wire::UqlResult;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_server_auth::auth::{dispatch_test_on_heap, scoped_test_request_on, ScopedTestCaller};
use medulla_server_blob::blob::BlobStateExt;
use medulla_source_types::source_document::SourceIngestionOutcome;
use medulla_source_types::source_ingestion::{SourceIngestionReceipt, SourceIngestionRequest};
use medulla_types::contract::{BoundedVec, Digest256, Nonce, ResourceId};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

const SECRET: &str = "connector-pack-served-fixture-secret"; // sanitizer:ignore
/// The fixed tenant the served test auth policy admits at the door.
pub(super) const TENANT: &str = "tenant-shared";
/// A second tenant of the same store.
pub(super) const OTHER_TENANT: &str = "tenant-other";
/// The System-role agent the harness isolation registers.
const ADMIN: &str = "connector-pack-admin";
const COMMONS: &str = "__commons__";
/// The graph the connector-pack journeys ingest into by default.
pub(super) const GRAPH: &str = "connector-pack-ingest";

pub(super) fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The decoded result of a successful response.
pub(super) fn ok<T: DeserializeOwned>(label: &str, response: Response) -> T {
    assert!(response.error.is_none(), "{label} was refused: {response:?}");
    let value = match response.result {
        Some(ResultPayload::Raw(bytes)) => {
            return rmp_serde::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("{label} result does not decode: {error}"));
        }
        Some(ResultPayload::Count(count)) => json!(count),
        Some(ResultPayload::String(text)) => json!(text),
        Some(ResultPayload::Bool(flag)) => json!(flag),
        Some(ResultPayload::Json(value)) => value,
        other => panic!("{label} answered an unexpected payload shape: {other:?}"),
    };
    serde_json::from_value(value)
        .unwrap_or_else(|error| panic!("{label} result does not decode: {error}"))
}

fn context(key: &str) -> medulla_agent_types::agent_library::AgentLibraryMutationContext {
    medulla_agent_types::agent_library::AgentLibraryMutationContext {
        request_id: 1,
        principal: "bound-by-engine".to_string(),
        caller_principal: "bound-by-engine".to_string(),
        attempt_nonce: Nonce::from_bytes([0; 32]),
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

/// A digest-honest pack of `connector` whose `streams` (name, ontology class) map to their class.
fn pack(connector: &str, streams: &[(&str, &str)]) -> (ConnectorPackIndex, Vec<u8>) {
    let mut archive = Vec::new();
    let entry = |kind, uri: String, name: &str, body: PackSection| PackEntry {
        kind,
        uri,
        name: name.to_string(),
        media_type: "application/json".to_string(),
        body,
        input_schema: None,
        output_schema: None,
        annotations: PackAnnotations::default(),
        references: BoundedVec::new(Vec::new()).unwrap(),
    };
    let server_body =
        format!(r#"{{"contract_version":"1","instructions":"Test.","name":"{connector}"}}"#);
    let server = entry(
        PackEntryKind::McpServer,
        format!("mcp-server://{connector}"),
        connector,
        section(&mut archive, server_body.as_bytes()),
    );
    let mappings: serde_json::Map<String, Value> = streams
        .iter()
        .map(|(stream, class)| {
            (
                (*stream).to_string(),
                json!({"fields": {"id": "identifier", "title": "label"}, "ontology_class": class}),
            )
        })
        .collect();
    let mapping = json!({
        "connector": connector,
        "schema_mappings": mappings,
        "sources": [],
    })
    .to_string();
    let resource = entry(
        PackEntryKind::Resource,
        "connector://resources".to_string(),
        "connector-resources",
        section(&mut archive, mapping.as_bytes()),
    );
    let mut index = ConnectorPackIndex {
        schema_version: CONNECTOR_PACK_SCHEMA_VERSION,
        connector: ResourceId::new(connector).unwrap(),
        server,
        server_package_version: "1.0.0".to_string(),
        archive: PackArchiveRef {
            blob_digest: String::new(),
            length: archive.len() as u64,
            sha256: Digest256::from_bytes(Sha256::digest(&archive).into()),
        },
        entries: BoundedVec::new(vec![resource]).unwrap(),
        producer: PackProducer {
            name: "connector-pack-served-tests".to_string(),
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

/// One served state holding an ingest graph and an imported connector pack.
pub(super) struct Served {
    _dir: tempfile::TempDir,
    state: Arc<RwLock<ServerState>>,
    connector: String,
    /// The graph the pack's records are ingested into.
    graph: String,
}

/// How one `SourceIngest` envelope differs from the plain delta of a journey.
#[derive(Default)]
pub(super) struct IngestOptions {
    /// Refuse a record that carries a field its mapping does not name.
    pub(super) strict_schema: bool,
    /// The envelope's derived features, as the wire carries them.
    pub(super) features: Vec<Value>,
}

impl Served {
    /// A fresh state, the ingest graph `graph`, and the pack of `connector` with its `streams`.
    pub(super) async fn provisioned(
        graph: &str,
        connector: &str,
        streams: &[(&str, &str)],
    ) -> Self {
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
        if state.registry.get(COMMONS).is_none() {
            state.registry.create_graph(COMMONS, GraphType::Team, None).unwrap();
        }
        let served = Self {
            _dir: dir,
            state: Arc::new(RwLock::new(state)),
            connector: connector.to_string(),
            graph: graph.to_string(),
        };
        served.provision(streams).await;
        served
    }

    /// The graph the pack's records are ingested into.
    pub(super) fn graph(&self) -> &str {
        &self.graph
    }

    /// One request as `ADMIN` of the door tenant, on `graph`, through the signed dispatch.
    pub(super) async fn call_on(&self, graph: &str, key: &str, method: Method) -> Response {
        let id = next_id();
        let request = scoped_test_request_on(
            SECRET,
            id,
            graph,
            method,
            ScopedTestCaller {
                principal: ADMIN,
                tenant: TENANT,
                scopes: &["*"],
                nonce: &format!("connector-pack-nonce-{id}"),
                idempotency_key: key,
            },
        );
        dispatch_test_on_heap(&self.state, request).await
    }

    /// One request as `tenant`'s verified context with `scopes`, through the chokepoint.
    pub(super) async fn verified(
        &self,
        tenant: &str,
        scopes: &[&str],
        method: Method,
    ) -> Response {
        let request = Request {
            id: next_id(),
            graph: self.graph.clone(),
            auth_token: String::new(),
            agent_id: Some(ADMIN.to_string()),
            method,
            trace_id: None,
        };
        let context = VerifiedRequestContext::verified_for_test_with_scopes(ADMIN, tenant, scopes);
        Box::pin(
            medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
                &self.state,
                request,
                context,
            ),
        )
        .await
    }

    /// Upload `bytes` through the blob CAS, hold them under the caller's owner scope and return
    /// the content digest.
    pub(super) async fn upload(&self, bytes: &[u8]) -> String {
        let tag = format!("upload:{}:{}", hex::encode(Sha256::digest(bytes)), next_id());
        let cursor: u64 = ok(
            "BlobBegin",
            self.call_on(COMMONS, &format!("{tag}:begin"), Method::BlobBegin { chunk_size: 1 << 20 })
                .await,
        );
        let _: Value = ok(
            "BlobChunkPut",
            self.call_on(
                COMMONS,
                &format!("{tag}:chunk"),
                Method::BlobChunkPut {
                    cursor,
                    data: bytes.to_vec(),
                },
            )
            .await,
        );
        let digest: String = ok(
            "BlobCommit",
            self.call_on(COMMONS, &format!("{tag}:commit"), Method::BlobCommit { cursor })
                .await,
        );
        let _: Value = ok(
            "BlobRef",
            self.call_on(
                COMMONS,
                &format!("{tag}:ref"),
                Method::BlobRef {
                    digest: digest.clone(),
                },
            )
            .await,
        );
        digest
    }

    async fn provision(&self, streams: &[(&str, &str)]) {
        let created = self
            .call_on(
                &self.graph,
                "create",
                Method::CreateGraph {
                    graph_name: self.graph.clone(),
                    graph_type: GraphType::Global,
                },
            )
            .await;
        assert!(created.error.is_none(), "CreateGraph: {created:?}");
        let connector = self.connector.as_str();
        let key = format!("bind:{connector}");
        let _: Value = ok(
            "Bind",
            self.call_on(
                COMMONS,
                &key,
                Method::ConnectorPack {
                    op: Box::new(ConnectorPackOp::Bind {
                        request: ConnectorPackBindRequest {
                            context: context(&key),
                            connector: ResourceId::new(connector).unwrap(),
                            importer: format!(
                                "principal:sha256:{}",
                                hex::encode(Sha256::digest(ADMIN.as_bytes()))
                            ),
                        },
                    }),
                },
            )
            .await,
        );
        let (mut index, archive) = pack(connector, streams);
        index.archive.blob_digest = self.upload(&archive).await;
        let key = format!("connector-pack:{connector}:import:{}:0", index.pack_digest);
        let response = self
            .call_on(
                COMMONS,
                &key,
                Method::ConnectorPack {
                    op: Box::new(ConnectorPackOp::Import {
                        request: Box::new(ConnectorPackImportRequest {
                            context: context(&key),
                            index,
                            expected_head: None,
                            allow_mass_withdrawal: false,
                        }),
                    }),
                },
            )
            .await;
        match ok::<PackImportResult>("Import", response) {
            PackImportResult::Imported { .. } => {}
            other => panic!("the pack did not import: {other:?}"),
        }
    }

    /// Ingest `records` of `stream`, `(record_id, payload)` each, in one plain delta envelope and
    /// return the accepted receipt.
    pub(super) async fn ingest(
        &self,
        stream: &str,
        records: &[(&str, Value)],
    ) -> SourceIngestionReceipt {
        let response = self
            .ingest_with(stream, records, &IngestOptions::default())
            .await;
        ok::<SourceIngestionOutcome>("SourceIngest", response)
            .into_accepted()
            .unwrap_or_else(|quarantine| panic!("the batch was quarantined: {quarantine:?}"))
    }

    /// Ingest `records` of `stream` in one envelope shaped by `options`, and return the raw
    /// response, so a journey can assert a refusal.
    pub(super) async fn ingest_with(
        &self,
        stream: &str,
        records: &[(&str, Value)],
        options: &IngestOptions,
    ) -> Response {
        static PAGE: AtomicU64 = AtomicU64::new(1);
        let connector = self.connector.as_str();
        let records: Vec<Value> = records
            .iter()
            .map(|(record_id, payload)| {
                json!({
                    "stream": stream, "record_id": record_id,
                    "mapping_reference": format!("connector:{connector}#mappings/{stream}"),
                    "payload": payload,
                    "updated_at": "2026-11-13T00:00:00Z",
                    "provenance": {"connector": connector, "adapter_kind": "mcp", "server": "source",
                                   "tool": stream, "tool_schema_sha256": "07".repeat(32),
                                   "source_uri": format!("source://{record_id}")},
                })
            })
            .collect();
        let mut value = json!({
            "connector": connector, "kind": "records", "mode": "delta",
            "strict_schema": options.strict_schema,
            "records": records,
            "relationships": [], "withdrawals": [],
            "provider_checkpoint": {"stream": stream, "position": {"page": PAGE.fetch_add(1, Ordering::Relaxed)},
                                      "watermark": "2026-11-13T00:00:00Z"},
            "expected_previous_checkpoint": Value::Null,
        });
        if !options.features.is_empty() {
            value["features"] = Value::Array(options.features.clone());
        }
        let request: SourceIngestionRequest =
            serde_json::from_value(value).expect("a valid source ingestion request");
        self.call_on(
            &self.graph,
            &format!("ingest:{stream}:{}", next_id()),
            Method::SourceIngest {
                request: Box::new(request),
            },
        )
        .await
    }

    /// The ids a UQL statement answers as a reader of `TENANT` holding only `kg:read`.
    pub(super) async fn uql_ids(&self, text: &str) -> Vec<String> {
        let response = self
            .verified(
                TENANT,
                &["kg:read"],
                Method::Uql {
                    text: text.to_string(),
                    params: Default::default(),
                    prepare: false,
                    foreign_executions: Default::default(),
                },
            )
            .await;
        assert!(response.error.is_none(), "{text}: {response:?}");
        let UqlResult::Rows { rows, .. } = ok::<UqlResult>("Uql", response) else {
            panic!("the statement answers rows");
        };
        let mut ids: Vec<String> = rows.into_iter().map(|row| row.id).collect();
        ids.sort();
        ids
    }

    /// The stored properties of a node, read as a `kg:read` reader of `TENANT`.
    pub(super) async fn properties(&self, id: &str) -> Option<serde_json::Map<String, Value>> {
        let response = self
            .verified(
                TENANT,
                &["kg:read"],
                Method::GetNodeProperties {
                    node_id: id.to_string(),
                },
            )
            .await;
        if response.error.is_some() {
            return None;
        }
        let value: Value = ok("GetNodeProperties", response);
        value.as_object().cloned()
    }
}
