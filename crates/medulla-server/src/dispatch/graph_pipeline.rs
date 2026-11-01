use medulla_server_blob::blob::BlobStateExt;
#[cfg(test)]
use super::change_envelope::decode_multi_graph_batches;
use super::change_envelope::route_change_envelope_ops;
#[cfg(feature = "redb")]
use super::change_envelope::work_item_capability_authority_epoch;
#[cfg(all(test, feature = "ast"))]
use super::request_boundary::preflight::{decode_ast_files, decode_ast_sources, AstInputLimits};
#[cfg(test)]
use super::request_boundary::screen::decode_screen_observation;
#[cfg(test)]
use super::request_boundary::preflight::preflight_request_msgpack;
use super::*;
mod dispatch_helpers;
#[cfg(any(
    feature = "query",
    feature = "cypher",
    feature = "graphql",
    feature = "rdf"
))]
mod gateway;
mod graph_access;
mod graph_dispatch;
#[cfg(all(feature = "raft", feature = "modality-serving"))]
mod modality;
mod native_routes;
mod pipeline;
// EH-280 — the durable, graph-scoped route of a branch-aware IndexRepository.
mod repository_index;
mod web_ingest;
#[cfg(all(feature = "ast", feature = "redb", feature = "blob"))]
pub(crate) use repository_index::drain_repository_enrichment_once;
#[cfg(all(feature = "ast", feature = "redb", feature = "blob", feature = "raft"))]
pub(crate) use repository_index::plan_held_underfunded_park;
#[cfg(all(
    feature = "ast",
    feature = "redb",
    feature = "blob",
    feature = "raft",
    feature = "security"
))]
pub(crate) use repository_index::{
    decode_pending_enrichment_intent, enrichment_intent_for_snapshot,
};
mod work_governance;
use dispatch_helpers::*;
#[cfg(any(
    feature = "query",
    feature = "cypher",
    feature = "graphql",
    feature = "rdf"
))]
use gateway::*;
use graph_access::*;
use graph_dispatch::dispatch_graph_op_inner;
pub(super) use graph_dispatch::GraphOpRouting;
#[cfg(all(feature = "raft", feature = "modality-serving"))]
use modality::*;
use native_routes::*;
use pipeline::*;
#[cfg(feature = "raft")]
use work_governance::enforce_native_read_leadership;
use work_governance::{
    dispatch_op_capacity_ops, dispatch_op_resource_reservation_query,
    dispatch_op_workitem_claim_capability, dispatch_op_workitem_submission_or_resources,
    NativeOpCtx,
};

/// Dispatch a graph-level operation to the target named graph, enforcing the
/// isolation ACL (`isolation.rs::check_access`) when rules are registered.
pub(super) async fn dispatch_graph_op(
    state: &Arc<RwLock<ServerState>>,
    graph_name: &str,
    req_id: u64,
    caller: Option<&str>,
    verified_context: &VerifiedRequestContext,
    method: Method,
) -> Response {
    dispatch_graph_op_inner(
        state,
        GraphOpContext {
            graph_name,
            req_id,
            caller,
            verified_context,
        },
        method,
        #[cfg(feature = "modality-serving")]
        None,
        #[cfg(feature = "knowledge-batch")]
        None,
    )
    .await
}

#[cfg(feature = "modality-serving")]
pub(super) async fn dispatch_served_modality(
    state: &Arc<RwLock<ServerState>>,
    graph_name: &str,
    req_id: u64,
    caller: Option<&str>,
    verified_context: &VerifiedRequestContext,
    op: medulla_types::ServedModalityOp,
    authority: medulla_h_ingest::modality::ModalityAuthority,
) -> Response {
    dispatch_graph_op_inner(
        state,
        GraphOpContext {
            graph_name,
            req_id,
            caller,
            verified_context,
        },
        Method::ServedModality { op },
        Some(authority),
        #[cfg(feature = "knowledge-batch")]
        None,
    )
    .await
}

#[cfg(feature = "knowledge-batch")]
pub(super) async fn dispatch_knowledge_stream(
    state: &Arc<RwLock<ServerState>>,
    graph_name: &str,
    req_id: u64,
    caller: Option<&str>,
    verified_context: &VerifiedRequestContext,
    request: medulla_types::knowledge_stream::KnowledgeStreamRequest,
    authority: medulla_h_query::knowledge_stream::KnowledgeStreamAuthority,
) -> Response {
    dispatch_graph_op_inner(
        state,
        GraphOpContext {
            graph_name,
            req_id,
            caller,
            verified_context,
        },
        Method::KnowledgeStream { request },
        #[cfg(feature = "modality-serving")]
        None,
        Some(authority),
    )
    .await
}

#[cfg(all(test, feature = "raft", feature = "modality-serving"))]
mod modality_replay_receipt_tests {
    use super::*;
    use serde::Serialize;

    fn encode_modality_replay_wire<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
        let inner = rmp_serde::to_vec_named(value).unwrap();
        rmp_serde::to_vec_named(&ResultPayload::Raw(inner)).unwrap()
    }

    fn single_wire() -> Vec<u8> {
        let outcome = medulla_modality::ApplyOutcome {
            disposition: medulla_modality::ApplyDisposition::Applied,
            observation_version: 11,
            event_sequence: 17,
        };
        encode_modality_replay_wire(&outcome)
    }

    fn stream_wire() -> Vec<u8> {
        let outcomes = vec![
            medulla_modality::ApplyOutcome {
                disposition: medulla_modality::ApplyDisposition::Applied,
                observation_version: 11,
                event_sequence: 17,
            },
            medulla_modality::ApplyOutcome {
                disposition: medulla_modality::ApplyDisposition::IdempotentReplay,
                observation_version: 12,
                event_sequence: 18,
            },
        ];
        encode_modality_replay_wire(&outcomes)
    }

    #[test]
    fn replay_decoder_accepts_single_receipt() {
        let payload = medulla_raft::raft::decode_sanitized_modality_result(
            medulla_types::ServedModalityKind::Document,
            medulla_raft::raft::SanitizedModalityMutation::Ingest,
            &single_wire(),
        )
        .unwrap();
        let ResultPayload::Raw(bytes) = payload else {
            panic!("typed replay receipt must remain a compact byte payload");
        };
        let outcome: medulla_modality::ApplyOutcome = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(outcome.observation_version, 11);
        assert_eq!(outcome.event_sequence, 17);
    }

    #[test]
    fn replay_decoder_accepts_bounded_stream_receipt() {
        let payload = medulla_raft::raft::decode_sanitized_modality_result(
            medulla_types::ServedModalityKind::Document,
            medulla_raft::raft::SanitizedModalityMutation::IngestStream,
            &stream_wire(),
        )
        .unwrap();
        let ResultPayload::Raw(bytes) = payload else {
            panic!("typed replay receipt must remain a compact byte payload");
        };
        let outcomes: Vec<medulla_modality::ApplyOutcome> = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(
            outcomes[1].disposition,
            medulla_modality::ApplyDisposition::IdempotentReplay
        );
    }

    #[test]
    fn replay_decoder_rejects_wrong_shape_and_oversized_receipts() {
        let wrong_payload = rmp_serde::to_vec_named(&ResultPayload::Bool(true)).unwrap();
        assert!(medulla_raft::raft::decode_sanitized_modality_result(
            medulla_types::ServedModalityKind::Document,
            medulla_raft::raft::SanitizedModalityMutation::Ingest,
            &wrong_payload,
        )
        .is_err());

        // A stream operation requires the typed stream result; a single outcome
        // must not be silently reinterpreted as a one-item stream.
        assert!(medulla_raft::raft::decode_sanitized_modality_result(
            medulla_types::ServedModalityKind::Document,
            medulla_raft::raft::SanitizedModalityMutation::IngestStream,
            &single_wire(),
        )
        .is_err());

        let outcome = medulla_modality::ApplyOutcome {
            disposition: medulla_modality::ApplyDisposition::Applied,
            observation_version: 1,
            event_sequence: 1,
        };
        let oversized = vec![outcome; 65];
        let oversized_wire = encode_modality_replay_wire(&oversized);
        assert!(medulla_raft::raft::decode_sanitized_modality_result(
            medulla_types::ServedModalityKind::Document,
            medulla_raft::raft::SanitizedModalityMutation::IngestStream,
            &oversized_wire,
        )
        .is_err());
    }
}

// ── Agent-memory / scene / trajectory dispatch round-trip (CONCEPT:MEDULLA-KG.memory.medulla-batch-decay-caller) ────
//
// Drive the MEDULLA-318 Methods through the SAME `dispatch` entrypoint a wire request
// hits (auth → routing → access-classify → handler → GraphCore), proving each wire
// op reaches its medulla-core primitive and returns the expected payload — the served
// surface, not the library unit. Runs on a bare `--features server` build (the
// state builder gates every optional field behind its own feature).
#[cfg(all(test, feature = "ast"))]
mod ast_input_hardening_tests {
    use super::*;

    fn limits() -> AstInputLimits {
        AstInputLimits {
            max_files: 2,
            max_source_bytes: 8,
            max_total_bytes: 12,
        }
    }

    fn pack(files: Vec<(String, serde_bytes::ByteBuf)>) -> Vec<u8> {
        rmp_serde::to_vec(&files).expect("encode AST source fixture")
    }

    #[test]
    fn accepts_canonical_bounded_relative_sources() {
        let encoded = pack(vec![(
            "src/lib.rs".to_string(),
            serde_bytes::ByteBuf::from(b"fn x(){}".to_vec()),
        )]);
        let decoded = decode_ast_files(&encoded, limits()).expect("valid source collection");
        assert_eq!(
            decoded,
            vec![("src/lib.rs".to_string(), b"fn x(){}".to_vec())]
        );
    }

    #[test]
    fn rejects_host_paths_traversal_duplicates_and_declared_bombs() {
        for name in [
            "/private/source.rs",
            "../source.rs",
            "C:\\source.rs",
            "a/./b.rs",
        ] {
            let encoded = pack(vec![(
                name.to_string(),
                serde_bytes::ByteBuf::from(vec![1]),
            )]);
            assert!(
                decode_ast_files(&encoded, limits()).is_err(),
                "accepted {name}"
            );
        }

        let duplicate = pack(vec![
            ("a.rs".to_string(), serde_bytes::ByteBuf::from(vec![1])),
            ("a.rs".to_string(), serde_bytes::ByteBuf::from(vec![2])),
        ]);
        assert!(decode_ast_files(&duplicate, limits()).is_err());
        // Content-keyed (scoped) decoding keeps both entries in order.
        let sources = decode_ast_sources(&duplicate, limits()).expect("content-keyed sources");
        assert_eq!(
            sources,
            vec![("a.rs".to_string(), vec![1]), ("a.rs".to_string(), vec![2])]
        );

        // array32 with a huge declared count and no entries: rejection happens
        // before allocation or element decoding.
        let declared_bomb = [0xdd, 0xff, 0xff, 0xff, 0xff];
        assert!(decode_ast_files(&declared_bomb, limits()).is_err());
    }

    #[test]
    fn rejects_per_source_and_aggregate_overflow() {
        let one_too_large = pack(vec![(
            "a.rs".to_string(),
            serde_bytes::ByteBuf::from(vec![0; 9]),
        )]);
        assert!(decode_ast_files(&one_too_large, limits()).is_err());

        let aggregate = pack(vec![
            ("a.rs".to_string(), serde_bytes::ByteBuf::from(vec![0; 7])),
            ("b.rs".to_string(), serde_bytes::ByteBuf::from(vec![0; 7])),
        ]);
        assert!(decode_ast_files(&aggregate, limits()).is_err());
    }
}

#[cfg(test)]
mod nested_payload_security_tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Element<'a> {
        role: &'a str,
        name: &'a str,
        x: i64,
        y: i64,
        w: i64,
        h: i64,
    }

    #[derive(Serialize)]
    struct ScreenWire<'a> {
        session_id: &'a str,
        frame_seq: u64,
        prev_frame_id: &'a str,
        prev_hash: u64,
        png: serde_bytes::ByteBuf,
        elements: Vec<Element<'a>>,
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 2, 0, 0, 0]);
        bytes
    }

    fn screen_blob(session: &str, frame_seq: u64, previous: &str, width: u32) -> Vec<u8> {
        rmp_serde::to_vec_named(&ScreenWire {
            session_id: session,
            frame_seq,
            prev_frame_id: previous,
            prev_hash: 0,
            png: serde_bytes::ByteBuf::from(png(width, 1080)),
            elements: vec![Element {
                role: "button",
                name: "Save",
                x: 1,
                y: 2,
                w: 10,
                h: 10,
            }],
        })
        .unwrap()
    }

    #[test]
    fn screen_observation_is_bounded_and_session_local() {
        let valid = screen_blob("session-1", 2, "screenobservation:session-1:1", 1920);
        assert!(decode_screen_observation(&valid).is_ok());

        let cross_session = screen_blob("session-1", 2, "screenobservation:session-2:1", 1920);
        assert!(decode_screen_observation(&cross_session).is_err());

        let oversized_dimensions = screen_blob("session-1", 0, "", 40_000);
        assert!(decode_screen_observation(&oversized_dimensions).is_err());
        assert!(decode_screen_observation(&[0xdd, 0xff, 0xff, 0xff, 0xff]).is_err());
    }

    #[test]
    fn multi_graph_batch_rejects_duplicate_graphs_and_inner_bombs() {
        let empty_ops = serde_bytes::ByteBuf::from(vec![0x90]);
        let valid = rmp_serde::to_vec_named(&vec![("graph-a", empty_ops.clone())]).unwrap();
        assert_eq!(decode_multi_graph_batches(&valid).unwrap().len(), 1);

        let duplicate = rmp_serde::to_vec_named(&vec![
            ("graph-a", empty_ops.clone()),
            ("graph-a", empty_ops),
        ])
        .unwrap();
        assert!(decode_multi_graph_batches(&duplicate).is_err());

        let inner_bomb = rmp_serde::to_vec_named(&vec![(
            "graph-a",
            serde_bytes::ByteBuf::from(vec![0xdd, 0xff, 0xff, 0xff, 0xff]),
        )])
        .unwrap();
        assert!(decode_multi_graph_batches(&inner_bomb).is_err());
    }

    #[test]
    fn request_preflight_scans_binary_fields_but_not_opaque_payloads() {
        let bomb = vec![0xdd, 0xff, 0xff, 0xff, 0xff];
        assert!(preflight_request_msgpack(&Method::AddNode {
            node_id: "node".to_string(),
            properties_msgpack: bomb,
        })
        .is_err());
        assert!(preflight_request_msgpack(&Method::Sql {
            query: "SELECT 1".to_string(),
            params_msgpack: Vec::new(),
        })
        .is_ok());
    }
}

#[cfg(all(test, feature = "redb"))]
mod eg318_dispatch_tests {
    use super::*;
    #[cfg(feature = "tsdb")]
    use medulla_types::acl::AgentIdentity;
    use medulla_types::acl::AgentRole;
    use medulla_protocol_types::protocol::Method;
    use medulla_protocol_types::protocol::Request;
    use medulla_server_api::api::persistence::PersistenceBackend;
    #[cfg(feature = "tsdb")]
    use medulla_server_auth::auth::sign_current_test_request;
    use medulla_server_auth::auth::build_shared_test_request;
    use medulla_server_auth::auth::dispatch_test_on_heap as dispatch_on_heap;
    use medulla_persistence::redb_backend::RedbBackend;
    use std::ops::Deref;
    use std::path::PathBuf;
    use std::sync::Arc;

    const SECRET: &str = "eg318-test-secret";

    struct DurableTestState {
        state: Option<Arc<RwLock<ServerState>>>,
        dir: PathBuf,
    }

    impl Deref for DurableTestState {
        type Target = Arc<RwLock<ServerState>>;

        fn deref(&self) -> &Self::Target {
            self.state.as_ref().expect("test state remains live")
        }
    }

    impl Drop for DurableTestState {
        fn drop(&mut self) {
            // Close redb before deleting its test directory.
            drop(self.state.take());
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn state_min() -> DurableTestState {
        let dir = medulla_kernel::kernel::persistence::temp_dir("eg318-dispatch", "state");
        let dir_string = dir.to_string_lossy().to_string();
        let persistence: Arc<dyn PersistenceBackend> = Arc::new(
            RedbBackend::open(dir_string.clone(), 64).expect("open authoritative test backend"),
        );
        let mut state = ServerState::new_for_test(SECRET, ServerState::test_isolation("system"));
        state.persist_dir = Some(dir_string);
        state.persistence = Some(persistence);
        let state = Arc::new(RwLock::new(state));
        DurableTestState {
            state: Some(state),
            dir,
        }
    }

    #[cfg(feature = "tsdb")]
    async fn ts_test_state(prefix: &str) -> (DurableTestState, PathBuf) {
        let state = state_min();
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}.redb",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        state.write().await.tsdb_store = Some(
            medulla_h_txn::timeseries::open_test_series_store(&path),
        );
        (state, path)
    }

    /// Give both time-series cases the same registered callers and private graphs.
    #[cfg(feature = "tsdb")]
    fn register_ts_tenants(state: &mut ServerState) {
        #[cfg(feature = "security")]
        {
            use medulla_types::acl::Grant;
            use medulla_types::acl::GrantEffect;
            use medulla_types::acl::RbacAction;
            use medulla_types::acl::ResourceSelector;
            use medulla_types::acl::Role;
            state.isolation.add_role(Role::new("owner-acme-private"));
            state.isolation.add_role(Role::new("owner-other-private"));
            let grant = |role: &str, graph: &str, action: RbacAction| Grant {
                role: role.to_string(),
                resource: ResourceSelector::Graph(graph.to_string()),
                action,
                effect: GrantEffect::Allow,
            };
            for action in [RbacAction::Read, RbacAction::Write] {
                state
                    .isolation
                    .add_grant(grant("owner-acme-private", "acme:private", action));
                state
                    .isolation
                    .add_grant(grant("owner-other-private", "other:private", action));
            }
        }
        for (agent_id, role) in [
            ("alice", "owner-acme-private"),
            ("bob", "owner-other-private"),
        ] {
            #[cfg(not(feature = "security"))]
            let _ = role;
            state.isolation.register_agent(AgentIdentity {
                agent_id: agent_id.into(),
                role: AgentRole::Agent,
                teams: vec![],
                #[cfg(feature = "security")]
                roles: vec![role.into()],
                #[cfg(not(feature = "security"))]
                roles: vec![],
            });
        }
        for (graph, agent_id) in [("acme:private", "alice"), ("other:private", "bob")] {
            let _ = state.registry.create_graph(
                graph,
                medulla_protocol_types::protocol::GraphType::Agent,
                Some(agent_id.into()),
            );
        }
    }

    #[cfg(feature = "tsdb")]
    fn ts_tenant_request(id: u64, graph: &str, agent: &str, method: Method) -> Request {
        sign_current_test_request(
            SECRET,
            Request {
                id,
                graph: graph.into(),
                auth_token: String::new(),
                agent_id: Some(agent.into()),
                method,
            },
        )
    }

    fn req(id: u64, method: Method) -> Request {
        build_shared_test_request(SECRET, id, "__commons__", "system", method)
    }

    fn blob(v: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&v).unwrap()
    }

    /// CONCEPT:MEDULLA-KG.memory.medulla-batch-decay-caller/MEDULLA-220 — CreateSummaryNode over the wire → SummaryChildren
    /// reads back the linked children.
    #[tokio::test(flavor = "multi_thread")]
    async fn eg318_create_summary_then_read_children() {
        // Reads the ambient encryption env at its durable open, so the env must hold
        // still for this whole body. READ guard: it excludes only a key MUTATOR, never
        // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let state = state_min();
        for (i, id) in ["e1", "e2"].iter().enumerate() {
            let r = dispatch_on_heap(
                &state,
                req(
                    100 + i as u64,
                    Method::AddNode {
                        node_id: (*id).into(),
                        properties_msgpack: blob(serde_json::json!({"type": "Episodic"})),
                    },
                ),
            )
            .await;
            assert!(r.error.is_none(), "AddNode: {:?}", r.error);
        }
        let created = dispatch_on_heap(
            &state,
            req(
                1,
                Method::CreateSummaryNode {
                    level: 1,
                    child_ids: vec!["e1".into(), "e2".into()],
                    props_msgpack: blob(serde_json::json!({})),
                },
            ),
        )
        .await;
        let sid = match created.result {
            Some(ResultPayload::String(s)) => s,
            other => panic!("CreateSummaryNode: {:?} / {:?}", other, created.error),
        };
        let children = dispatch_on_heap(
            &state,
            req(
                2,
                Method::SummaryChildren {
                    node_id: sid.clone(),
                },
            ),
        )
        .await;
        match children.result {
            Some(ResultPayload::Ids(ids)) => assert_eq!(ids, vec!["e1", "e2"]),
            other => panic!("SummaryChildren: {:?} / {:?}", other, children.error),
        }
    }

    /// CONCEPT:MEDULLA-KG.memory.medulla-batch-decay-caller/MEDULLA-221 — Consolidate over the wire returns the deterministic
    /// semantic node id.
    #[tokio::test(flavor = "multi_thread")]
    async fn eg318_consolidate_returns_semantic_id() {
        // Reads the ambient encryption env at its durable open, so the env must hold
        // still for this whole body. READ guard: it excludes only a key MUTATOR, never
        // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let state = state_min();
        for (i, id) in ["a", "b"].iter().enumerate() {
            let _ = dispatch_on_heap(
                &state,
                req(
                    200 + i as u64,
                    Method::AddNode {
                        node_id: (*id).into(),
                        properties_msgpack: blob(serde_json::json!({"type": "Episodic"})),
                    },
                ),
            )
            .await;
        }
        let r = dispatch_on_heap(
            &state,
            req(
                3,
                Method::Consolidate {
                    episodic_ids: vec!["a".into(), "b".into()],
                    semantic_props_msgpack: blob(serde_json::json!({"summary": "s"})),
                },
            ),
        )
        .await;
        match r.result {
            Some(ResultPayload::String(s)) => assert!(s.starts_with("semantic:")),
            other => panic!("Consolidate: {:?} / {:?}", other, r.error),
        }
    }

    /// CONCEPT:MEDULLA-KG.memory.medulla-batch-decay-caller/MEDULLA-222 — Maintain (decay + evict) over the wire returns the
    /// `(decayed, pruned_ids)` tuple.
    #[tokio::test(flavor = "multi_thread")]
    async fn eg318_maintain_decays_and_evicts() {
        // Reads the ambient encryption env at its durable open, so the env must hold
        // still for this whole body. READ guard: it excludes only a key MUTATOR, never
        // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let state = state_min();
        // A low-importance node in the working set gets evicted below threshold.
        let _ = dispatch_on_heap(
            &state,
            req(
                300,
                Method::AddNode {
                    node_id: "low".into(),
                    properties_msgpack: blob(serde_json::json!({"importance": 0.1})),
                },
            ),
        )
        .await;
        let r = dispatch_on_heap(
            &state,
            req(
                4,
                Method::Maintain {
                    ids: vec!["low".into()],
                    now_ms: 1_000,
                    half_life_ms: 604_800_000,
                    evict_threshold: 0.5,
                    delete: false,
                },
            ),
        )
        .await;
        let raw = match r.result {
            Some(ResultPayload::Raw(b)) => b,
            other => panic!("Maintain: {:?} / {:?}", other, r.error),
        };
        let (_decayed, pruned): (usize, Vec<String>) = rmp_serde::from_slice(&raw).unwrap();
        assert_eq!(pruned, vec!["low"]);
    }

    /// CONCEPT:MEDULLA-KG.memory.medulla-batch-decay-caller/MEDULLA-087 — AddSceneObject over the wire → WorldTransform reads
    /// back the composed world pose.
    #[tokio::test(flavor = "multi_thread")]
    async fn eg318_scene_object_then_world_transform() {
        // Reads the ambient encryption env at its durable open, so the env must hold
        // still for this whole body. READ guard: it excludes only a key MUTATOR, never
        // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let state = state_min();
        let pose = serde_json::json!({"translation": {"x": 5.0, "y": 0.0, "z": 0.0}});
        let created = dispatch_on_heap(
            &state,
            req(
                5,
                Method::AddSceneObject {
                    pose_msgpack: blob(pose),
                    parent: None,
                },
            ),
        )
        .await;
        let oid = match created.result {
            Some(ResultPayload::String(s)) => s,
            other => panic!("AddSceneObject: {:?} / {:?}", other, created.error),
        };
        let wt = dispatch_on_heap(&state, req(6, Method::WorldTransform { node_id: oid })).await;
        match wt.result {
            Some(ResultPayload::Json(v)) => {
                let tx = v["translation"]["x"].as_f64().unwrap();
                assert!((tx - 5.0).abs() < 1e-9, "world x = {tx}");
            }
            other => panic!("WorldTransform: {:?} / {:?}", other, wt.error),
        }
    }

    /// CONCEPT:MEDULLA-KG.memory.medulla-batch-decay-caller/MEDULLA-099 — StartTrajectory + AppendStep over the wire →
    /// DiscountedReturn computes `Σ gamma^t · reward`.
    #[tokio::test(flavor = "multi_thread")]
    async fn eg318_trajectory_append_then_discounted_return() {
        // Reads the ambient encryption env at its durable open, so the env must hold
        // still for this whole body. READ guard: it excludes only a key MUTATOR, never
        // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let state = state_min();
        let started = dispatch_on_heap(
            &state,
            req(
                7,
                Method::StartTrajectory {
                    props_msgpack: blob(serde_json::json!({})),
                },
            ),
        )
        .await;
        let tid = match started.result {
            Some(ResultPayload::String(s)) => s,
            other => panic!("StartTrajectory: {:?} / {:?}", other, started.error),
        };
        for (i, reward) in [2.0f64, 4.0].into_iter().enumerate() {
            let r = dispatch_on_heap(
                &state,
                req(
                    8 + i as u64,
                    Method::AppendStep {
                        traj_id: tid.clone(),
                        action_msgpack: blob(serde_json::json!("go")),
                        reward,
                        state_ref: None,
                        next_state_ref: None,
                        t: i as u64,
                    },
                ),
            )
            .await;
            // Raw(Option<String>) — Some(step id) since the trajectory exists.
            match r.result {
                Some(ResultPayload::Raw(b)) => {
                    let step: Option<String> = rmp_serde::from_slice(&b).unwrap();
                    assert!(step.is_some(), "AppendStep should return a step id");
                }
                other => panic!("AppendStep: {:?} / {:?}", other, r.error),
            }
        }
        let dr = dispatch_on_heap(
            &state,
            req(
                20,
                Method::DiscountedReturn {
                    traj_id: tid,
                    gamma: 0.5,
                },
            ),
        )
        .await;
        match dr.result {
            // 2.0 + 0.5^1 * 4.0 = 4.0
            Some(ResultPayload::Float(f)) => assert!((f - 4.0).abs() < 1e-9, "return = {f}"),
            other => panic!("DiscountedReturn: {:?} / {:?}", other, dr.error),
        }
    }

    /// Public Ts* calls must share the ordinary graph ACL boundary, and identical
    /// local series ids in two tenants must never collide in series.redb.
    #[cfg(feature = "tsdb")]
    #[tokio::test(flavor = "multi_thread")]
    async fn timeseries_is_graph_authorized_and_tenant_scoped() {
        // Reads the ambient encryption env at its durable open, so the env must hold
        // still for this whole body. READ guard: it excludes only a key MUTATOR, never
        // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let (state, path) = ts_test_state("medulla-ts-policy").await;
        {
            let mut s = state.write().await;
            // RBAC (`feature = "security"`) is the mandatory current access decision
            // for a non-System identity — `check_access` ignores `graph_owner`
            // entirely under this feature and evaluates ONLY `identity.roles`
            // against the RBAC policy (no pre-RBAC ACL fall-through, no "owner
            // always wins" shortcut). So each agent needs an explicit grant on
            // their own private graph, or every Ts* call below default-denies.
            register_ts_tenants(&mut s);
        }
        let request = ts_tenant_request;
        let append = |value: f64| Method::TsAppend {
            series_id: "cpu".into(),
            n_fields: 1,
            bucket_ns: 1_000,
            field_names: vec!["value".into()],
            points_msgpack: rmp_serde::to_vec(&vec![(1i64, vec![value])]).unwrap(),
        };
        assert!(
            dispatch_on_heap(&state, request(1, "acme:private", "alice", append(10.0)))
                .await
                .error
                .is_none()
        );
        assert!(
            dispatch_on_heap(&state, request(2, "other:private", "bob", append(20.0)))
                .await
                .error
                .is_none()
        );

        let denied = dispatch_on_heap(
            &state,
            request(
                3,
                "other:private",
                "alice",
                Method::TsRange {
                    series_id: "cpu".into(),
                    from: 0,
                    to: 10,
                },
            ),
        )
        .await;
        assert!(
            denied.error.is_some(),
            "cross-tenant series read must be denied"
        );

        let own = dispatch_on_heap(
            &state,
            request(
                4,
                "acme:private",
                "alice",
                Method::TsRange {
                    series_id: "cpu".into(),
                    from: 0,
                    to: 10,
                },
            ),
        )
        .await;
        let points: Vec<(i64, Vec<f64>)> = match own.result {
            Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).unwrap(),
            other => panic!("expected scoped TsRange result, got {other:?}"),
        };
        assert_eq!(points, vec![(1, vec![10.0])]);
        drop(state);
        let _ = std::fs::remove_file(path);
    }

    /// End-to-end reachability proof for `TsListSeries`/`TsEvict`/`TsDeleteSeries`
    /// (CONCEPT:MEDULLA-KG.storage.series-retention-reachability): before this test's
    /// production code existed, `SeriesStore::evict_before`/`delete_series`/
    /// `list_series` were reachable ONLY from `medulla-tsdb`'s own crate-internal unit
    /// tests — no `Method` variant, no RPC route, no caller anywhere in `src/`. This
    /// drives all three through the SAME `dispatch()` entrypoint a wire request
    /// hits, against a REAL `SeriesStore`/redb file, proving: (1) `TsListSeries`
    /// enumerates a just-appended series scoped to the caller's own tenant/graph
    /// (never cross-tenant, mirroring `timeseries_is_graph_authorized_and_tenant_scoped`
    /// above); (2) `TsEvict` actually removes only the points before its cutoff,
    /// verified by reading the survivors back with `TsRange`; (3) `TsDeleteSeries`
    /// removes the series entirely -- it drops off `TsListSeries` and `TsRange`
    /// against it comes back empty, not an error (an unknown series is legal, per
    /// `SeriesStore::range_scoped`'s existing "empty for an unknown series"
    /// contract).
    #[cfg(feature = "tsdb")]
    #[tokio::test(flavor = "multi_thread")]
    async fn timeseries_retention_evict_delete_and_list_are_scoped_and_reachable() {
        // Reads the ambient encryption env at its durable open, so the env must hold
        // still for this whole body. READ guard: it excludes only a key MUTATOR, never
        // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let (state, path) = ts_test_state("medulla-ts-retention").await;
        {
            let mut s = state.write().await;
            register_ts_tenants(&mut s);
        }
        let request = ts_tenant_request;
        // Two points a full bucket apart (bucket_ns = 1_000) so evicting one leaves
        // the other in a surviving bucket rather than trimming inside a shared one.
        let append = Method::TsAppend {
            series_id: "cpu".into(),
            n_fields: 1,
            bucket_ns: 1_000,
            field_names: vec!["value".into()],
            points_msgpack: rmp_serde::to_vec(&vec![(1i64, vec![10.0]), (2_000i64, vec![20.0])])
                .unwrap(),
        };
        assert!(
            dispatch_on_heap(&state, request(1, "acme:private", "alice", append))
                .await
                .error
                .is_none()
        );

        // (1) TsListSeries: the just-appended series is visible to its own tenant...
        let listed = dispatch_on_heap(
            &state,
            request(2, "acme:private", "alice", Method::TsListSeries),
        )
        .await;
        let series_ids: Vec<String> = match listed.result {
            Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).unwrap(),
            other => panic!("expected TsListSeries result, got {other:?}"),
        };
        assert_eq!(series_ids, vec!["cpu".to_string()]);
        // ...and invisible (denied, not merely empty) to a caller with no access to
        // that graph at all -- the SAME cross-tenant graph ACL boundary
        // `timeseries_is_graph_authorized_and_tenant_scoped` proves for TsRange.
        let cross_tenant = dispatch_on_heap(
            &state,
            request(3, "other:private", "alice", Method::TsListSeries),
        )
        .await;
        assert!(
            cross_tenant.error.is_some(),
            "cross-tenant TsListSeries must be denied"
        );

        // (2) TsEvict: cutoff = 1_000 drops the bucket containing ts=1 (< 1_000) and
        // keeps the bucket containing ts=2_000 (>= 1_000).
        let evicted = dispatch_on_heap(
            &state,
            request(
                4,
                "acme:private",
                "alice",
                Method::TsEvict {
                    series_id: "cpu".into(),
                    cutoff: 1_000,
                },
            ),
        )
        .await;
        match evicted.result {
            Some(ResultPayload::Count(dropped)) => {
                assert_eq!(dropped, 1, "exactly one whole bucket must be evicted")
            }
            other => panic!(
                "expected TsEvict Count result, got {other:?} / {:?}",
                evicted.error
            ),
        }
        let after_evict = dispatch_on_heap(
            &state,
            request(
                5,
                "acme:private",
                "alice",
                Method::TsRange {
                    series_id: "cpu".into(),
                    from: 0,
                    to: 10_000,
                },
            ),
        )
        .await;
        let survivors: Vec<(i64, Vec<f64>)> = match after_evict.result {
            Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).unwrap(),
            other => panic!("expected scoped TsRange result, got {other:?}"),
        };
        assert_eq!(
            survivors,
            vec![(2_000, vec![20.0])],
            "the point at ts=1 must be gone; the point at ts=2_000 must survive"
        );

        // (3) TsDeleteSeries: removes the series entirely.
        let deleted = dispatch_on_heap(
            &state,
            request(
                6,
                "acme:private",
                "alice",
                Method::TsDeleteSeries {
                    series_id: "cpu".into(),
                },
            ),
        )
        .await;
        match deleted.result {
            Some(ResultPayload::Count(dropped)) => {
                assert_eq!(dropped, 1, "the one surviving bucket must be removed")
            }
            other => panic!(
                "expected TsDeleteSeries Count result, got {other:?} / {:?}",
                deleted.error
            ),
        }
        let listed_after_delete = dispatch_on_heap(
            &state,
            request(7, "acme:private", "alice", Method::TsListSeries),
        )
        .await;
        let series_ids_after_delete: Vec<String> = match listed_after_delete.result {
            Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).unwrap(),
            other => panic!("expected TsListSeries result, got {other:?}"),
        };
        assert!(
            series_ids_after_delete.is_empty(),
            "the deleted series must no longer be listed"
        );
        let range_after_delete = dispatch_on_heap(
            &state,
            request(
                8,
                "acme:private",
                "alice",
                Method::TsRange {
                    series_id: "cpu".into(),
                    from: 0,
                    to: 10_000,
                },
            ),
        )
        .await;
        assert!(
            range_after_delete.error.is_none(),
            "TsRange against a deleted series is legal (empty), not an error"
        );
        let empty: Vec<(i64, Vec<f64>)> = match range_after_delete.result {
            Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).unwrap(),
            other => panic!("expected scoped TsRange result, got {other:?}"),
        };
        assert!(empty.is_empty());

        drop(state);
        let _ = std::fs::remove_file(path);
    }

    /// The ACL fix this task made: `TsEvict`/`TsDeleteSeries` are `series.redb`
    /// MUTATIONS and must require the same Write access level as `TsAppend` -- a
    /// caller granted only Read on a graph must be able to enumerate/read its
    /// series (`TsListSeries`/`TsRange`) but must NOT be able to evict or delete
    /// one. Before the fix to the `access` computation in this file (the
    /// `matches!` alongside `requires_write`), `TsEvict`/`TsDeleteSeries` fell to
    /// the `else` branch and were silently classified `AccessLevel::Read`, so a
    /// Read-only caller could destroy retained data.
    #[cfg(all(feature = "tsdb", feature = "security"))]
    #[tokio::test(flavor = "multi_thread")]
    async fn timeseries_retention_mutations_require_write_not_read() {
        // Reads the ambient encryption env at its durable open, so the env must hold
        // still for this whole body. READ guard: it excludes only a key MUTATOR, never
        // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        use medulla_types::acl::Grant;
        use medulla_types::acl::GrantEffect;
        use medulla_types::acl::RbacAction;
        use medulla_types::acl::ResourceSelector;
        use medulla_types::acl::Role;

        let (state, path) = ts_test_state("medulla-ts-retention-acl").await;
        {
            let mut s = state.write().await;
            s.isolation.add_role(Role::new("reader-acme-private"));
            s.isolation.add_grant(Grant {
                role: "reader-acme-private".to_string(),
                resource: ResourceSelector::Graph("acme:private".to_string()),
                action: RbacAction::Read,
                effect: GrantEffect::Allow,
            });
            s.isolation.register_agent(AgentIdentity {
                agent_id: "reader".into(),
                role: AgentRole::Agent,
                teams: vec![],
                roles: vec!["reader-acme-private".into()],
            });
            let _ = s.registry.create_graph(
                "acme:private",
                medulla_protocol_types::protocol::GraphType::Agent,
                Some("owner".into()),
            );
        }
        let request = |id: u64, method: Method| {
            sign_current_test_request(
                SECRET,
                Request {
                    id,
                    graph: "acme:private".into(),
                    auth_token: String::new(),
                    agent_id: Some("reader".into()),
                    method,
                },
            )
        };

        let list = dispatch_on_heap(&state, request(1, Method::TsListSeries)).await;
        assert!(
            list.error.is_none(),
            "a Read-granted caller must be able to list series: {:?}",
            list.error
        );
        let range = dispatch_on_heap(
            &state,
            request(
                2,
                Method::TsRange {
                    series_id: "cpu".into(),
                    from: 0,
                    to: 10,
                },
            ),
        )
        .await;
        assert!(
            range.error.is_none(),
            "a Read-granted caller must be able to range-read series: {:?}",
            range.error
        );

        let evict = dispatch_on_heap(
            &state,
            request(
                3,
                Method::TsEvict {
                    series_id: "cpu".into(),
                    cutoff: i64::MAX,
                },
            ),
        )
        .await;
        assert!(
            evict.error.is_some(),
            "a Read-only caller must NOT be able to evict series data"
        );
        let delete = dispatch_on_heap(
            &state,
            request(
                4,
                Method::TsDeleteSeries {
                    series_id: "cpu".into(),
                },
            ),
        )
        .await;
        assert!(
            delete.error.is_some(),
            "a Read-only caller must NOT be able to delete a series"
        );

        drop(state);
        let _ = std::fs::remove_file(path);
    }
}
