//! MEDULLA-FEDERATED-QUERY-R058.4.11: two tenants hold rows in ONE store and each tenant's
//! served natural-language query (dialect `sparql`) returns only its own rows.
//!
//! The rows are the surface nodes of a connector pack each tenant imported: the served
//! SPARQL default graph unions the pack graphs of the caller's verified tenant, so the
//! pack graph is the tenant-keyed data the NL path reads. Every request goes through
//! the real verified dispatch under its tenant; the projection worker is swept exactly as
//! its spawned loop sweeps it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use medulla_catalog_types::connector_pack::{
    ConnectorPackBindRequest, ConnectorPackImportRequest, ConnectorPackIndex, ConnectorPackOp,
    McpCatalogSnapshotBinding, PackAnnotations, PackArchiveRef, PackEntry, PackEntryKind,
    PackImportResult, PackProducer, PackSection, CONNECTOR_PACK_SCHEMA_VERSION,
};
use medulla_identity_core_types::acl::RequestContextClaims;
use medulla_nlq::nl::context::{NlPlanStep, SchemaContext};
use medulla_nlq::nl::NlQueryError;
use medulla_protocol_types::protocol::{
    GraphType, Method, Request, Response, ResultPayload, SparqlResult,
};
use medulla_query_types::nl_query::NlDialect;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::server_context::ServerContext;
use medulla_server_api::state::ServerState;
use medulla_server_blob::blob::BlobStateExt;
use medulla_types::contract::{BoundedVec, Digest256, Nonce, ResourceId};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

/// The tenant the served test auth policy expects.
const TENANT: &str = "tenant-shared";
/// A second tenant that holds its own rows in the same store.
const OTHER_TENANT: &str = "tenant-other";
/// The System-role agent the harness isolation registers.
const ADMIN: &str = "nl-sparql-tenant-admin";
const COMMONS: &str = "__commons__";
const TENANT_CONNECTOR: &str = "nlsparql-alpha";
const OTHER_CONNECTOR: &str = "nlsparql-beta";
/// Lists every subject of the caller's visible graphs.
const QUESTION: &str = "SELECT ?s WHERE { ?s a ?class }";

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

struct EchoPlanner;

impl medulla_nlq::nl::NlPlanner for EchoPlanner {
    fn id(&self) -> String {
        "scripted".to_string()
    }

    fn revision(&self) -> String {
        "1".to_string()
    }

    fn plan(&self, nl: &str, _schema_hint: &str) -> Result<String, String> {
        Ok(nl.to_string())
    }
}

/// Answers the question text as the SPARQL.
struct EchoSparql;

impl medulla_nlq::nl::sparql_planner::SparqlNlPlanner for EchoSparql {
    fn plan_sparql(
        &self,
        _context: &SchemaContext,
        question: &str,
        _feedback: &[NlPlanStep],
    ) -> Result<String, NlQueryError> {
        Ok(question.to_string())
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
            "nl-sparql-tenant-fixture-secret", // sanitizer:ignore
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
            state
                .registry
                .create_graph(COMMONS, GraphType::Team, None)
                .unwrap();
        }
        Self {
            _dir: dir,
            state: Arc::new(RwLock::new(state)),
        }
    }

    /// One request as `ADMIN` in `tenant`, through the verified dispatch.
    async fn call(&self, tenant: &str, key: &str, method: Method) -> Response {
        let id = next_id();
        let claims = RequestContextClaims {
            principal: ADMIN.to_string(),
            agent_id: ADMIN.to_string(),
            tenant: tenant.to_string(),
            audience: "medulla-test".to_string(),
            policy_version: "policy-test".to_string(),
            scopes: vec!["*".to_string()],
            ..RequestContextClaims::default()
        };
        let mut nonce = [0u8; 32];
        nonce[..8].copy_from_slice(&id.to_be_bytes());
        let context = VerifiedRequestContext::from_verified_claims_with_nonce(
            claims,
            key.to_string(),
            Some(Nonce::from_bytes(nonce)),
        );
        let request = Request {
            id,
            graph: COMMONS.to_string(),
            auth_token: String::new(),
            agent_id: Some(ADMIN.to_string()),
            method,
            trace_id: None,
        };
        Box::pin(
            medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
                &self.state,
                request,
                context,
            ),
        )
        .await
    }

    async fn upload(&self, tenant: &str, archive: &[u8]) -> String {
        let tag = format!(
            "{tenant}:{}:{}",
            hex::encode(Sha256::digest(archive)),
            next_id()
        );
        let cursor: u64 = ok(
            "BlobBegin",
            self.call(
                tenant,
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
                tenant,
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
                tenant,
                &format!("upload:{tag}:commit"),
                Method::BlobCommit { cursor },
            )
            .await,
        )
    }

    /// Bind `connector` to `ADMIN` in `tenant`, upload and import its pack.
    async fn import(&self, tenant: &str, connector: &str) {
        let key = format!("bind:{tenant}:{connector}");
        let _: serde_json::Value = ok(
            "Bind",
            self.call(
                tenant,
                &key,
                Method::ConnectorPack {
                    op: Box::new(ConnectorPackOp::Bind {
                        request: ConnectorPackBindRequest {
                            context: context(tenant, &key),
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
        let (mut index, archive) = build_pack(connector);
        index.archive.blob_digest = self.upload(tenant, &archive).await;
        let key = format!(
            "connector-pack:{connector}:import:{digest}:0",
            digest = index.pack_digest
        );
        let response = self
            .call(
                tenant,
                &key,
                Method::ConnectorPack {
                    op: Box::new(ConnectorPackOp::Import {
                        request: Box::new(ConnectorPackImportRequest {
                            context: context(tenant, &key),
                            index,
                            expected_head: None,
                            allow_mass_withdrawal: false,
                        }),
                    }),
                },
            )
            .await;
        match ok("Import", response) {
            PackImportResult::Imported { .. } => {}
            other => panic!("import did not land: {other:?}"),
        }
    }

    /// Run the projection worker until the import outbox of each tenant is drained.
    ///
    /// The worker sweeps the import stream of the deployment tenant its request policy
    /// names, so the one store shared by both tenants is swept once under each tenant's
    /// own policy, exactly as that tenant's deployment would sweep it.
    async fn project(&self) {
        for tenant in [TENANT, OTHER_TENANT] {
            let policy_for = |name: &str| match name {
                "MEDULLA_AUDIENCE" => Some("medulla-test".to_string()),
                "MEDULLA_TENANT" => Some(tenant.to_string()),
                "MEDULLA_POLICY_VERSION" => Some("policy-test".to_string()),
                _ => None,
            };
            let own = Arc::new(ServerContext::build(None, &policy_for));
            let shared = std::mem::replace(&mut self.state.write().await.context, own);
            let subscribed = AtomicBool::new(false);
            while medulla_h_admin::connector_pack_projection::worker::sweep(
                &self.state,
                &subscribed,
            )
            .await
            .unwrap()
            {}
            self.state.write().await.context = shared;
        }
    }

    /// The subjects the served SPARQL answer of `tenant` holds for `QUESTION`, asked
    /// as natural language of dialect `sparql`.
    async fn nl_subjects(&self, tenant: &str) -> Vec<String> {
        let response = self
            .call(
                tenant,
                &format!("nl:{tenant}"),
                Method::NlQuery {
                    text: QUESTION.to_string(),
                    graph: COMMONS.to_string(),
                    history: Vec::new(),
                    dialect: NlDialect::Sparql,
                    max_steps: None,
                },
            )
            .await;
        subjects(ok("NlQuery", response))
    }

    /// The subjects a direct `Method::Sparql` of `tenant` answers for `QUESTION`.
    async fn direct_subjects(&self, tenant: &str) -> Vec<String> {
        let response = self
            .call(
                tenant,
                &format!("sparql:{tenant}"),
                Method::Sparql {
                    query: QUESTION.to_string(),
                    base_iri: String::new(),
                    type_convention: String::new(),
                    explain: false,
                    requires_packs: Vec::new(),
                    latency_class: Default::default(),
                    graph_media_type: None,
                },
            )
            .await;
        subjects(ok("Sparql", response))
    }
}

fn subjects(result: SparqlResult) -> Vec<String> {
    let column = result
        .vars
        .iter()
        .position(|var| var == "s")
        .expect("the answer projects ?s");
    let mut found: Vec<String> = result
        .rows
        .iter()
        .filter_map(|row| row[column].as_ref().map(|term| term.value.clone()))
        .collect();
    found.sort();
    found
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
fn context(
    tenant: &str,
    key: &str,
) -> medulla_agent_types::agent_library::AgentLibraryMutationContext {
    medulla_agent_types::agent_library::AgentLibraryMutationContext {
        request_id: 1,
        principal: "bound-by-engine".to_string(),
        caller_principal: "bound-by-engine".to_string(),
        attempt_nonce: Nonce::from_bytes([0; 32]),
        tenant_id: tenant.to_string(),
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
    (uri, name, media_type): (String, &str, &str),
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

/// A pack of one tool and the ontology class it operates on, all named by `connector`.
fn build_pack(connector: &str) -> (ConnectorPackIndex, Vec<u8>) {
    let mut archive = Vec::new();
    let server_body =
        format!(r#"{{"contract_version":"1","instructions":"Test.","name":"{connector}"}}"#);
    let server = entry(
        &mut archive,
        PackEntryKind::McpServer,
        (
            format!("mcp-server://{connector}"),
            connector,
            "application/json",
        ),
        server_body.as_bytes(),
        None,
    );
    let schema = br#"{"properties":{"q":{"type":"string"}},"type":"object"}"#;
    let ontology = format!(
        "@prefix cp: <https://knuckles-team.github.io/ontology/connector-pack#> .\n\
         @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
         <https://example.org/{connector}/Order> a owl:Class ;\n\
         rdfs:subClassOf <https://knuckles-team.github.io/ontology/core#Agreement> .\n\
         <tool://{connector}/a> cp:operatesOn <https://example.org/{connector}/Order> .\n"
    );
    let mut entries = vec![
        entry(
            &mut archive,
            PackEntryKind::Tool,
            (format!("tool://{connector}/a"), "a", "application/json"),
            br#"{"description":"alpha lookup","name":"a"}"#,
            Some(schema),
        ),
        entry(
            &mut archive,
            PackEntryKind::Ontology,
            (
                format!("ontology://{connector}/surface.ttl"),
                "surface.ttl",
                "text/turtle",
            ),
            ontology.as_bytes(),
            None,
        ),
    ];
    entries.sort_by(|left, right| left.uri.cmp(&right.uri));
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
        entries: BoundedVec::new(entries).unwrap(),
        producer: PackProducer {
            name: "nl-sparql-tenant-served-tests".to_string(),
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

// spec: MEDULLA-FEDERATED-QUERY-R058.4.11
#[tokio::test]
async fn nl_sparql_returns_only_the_callers_tenant_rows() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    medulla_h_query_read::nl::set_nl_planner(Arc::new(EchoPlanner));
    medulla_h_query_read::nl::set_sparql_nl_planner(Arc::new(EchoSparql));
    let served = Served::new();

    // Both tenants import a pack into the SAME store and both are projected, so each
    // tenant's rows are resident at once.
    served.import(TENANT, TENANT_CONNECTOR).await;
    served.import(OTHER_TENANT, OTHER_CONNECTOR).await;
    served.project().await;

    let own = served.nl_subjects(TENANT).await;
    let other = served.nl_subjects(OTHER_TENANT).await;

    // Each tenant's NL answer holds its own rows and none of the other tenant's.
    assert!(
        own.iter().any(|subject| subject.contains(TENANT_CONNECTOR)),
        "the first tenant reads its own rows: {own:?}"
    );
    assert!(
        own.iter().all(|subject| !subject.contains(OTHER_CONNECTOR)),
        "the first tenant reads none of the second tenant's rows: {own:?}"
    );
    assert!(
        other
            .iter()
            .any(|subject| subject.contains(OTHER_CONNECTOR)),
        "the second tenant reads its own rows: {other:?}"
    );
    assert!(
        other
            .iter()
            .all(|subject| !subject.contains(TENANT_CONNECTOR)),
        "the second tenant reads none of the first tenant's rows: {other:?}"
    );

    // The planned answer is exactly what a direct served SPARQL answers for the same
    // tenant, so the NL path adds no second visibility rule.
    assert_eq!(own, served.direct_subjects(TENANT).await);
    assert_eq!(other, served.direct_subjects(OTHER_TENANT).await);
}
