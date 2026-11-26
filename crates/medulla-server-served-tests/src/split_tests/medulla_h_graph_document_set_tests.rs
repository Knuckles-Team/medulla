// MEDULLA-DECISION-ENGINE-R656.3: a named document set narrows `MemoryRecall` and `Discover`,
// the ACL still applies to every set and result, and another tenant's set is not found.

#[cfg(all(unix, feature = "redb", feature = "blob", feature = "shacl", feature = "security"))]
mod served {
    use medulla_identity_types::acl::{GrantEffect, RbacAction};
    use medulla_identity_types::assistant_profile::{
        DocumentSet, DocumentSetKind, ProfileOp, ProfileShare, ASSISTANT_PROFILE_SCHEMA_VERSION,
    };
    use medulla_protocol_types::protocol::{Method, Response};
    use medulla_types::contract::{ResourceId, TenantId};
    use medulla_work_types::memory::{
        MemoryKind, MemoryRecallRequest, MemoryRecallResult, MemoryRecord, MemoryRememberRequest,
        MemoryTier, MEMORY_RECORD_SCHEMA_VERSION,
    };
    use serde_json::json;

    use super::super::connector_pack_harness::{next_id, ok, Served, OTHER_TENANT, TENANT};

    const GRAPH: &str = "document-set-retrieval";
    /// The harness administrator, who owns the sets this journey saves.
    const OWNER: &str = "connector-pack-admin";

    fn set(set_id: &str, members: &[&str]) -> DocumentSet {
        DocumentSet {
            set_id: set_id.into(),
            tenant: TENANT.into(),
            name: set_id.into(),
            kind: DocumentSetKind::Members(members.iter().map(|m| m.to_string()).collect()),
            owner_principal: OWNER.into(),
            shares: vec![ProfileShare {
                role: "analyst".into(),
                action: RbacAction::Read,
                effect: GrantEffect::Allow,
            }],
            schema_version: ASSISTANT_PROFILE_SCHEMA_VERSION,
        }
    }

    fn memory(id: &str, document: &str) -> MemoryRecord {
        MemoryRecord {
            memory_id: ResourceId::new(id).unwrap(),
            tenant: TenantId::new(TENANT).unwrap(),
            kind: MemoryKind::Episode,
            tier: MemoryTier::Episodic,
            text: format!("note about {document}"),
            provenance_key: Some(ResourceId::new(document).unwrap()),
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
            attachments: Vec::new(),
            speaker: None,
            schema_version: MEMORY_RECORD_SCHEMA_VERSION,
        }
    }

    impl Served {
        async fn ds_as_tenant(&self, tenant: &str, method: Method) -> Response {
            if tenant == TENANT {
                self.call_on(GRAPH, &format!("document-set:{}", next_id()), method).await
            } else {
                self.verified(tenant, &["*"], method).await
            }
        }

        async fn ds_put_set(&self, set: DocumentSet) {
            let response = self
                .ds_as_tenant(
                    TENANT,
                    Method::AssistantProfile {
                        op: Box::new(ProfileOp::PutDocumentSet { set }),
                    },
                )
                .await;
            assert!(response.error.is_none(), "PutDocumentSet refused: {response:?}");
        }

        async fn ds_recall(&self, tenant: &str, document_set_ids: &[&str]) -> Response {
            self.ds_as_tenant(
                tenant,
                Method::MemoryRecall {
                    request: Box::new(MemoryRecallRequest {
                        query_embedding: vec![0.0, 1.0],
                        keywords: Vec::new(),
                        tiers: Vec::new(),
                        scope: None,
                        as_of_ms: None,
                        budget: 10_000,
                        prime_session: None,
                        include_plan: false,
                        include_media: false,
                        previous_manifest_digest: None,
                        document_set_ids: document_set_ids.iter().map(|s| s.to_string()).collect(),
                        min_relevance: None,
                        include_archived: false,
                        current_revision: None,
                        episode_scope: None,
                    }),
                },
            )
            .await
        }

        async fn ds_discover(&self, tenant: &str, document_set_ids: &[&str]) -> Response {
            self.ds_as_tenant(
                tenant,
                Method::Discover {
                    keywords: vec!["note".into()],
                    query_embedding: Vec::new(),
                    k: 10,
                    quality_policy_ref: None,
                    hops: 0,
                    query_text: None,
                    mode: Default::default(),
                    as_of: None,
                    perspectives: None,
                    capability: None,
                    document_set_ids: document_set_ids.iter().map(|s| s.to_string()).collect(),
                    agent_ref: None,
                },
            )
            .await
        }
    }

    fn recalled(response: Response) -> Vec<String> {
        let result: MemoryRecallResult = ok("MemoryRecall", response);
        let mut ids: Vec<String> = result
            .hits
            .iter()
            .map(|hit| hit.record.memory_id.as_str().to_string())
            .collect();
        ids.sort();
        ids
    }

    // spec: MEDULLA-DECISION-ENGINE-R656.3
    #[tokio::test]
    async fn document_set_narrows_retrieval_and_acl_still_applies() {
        let served = Served::provisioned(GRAPH, "document-set-source", &[("doc", "Document")]).await;
        for document in ["doc-a", "doc-b"] {
            let response = served
                .ds_as_tenant(
                    TENANT,
                    Method::AddNode {
                        node_id: document.into(),
                        properties_msgpack: rmp_serde::to_vec_named(
                            &json!({"type": "Document", "name": format!("note about {document}")}),
                        )
                        .unwrap(),
                    },
                )
                .await;
            assert!(response.error.is_none(), "AddNode refused: {response:?}");
            let response = served
                .ds_as_tenant(
                    TENANT,
                    Method::MemoryRemember {
                        request: Box::new(MemoryRememberRequest {
                            record: memory(&format!("m-{document}"), document),
                            embedding: vec![0.0, 1.0],
                            episode: None,
                        }),
                    },
                )
                .await;
            assert!(response.error.is_none(), "MemoryRemember refused: {response:?}");
        }
        served.ds_put_set(set("only-a", &["doc-a"])).await;
        served.ds_put_set(set("a-and-ghost", &["doc-a", "doc-ghost"])).await;
        served.ds_put_set(set("empty", &[])).await;

        // No set: every record. One set: only its documents; the union of sets adds.
        assert_eq!(recalled(served.ds_recall(TENANT, &[]).await), vec!["m-doc-a", "m-doc-b"]);
        assert_eq!(recalled(served.ds_recall(TENANT, &["only-a"]).await), vec!["m-doc-a"]);
        // A member that is not visible narrows nothing in; it is dropped, not an error.
        assert_eq!(
            recalled(served.ds_recall(TENANT, &["a-and-ghost"]).await),
            vec!["m-doc-a"]
        );
        // An empty set returns no candidates.
        assert!(recalled(served.ds_recall(TENANT, &["empty"]).await).is_empty());

        // Discover narrows the same way.
        let found: serde_json::Value = ok("Discover", served.ds_discover(TENANT, &["only-a"]).await);
        let ids: Vec<&str> = found["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hit| hit["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["doc-a"]);

        // Unknown set and another tenant's caller both refuse DOCUMENT_SET_NOT_FOUND.
        for response in [
            served.ds_recall(TENANT, &["no-such-set"]).await,
            served.ds_recall(OTHER_TENANT, &["only-a"]).await,
            served.ds_discover(OTHER_TENANT, &["only-a"]).await,
        ] {
            let detail = format!("{response:?}");
            assert!(detail.contains("DOCUMENT_SET_NOT_FOUND"), "{detail}");
        }
    }
}
