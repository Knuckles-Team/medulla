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
    use medulla_protocol_types::protocol::{Method, Response};
    use medulla_types::contract::{ResourceId, TenantId};
    use medulla_types::media_derivation::MediaDerivationValue;
    use medulla_work_types::memory::{
        MediaAttachment, MemoryKind, MemoryRecallRequest, MemoryRecallResult, MemoryRecord,
        MemoryRememberRequest, MemoryRememberResult, MemoryTier, MEMORY_RECORD_SCHEMA_VERSION,
    };
    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::super::connector_pack_harness::{
        next_id, ok, IngestOptions, Served, OTHER_TENANT, TENANT,
    };

    const GRAPH: &str = "media-memory";
    const CONNECTOR: &str = "media-source";
    const STREAM: &str = "image";
    const CAPTION: &str = "a dog on a beach";
    const PRODUCER: &str = "captioner-1";
    const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

    /// The refusal text of a refused response, whichever field carries it.
    fn refusal(label: &str, response: &Response) -> String {
        assert!(response.error.is_some(), "{label} was not refused: {response:?}");
        format!("{response:?}")
    }

    /// The media journeys' view of the shared connector-pack harness: one `image` stream that
    /// maps to `Document`.
    impl Served {
        async fn media_provisioned() -> Self {
            Served::provisioned(GRAPH, CONNECTOR, &[(STREAM, "Document")]).await
        }

        /// Ingest one image record with one derivation feature of `kind` and `text`, under a
        /// strict schema, and return the raw response.
        async fn ingest_media(&self, record_id: &str, kind: &str, text: &str) -> Response {
            let feature = json!({
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
            });
            self.ingest_with(
                STREAM,
                &[(record_id, json!({"id": record_id, "title": "beach"}))],
                &IngestOptions {
                    strict_schema: true,
                    features: vec![feature],
                },
            )
            .await
        }

        async fn remember(&self, tenant: &str, record: MemoryRecord) -> Response {
            let method = Method::MemoryRemember {
                request: Box::new(MemoryRememberRequest {
                    record,
                    embedding: vec![0.0, 1.0],
                    episode: None,
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
                    previous_manifest_digest: None,
                    document_set_ids: Vec::new(),
                    min_relevance: None,
                    include_archived: false,
                    current_revision: None,
                    episode_scope: None,
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
            speaker: None,
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
        let served = Served::media_provisioned().await;
        let ingested = served.ingest_media(record_id, "caption", CAPTION).await;
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
                        previous_manifest_digest: None,
                    document_set_ids: Vec::new(),
                    min_relevance: None,
                    include_archived: false,
                    current_revision: None,
                    episode_scope: None,
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
        let before_derivations = served.uql_ids("MATCH (:MediaDerivation)").await;

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
            &served.ingest_media("img-empty", "caption", "").await,
        );
        assert!(empty.contains("MEDIA_DERIVATION_EMPTY"), "{empty}");
        let kind = refusal(
            "invalid derivation kind",
            &served.ingest_media("img-kind", "poem", "text").await,
        );
        assert!(kind.contains("MEDIA_DERIVATION_KIND_INVALID"), "{kind}");

        // No refusal changed the store.
        assert!(served.properties("mem-refused").await.is_none());
        assert_eq!(served.recall(TENANT, true).await.hits, before_recall);
        assert_eq!(
            served.uql_ids("MATCH (:MediaDerivation)").await,
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
            .uql_ids(&format!("MATCH (:MediaDerivation) WHERE id = '{}'", journey.derivation))
            .await;
        assert_eq!(derivations, vec![journey.derivation.clone()]);
        let reached = served
            .uql_ids(
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
