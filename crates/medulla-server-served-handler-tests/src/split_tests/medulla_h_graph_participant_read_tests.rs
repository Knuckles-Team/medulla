// MEDULLA-DECISION-ENGINE-R655.4: the served `ParticipantRead` reads one participant, the list and
// the derivation backlog a caller may see, under the scope `participant:read`, and another tenant's
// caller reads nothing.

#[cfg(all(
    unix,
    feature = "redb",
    feature = "blob",
    feature = "shacl",
    feature = "security"
))]
mod served {
    use medulla_decision_types::participant_memory::{DerivationStatus, ParticipantReadOp};
    use medulla_protocol_types::protocol::{Method, Response};
    use medulla_types::contract::{ResourceId, TenantId};
    use medulla_work_types::memory::{
        MemoryKind, MemoryRecord, MemoryRememberRequest, MemoryTier, MEMORY_RECORD_SCHEMA_VERSION,
    };
    use medulla_work_types::participant::{
        Participant, ParticipantId, ParticipantKind, ParticipantOp,
    };

    use medulla_served_test_harness::connector_pack_harness::{
        next_id, ok, Served, OTHER_TENANT, TENANT,
    };

    const GRAPH: &str = "participant-read";
    /// The harness administrator: the principal the participant acts as.
    const OWNER: &str = "connector-pack-admin";
    const SESSION: &str = "session-pr-1";

    fn slug(value: &str) -> ParticipantId {
        ParticipantId::new(value).expect("a valid slug")
    }

    fn turn(id: &str, speaker: &str, timestamp_ms: i64) -> MemoryRecord {
        MemoryRecord {
            memory_id: ResourceId::new(id).unwrap(),
            tenant: TenantId::new(TENANT).unwrap(),
            kind: MemoryKind::ConversationTurn,
            tier: MemoryTier::Episodic,
            text: format!("{speaker} said {id}"),
            provenance_key: None,
            session_id: Some(ResourceId::new(SESSION).unwrap()),
            speaker: Some(slug(speaker)),
            timestamp_ms,
            topics: Vec::new(),
            pattern_key: None,
            importance: 1.0,
            access_count: 0,
            last_access_ms: 0,
            last_decay_ms: 0,
            forgotten: false,
            legal_hold: false,
            expires_at_ms: None,
            support_count: 1,
            token_estimate: 10,
            attachments: Vec::new(),
            schema_version: MEMORY_RECORD_SCHEMA_VERSION,
        }
    }

    trait ParticipantReadServed: Sized {
        async fn pr_call(&self, method: Method) -> Response;
        async fn pr_read(&self, tenant: &str, scopes: &[&str], op: ParticipantReadOp) -> Response;
    }

    impl ParticipantReadServed for Served {
        async fn pr_call(&self, method: Method) -> Response {
            self.call_on(GRAPH, &format!("participant-read:{}", next_id()), method)
                .await
        }

        async fn pr_read(&self, tenant: &str, scopes: &[&str], op: ParticipantReadOp) -> Response {
            self.verified(tenant, scopes, Method::ParticipantRead { op: Box::new(op) })
                .await
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R655.4
    #[tokio::test]
    async fn participant_read_is_served_and_classified() {
        let served =
            Served::provisioned(GRAPH, "participant-read-source", &[("doc", "Document")]).await;

        // A participant the harness administrator acts as, held by one transcript thread.
        let ada = Participant::new(
            "ada",
            TENANT,
            "ws-main",
            ParticipantKind::Person,
            "Ada",
            Some("person:ada"),
            vec![OWNER.to_string()],
        )
        .unwrap();
        let put = served
            .pr_call(Method::Participant {
                op: Box::new(ParticipantOp::Put {
                    participant: ada.clone(),
                }),
            })
            .await;
        assert!(put.error.is_none(), "Put refused: {put:?}");
        {
            let guard = served.state().read().await;
            let core = guard
                .registry
                .get(GRAPH)
                .expect("the served graph")
                .core
                .clone();
            let thread = format!("transcript-thread:{SESSION}");
            core.add_node(
                thread.clone(),
                rmp_serde::to_vec_named(&serde_json::json!({
                    "type": "Transcript",
                    "connector": "ws-main",
                    "session_id": SESSION,
                }))
                .unwrap(),
            );
            core.add_edge(
                thread,
                ada.iri(),
                rmp_serde::to_vec_named(&serde_json::json!({"relationship": "hasParticipant"}))
                    .unwrap(),
            )
            .unwrap();
        }

        // Two turns of ada's session leave one derive task waiting.
        for (n, id) in ["pr-t1", "pr-t2"].iter().enumerate() {
            let remembered = served
                .pr_call(Method::MemoryRemember {
                    request: Box::new(MemoryRememberRequest {
                        record: turn(id, "ada", 1_000 + n as i64),
                        embedding: vec![1.0, 0.0],
                        episode: None,
                    }),
                })
                .await;
            assert!(
                remembered.error.is_none(),
                "MemoryRemember refused: {remembered:?}"
            );
        }

        // Get and List answer the stored participant under the read scope.
        let got: Participant = ok(
            "Get",
            served
                .pr_read(
                    TENANT,
                    &["participant:read"],
                    ParticipantReadOp::Get {
                        participant_id: slug("ada"),
                    },
                )
                .await,
        );
        assert_eq!(got.participant_id, slug("ada"));
        let listed: Vec<Participant> = ok(
            "List",
            served
                .pr_read(TENANT, &["participant:read"], ParticipantReadOp::List)
                .await,
        );
        assert_eq!(listed.len(), 1);

        // Status groups the waiting tasks by scope, session and subject.
        let status: Vec<DerivationStatus> = ok(
            "Status",
            served
                .pr_read(
                    TENANT,
                    &["participant:read"],
                    ParticipantReadOp::Status { memory_scope: None },
                )
                .await,
        );
        assert_eq!(status.len(), 1, "{status:?}");
        assert_eq!(status[0].session_id, SESSION);
        assert_eq!(status[0].subject, slug("ada"));
        assert!(status[0].pending >= 1);
        assert!(status[0].scope_id.is_empty());

        // The method is classified as a read behind its own scope: without it the call is denied,
        // and a caller of another tenant reads nothing of this tenant.
        let denied = served
            .pr_read(
                TENANT,
                &["kg:read"],
                ParticipantReadOp::Status { memory_scope: None },
            )
            .await;
        assert!(denied.error.expect("denied").contains("ACCESS_DENIED"));
        let foreign: Vec<DerivationStatus> = ok(
            "foreign Status",
            served
                .pr_read(
                    OTHER_TENANT,
                    &["participant:read"],
                    ParticipantReadOp::Status { memory_scope: None },
                )
                .await,
        );
        assert!(foreign.is_empty());
        let absent = served
            .pr_read(
                OTHER_TENANT,
                &["participant:read"],
                ParticipantReadOp::Get {
                    participant_id: slug("ada"),
                },
            )
            .await;
        assert!(absent
            .error
            .expect("absent")
            .contains("PARTICIPANT_NOT_VISIBLE"));
    }
}
