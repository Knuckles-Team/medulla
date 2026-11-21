// MEDULLA-MEDIA-VOCAB-R650.3 .. R650.5: the wire contract of the media derivation feature of
// `SourceIngest` and the served journeys (`served` below) over an imported connector pack: ingest
// an image with a caption, remember it, recall it by the caption embedding, refuse each bad
// case and read the stored evidence back. The admission and recall rules are also proved in
// `crates/medulla-h-graph/src/graph_ops/memory_surface_tests.rs`.

use medulla_source_types::source_ingestion::{fixtures, SourceIngestionRequest};
use medulla_types::media_derivation::MediaDerivationValue;
use serde_json::json;

fn derivation_feature() -> serde_json::Value {
    json!({
        "feature_id": "feature-caption",
        "object_id": "media-1",
        "kind": "media_derivation",
        "value_msgpack": rmp_serde::to_vec_named(&MediaDerivationValue {
            kind: "caption".into(),
            text: "a dog on a beach".into(),
            embedding: vec![1.0, 0.0],
        })
        .unwrap(),
        "model_version": "captioner-1",
    })
}

// spec: MEDULLA-MEDIA-VOCAB-R650.3
#[test]
fn media_derivation_feature_wire_contract() {
    let plain = fixtures::request();
    let mut batch = serde_json::to_value(plain.as_batch()).unwrap();
    batch["features"] = json!([derivation_feature()]);
    let request: SourceIngestionRequest = serde_json::from_value(batch).unwrap();
    assert_ne!(request.batch_digest().unwrap(), plain.batch_digest().unwrap());
    let stored: MediaDerivationValue =
        rmp_serde::from_slice(&request.as_batch().features.as_slice()[0].value_msgpack).unwrap();
    assert_eq!(stored.kind, "caption");
    assert_eq!(stored.embedding, vec![1.0, 0.0]);
}

#[cfg(all(unix, feature = "redb", feature = "blob", feature = "shacl", feature = "security"))]
mod served {
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
    use medulla_source_types::source_ingestion::SourceIngestionRequest;
    use medulla_types::contract::{BoundedVec, Digest256, Nonce, ResourceId, TenantId};
    use medulla_types::media_derivation::MediaDerivationValue;
    use medulla_work_types::memory::{
        MediaAttachment, MemoryKind, MemoryRecallRequest, MemoryRecallResult, MemoryRecord,
        MemoryRememberRequest, MemoryRememberResult, MemoryTier, MEMORY_RECORD_SCHEMA_VERSION,
    };
    use serde::de::DeserializeOwned;
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use tokio::sync::RwLock;

    const SECRET: &str = "media-memory-served-fixture-secret"; // sanitizer:ignore
    /// The fixed tenant the served test auth policy admits at the door.
    const TENANT: &str = "tenant-shared";
    /// A second tenant of the same store.
    const OTHER_TENANT: &str = "tenant-other";
    /// The System-role agent the harness isolation registers.
    const ADMIN: &str = "media-memory-admin";
    const COMMONS: &str = "__commons__";
    const GRAPH: &str = "media-memory";
    const CONNECTOR: &str = "media-source";
    const STREAM: &str = "image";
    const CAPTION: &str = "a dog on a beach";
    const PRODUCER: &str = "captioner-1";
    const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

    fn next_id() -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    fn ok<T: DeserializeOwned>(label: &str, response: Response) -> T {
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

    /// The refusal text of a refused response, whichever field carries it.
    fn refusal(label: &str, response: &Response) -> String {
        assert!(response.error.is_some(), "{label} was not refused: {response:?}");
        format!("{response:?}")
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

    /// A digest-honest pack whose `image` stream maps to `Document`.
    fn pack() -> (ConnectorPackIndex, Vec<u8>) {
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
            format!(r#"{{"contract_version":"1","instructions":"Test.","name":"{CONNECTOR}"}}"#);
        let server = entry(
            PackEntryKind::McpServer,
            format!("mcp-server://{CONNECTOR}"),
            CONNECTOR,
            section(&mut archive, server_body.as_bytes()),
        );
        let mapping = json!({
            "connector": CONNECTOR,
            "schema_mappings": {
                STREAM: {"fields": {"id": "identifier", "name": "label"}, "ontology_class": "Document"},
            },
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
            connector: ResourceId::new(CONNECTOR).unwrap(),
            server,
            server_package_version: "1.0.0".to_string(),
            archive: PackArchiveRef {
                blob_digest: String::new(),
                length: archive.len() as u64,
                sha256: Digest256::from_bytes(Sha256::digest(&archive).into()),
            },
            entries: BoundedVec::new(vec![resource]).unwrap(),
            producer: PackProducer {
                name: "media-memory-served-tests".to_string(),
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
        index.pack_digest =
            medulla_catalog_types::connector_pack::digest::pack_digest(&index).unwrap();
        (index, archive)
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
            if state.registry.get(COMMONS).is_none() {
                state.registry.create_graph(COMMONS, GraphType::Team, None).unwrap();
            }
            Self {
                _dir: dir,
                state: Arc::new(RwLock::new(state)),
            }
        }

        /// One request as `ADMIN` of the door tenant, on `graph`, through the signed dispatch.
        async fn call_on(&self, graph: &str, key: &str, method: Method) -> Response {
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
                    nonce: &format!("media-memory-nonce-{id}"),
                    idempotency_key: key,
                },
            );
            dispatch_test_on_heap(&self.state, request).await
        }

        /// One request as `tenant`'s verified context with `scopes`, through the chokepoint.
        async fn verified(&self, tenant: &str, scopes: &[&str], method: Method) -> Response {
            let request = Request {
                id: next_id(),
                graph: GRAPH.to_string(),
                auth_token: String::new(),
                agent_id: Some(ADMIN.to_string()),
                method,
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

        /// Upload `bytes` through the blob CAS, hold them under the caller's owner scope and
        /// return the content digest.
        async fn upload(&self, bytes: &[u8]) -> String {
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

        /// The ingest graph and the imported pack.
        async fn provision(&self) {
            let created = self
                .call_on(
                    GRAPH,
                    "create",
                    Method::CreateGraph {
                        graph_name: GRAPH.to_string(),
                        graph_type: GraphType::Global,
                    },
                )
                .await;
            assert!(created.error.is_none(), "CreateGraph: {created:?}");
            let key = format!("bind:{CONNECTOR}");
            let _: Value = ok(
                "Bind",
                self.call_on(
                    COMMONS,
                    &key,
                    Method::ConnectorPack {
                        op: Box::new(ConnectorPackOp::Bind {
                            request: ConnectorPackBindRequest {
                                context: context(&key),
                                connector: ResourceId::new(CONNECTOR).unwrap(),
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
            let (mut index, archive) = pack();
            index.archive.blob_digest = self.upload(&archive).await;
            let key = format!("connector-pack:{CONNECTOR}:import:{}:0", index.pack_digest);
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

        /// Ingest one image record with one derivation feature of `kind` and `text`.
        async fn ingest(&self, record_id: &str, kind: &str, text: &str) -> Response {
            static PAGE: AtomicU64 = AtomicU64::new(1);
            let value = json!({
                "connector": CONNECTOR, "kind": "records", "mode": "delta", "strict_schema": true,
                "records": [{
                    "stream": STREAM, "record_id": record_id,
                    "mapping_reference": format!("connector:{CONNECTOR}#mappings/{STREAM}"),
                    "payload": {"id": record_id, "name": "beach"},
                    "updated_at": "2026-11-13T00:00:00Z",
                    "provenance": {"connector": CONNECTOR, "adapter_kind": "mcp", "server": "media",
                                   "tool": "image", "tool_schema_sha256": "07".repeat(32),
                                   "source_uri": format!("media://{record_id}")},
                }],
                "features": [{
                    "feature_id": format!("feature-{record_id}"),
                    "object_id": node_id(record_id),
                    "kind": "media_derivation",
                    "value_msgpack": rmp_serde::to_vec_named(&MediaDerivationValue {
                        kind: kind.into(),
                        text: text.into(),
                        embedding: vec![1.0, 0.0],
                    })
                    .unwrap(),
                    "model_version": PRODUCER,
                }],
                "relationships": [], "withdrawals": [],
                "provider_checkpoint": {"stream": STREAM, "position": {"page": PAGE.fetch_add(1, Ordering::Relaxed)},
                                          "watermark": "2026-11-13T00:00:00Z"},
                "expected_previous_checkpoint": Value::Null,
            });
            let request: SourceIngestionRequest =
                serde_json::from_value(value).expect("a valid source ingestion request");
            self.call_on(
                GRAPH,
                &format!("ingest:{record_id}:{}", next_id()),
                Method::SourceIngest {
                    request: Box::new(request),
                },
            )
            .await
        }

        /// The ids a UQL statement answers as a reader holding only `kg:read`.
        async fn uql_ids(&self, tenant: &str, text: &str) -> Vec<String> {
            let response = self
                .verified(
                    tenant,
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
            let Some(ResultPayload::Raw(bytes)) = response.result.as_ref() else {
                panic!("a UQL statement answers with a raw result");
            };
            let UqlResult::Rows { rows, .. } =
                rmp_serde::from_slice(bytes).expect("a UQL result")
            else {
                panic!("the statement answers rows");
            };
            let mut ids: Vec<String> = rows.into_iter().map(|row| row.id).collect();
            ids.sort();
            ids
        }

        /// The stored properties of a node, read as a `kg:read` reader.
        async fn properties(&self, id: &str) -> Option<serde_json::Map<String, Value>> {
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

        async fn remember(&self, tenant: &str, record: MemoryRecord) -> Response {
            let method = Method::MemoryRemember {
                request: Box::new(MemoryRememberRequest {
                    record,
                    embedding: vec![0.0, 1.0],
                }),
            };
            if tenant == TENANT {
                self.call_on(GRAPH, &format!("remember:{}", next_id()), method).await
            } else {
                self.verified(tenant, &["*"], method).await
            }
        }

        async fn recall(&self, tenant: &str, include_media: bool) -> MemoryRecallResult {
            let method = Method::MemoryRecall {
                request: Box::new(MemoryRecallRequest {
                    query_embedding: vec![1.0, 0.0],
                    keywords: Vec::new(),
                    tiers: Vec::new(),
                    scope: None,
                    as_of_ms: None,
                    budget: 10_000,
                    prime_session: None,
                    include_plan: false,
                    include_media,
                }),
            };
            let response = if tenant == TENANT {
                self.call_on(GRAPH, &format!("recall:{}", next_id()), method).await
            } else {
                self.verified(tenant, &["*"], method).await
            };
            ok("MemoryRecall", response)
        }
    }

    fn node_id(record_id: &str) -> String {
        medulla_h_ingest::source_ingestion::source_record_node_id(TENANT, CONNECTOR, STREAM, record_id)
            .unwrap()
    }

    /// The id of the derivation the engine plans for a caption of the item whose raw record
    /// holds `raw_digest`: SHA-256 over the digest, a NUL, the kind, a NUL and the text.
    fn derivation_id(raw_digest: &str, kind: &str, text: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(raw_digest.as_bytes());
        hasher.update([0u8]);
        hasher.update(kind.as_bytes());
        hasher.update([0u8]);
        hasher.update(text.as_bytes());
        hex::encode(hasher.finalize())
    }

    fn memory(id: &str, tenant: &str, attachment: Option<MediaAttachment>) -> MemoryRecord {
        MemoryRecord {
            memory_id: ResourceId::new(id).unwrap(),
            tenant: TenantId::new(tenant).unwrap(),
            kind: MemoryKind::Episode,
            tier: MemoryTier::Episodic,
            text: "a photo from the trip".into(),
            provenance_key: None,
            session_id: None,
            timestamp_ms: 10,
            topics: Vec::new(),
            pattern_key: None,
            importance: 1.0,
            access_count: 0,
            last_access_ms: 0,
            last_decay_ms: 0,
            forgotten: false,
            legal_hold: false,
            expires_at_ms: None,
            support_count: 0,
            token_estimate: 10,
            attachments: attachment.into_iter().collect(),
            schema_version: MEMORY_RECORD_SCHEMA_VERSION,
        }
    }

    /// The ingested image, its caption derivation and the held PNG blob.
    struct Journey {
        served: Served,
        item: String,
        derivation: String,
        png: Vec<u8>,
        png_digest: String,
    }

    async fn journey(record_id: &str) -> Journey {
        let served = Served::new();
        served.provision().await;
        let ingested = served.ingest(record_id, "caption", CAPTION).await;
        let outcome: medulla_source_types::source_document::SourceIngestionOutcome =
            ok("SourceIngest", ingested);
        let receipt = outcome
            .into_accepted()
            .expect("the batch is accepted, not quarantined");
        let raw = receipt.raw_admissions.as_slice()[0].raw_digest.to_hex();
        let mut png = PNG_MAGIC.to_vec();
        png.extend_from_slice(&[7u8; 64]);
        let png_digest = served.upload(&png).await;
        Journey {
            served,
            item: node_id(record_id),
            derivation: derivation_id(&raw, "caption", CAPTION),
            png,
            png_digest,
        }
    }

    impl Journey {
        fn attachment(&self) -> MediaAttachment {
            MediaAttachment {
                blob_digest: self.png_digest.clone(),
                mime: "image/png".into(),
                byte_len: self.png.len() as u64,
                derivation_ids: vec![ResourceId::new(&self.derivation).unwrap()],
            }
        }
    }

    // spec: MEDULLA-MEDIA-VOCAB-R650.3
    #[tokio::test]
    async fn media_attachment_remember_and_recall() {
        let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let journey = journey("img-recall").await;
        let served = &journey.served;

        // The ingested caption is a derivation node of the item, keyed by the item's raw digest.
        let stored = served
            .properties(&journey.derivation)
            .await
            .expect("the derivation id is the one built from the ledger digest of the item");
        assert_eq!(stored["derivationText"], CAPTION);

        let remembered: MemoryRememberResult = ok(
            "MemoryRemember",
            served
                .remember(TENANT, memory("mem-img", TENANT, Some(journey.attachment())))
                .await,
        );
        assert!(remembered.stored);

        // The caption embedding recalls the image; the record comes back once with its
        // attachment intact.
        let found = served.recall(TENANT, true).await;
        let hit = found
            .hits
            .iter()
            .find(|hit| hit.record.memory_id.as_str() == "mem-img")
            .expect("the image is recalled by its caption embedding");
        assert!(hit.score_q16 > 0);
        assert_eq!(hit.record.attachments, vec![journey.attachment()]);
        assert_eq!(
            found
                .hits
                .iter()
                .filter(|hit| hit.record.memory_id.as_str() == "mem-img")
                .count(),
            1
        );

        // Cross-tenant negative: the other tenant recalls nothing of it and cannot attach a
        // blob its own owner scope does not hold.
        let other = served.recall(OTHER_TENANT, true).await;
        assert!(other.hits.iter().all(|hit| hit.record.memory_id.as_str() != "mem-img"));
        let refused = refusal(
            "other tenant remember",
            &served
                .remember(
                    OTHER_TENANT,
                    memory("mem-other", OTHER_TENANT, Some(journey.attachment())),
                )
                .await,
        );
        assert!(refused.contains("MEDIA_BLOB_UNKNOWN"), "{refused}");

        // `include_media` with no query embedding is a parameter error.
        let response = served
            .call_on(
                GRAPH,
                "recall-invalid",
                Method::MemoryRecall {
                    request: Box::new(MemoryRecallRequest {
                        query_embedding: Vec::new(),
                        keywords: Vec::new(),
                        tiers: Vec::new(),
                        scope: None,
                        as_of_ms: None,
                        budget: 100,
                        prime_session: None,
                        include_plan: false,
                        include_media: true,
                    }),
                },
            )
            .await;
        assert!(refusal("include_media without embedding", &response).contains("PARAMETER_INVALID"));
    }

    // spec: MEDULLA-MEDIA-VOCAB-R650.4
    #[tokio::test]
    async fn media_refusals() {
        let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let journey = journey("img-refusals").await;
        let served = &journey.served;
        let before_recall = served.recall(TENANT, true).await.hits;
        let before_derivations = served.uql_ids(TENANT, "MATCH (:MediaDerivation)").await;

        let text_digest = served.upload(b"plain text, no magic number").await;
        let unheld = hex::encode(Sha256::digest(b"never uploaded"));
        let cases: Vec<(&str, MediaAttachment)> = vec![
            (
                "MEDIA_BLOB_UNKNOWN",
                MediaAttachment {
                    blob_digest: unheld,
                    ..journey.attachment()
                },
            ),
            (
                "MEDIA_TOO_LARGE",
                MediaAttachment {
                    byte_len: medulla_work_types::memory::media_max_blob_bytes() + 1,
                    ..journey.attachment()
                },
            ),
            (
                "MEDIA_TYPE_MISMATCH",
                MediaAttachment {
                    mime: "image/jpeg".into(),
                    ..journey.attachment()
                },
            ),
            (
                "MEDIA_TYPE_UNSUPPORTED",
                MediaAttachment {
                    blob_digest: text_digest,
                    mime: "text/plain".into(),
                    byte_len: 27,
                    derivation_ids: Vec::new(),
                },
            ),
        ];
        for (code, attachment) in cases {
            let refused = refusal(
                code,
                &served
                    .remember(TENANT, memory("mem-refused", TENANT, Some(attachment)))
                    .await,
            );
            assert!(refused.contains(code), "{code}: {refused}");
        }

        // The two derivation refusals of the `SourceIngest` path.
        let empty = refusal(
            "empty derivation",
            &served.ingest("img-empty", "caption", "").await,
        );
        assert!(empty.contains("MEDIA_DERIVATION_EMPTY"), "{empty}");
        let kind = refusal(
            "invalid derivation kind",
            &served.ingest("img-kind", "poem", "text").await,
        );
        assert!(kind.contains("MEDIA_DERIVATION_KIND_INVALID"), "{kind}");

        // No refusal changed the store.
        assert!(served.properties("mem-refused").await.is_none());
        assert_eq!(served.recall(TENANT, true).await.hits, before_recall);
        assert_eq!(
            served.uql_ids(TENANT, "MATCH (:MediaDerivation)").await,
            before_derivations
        );
    }

    // spec: MEDULLA-MEDIA-VOCAB-R650.5
    #[tokio::test]
    async fn media_evidence_reads_back() {
        let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let journey = journey("img-evidence").await;
        let served = &journey.served;
        let _: MemoryRememberResult = ok(
            "MemoryRemember",
            served
                .remember(TENANT, memory("mem-evidence", TENANT, Some(journey.attachment())))
                .await,
        );

        // A reader holding only `kg:read` finds the derivation node by UQL, and its
        // `wasDerivedFrom` edge reaches the item.
        let derivations = served
            .uql_ids(TENANT, &format!("MATCH (:MediaDerivation) WHERE id = '{}'", journey.derivation))
            .await;
        assert_eq!(derivations, vec![journey.derivation.clone()]);
        let reached = served
            .uql_ids(
                TENANT,
                &format!(
                    "MATCH (:MediaDerivation) WHERE id = '{}' |> TRAVERSE -[:wasDerivedFrom]->{{1,1}}",
                    journey.derivation
                ),
            )
            .await;
        assert!(reached.contains(&journey.item), "{reached:?}");

        // Every stored field of the derivation node.
        let derivation = served.properties(&journey.derivation).await.expect("derivation node");
        assert_eq!(derivation["type"], "MediaDerivation");
        assert_eq!(derivation["derivationKind"], "caption");
        assert_eq!(derivation["derivationText"], CAPTION);
        assert_eq!(derivation["producerRevision"], PRODUCER);

        // Every stored field of the citing memory node: digest, length, MIME and derivation ids.
        let node = served.properties("mem-evidence").await.expect("memory node");
        let attachments = node["attachments"].as_array().expect("attachments array");
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0]["blob_digest"], journey.png_digest);
        assert_eq!(attachments[0]["mime"], "image/png");
        assert_eq!(attachments[0]["byte_len"], journey.png.len() as u64);
        assert_eq!(attachments[0]["derivation_ids"], json!([journey.derivation]));
    }
}
