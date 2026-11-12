//! Runtime ontology import (MEDULLA-TYPED-PACKS-R103.2 to R103.5).

use super::*;

use medulla_types::contract::ResourceId;

const EX_A: &str = "@prefix owl: <http://www.w3.org/2002/07/owl#> . <http://example.org/onto#Alpha> a owl:Class .\n";
const EX_B: &str = "@prefix owl: <http://www.w3.org/2002/07/owl#> . <http://example.org/onto#Beta> a owl:Class .\n";
const IRI: &str = "http://example.org/onto";

fn authority(importer: &str, at_ms: u64) -> OntologyAuthority {
    OntologyAuthority {
        importer: importer.to_string(),
        tenant: "tenant-a".to_string(),
        at_ms,
    }
}

fn import_op(body: &str, expected_prior_revision: Option<u64>) -> GraphSchemaOp {
    GraphSchemaOp::ImportOntology {
        connector: ResourceId::new("weather").unwrap(),
        ontology_iri: IRI.to_string(),
        version: "1".to_string(),
        ontology_ttl: body.to_string(),
        shapes_ttl: None,
        content_digest: Digest256::sha256(body.as_bytes()),
        expected_prior_revision,
    }
}

fn withdraw_op(expected_revision: u64) -> GraphSchemaOp {
    GraphSchemaOp::WithdrawOntology {
        connector: ResourceId::new("weather").unwrap(),
        ontology_iri: IRI.to_string(),
        expected_revision,
    }
}

fn key() -> String {
    ontology_source_key(&ResourceId::new("weather").unwrap(), IRI)
}

fn record(core: &GraphCore) -> (u64, String, Vec<OntologyRevisionEntry>) {
    prior_record(core, &key()).expect("an ontology record")
}

fn composed_classes(core: &GraphCore) -> Vec<String> {
    let composed =
        medulla_server_api::schema_compose::validate_and_compose(&core.schema_sources()).unwrap();
    composed
        .ontology
        .iter()
        .map(|triple| triple.subject.to_string())
        .filter(|subject| subject.contains("example.org/onto#"))
        .collect()
}

// spec: MEDULLA-TYPED-PACKS-R103.2
#[test]
fn import_is_idempotent_by_digest_and_revises() {
    let core = GraphCore::new();
    let first =
        import_ontology(&core, "g", &import_op(EX_A, None), &authority("alice", 10)).unwrap();
    assert!(first.changed);
    let repeat =
        import_ontology(&core, "g", &import_op(EX_A, None), &authority("alice", 11)).unwrap();
    assert!(!repeat.changed);
    assert_eq!(repeat.composed_digest, first.composed_digest);
    assert_eq!(record(&core).0, 1);

    let second = import_ontology(
        &core,
        "g",
        &import_op(EX_B, Some(1)),
        &authority("alice", 20),
    )
    .unwrap();
    assert!(second.changed);
    let (revision, importer, history) = record(&core);
    assert_eq!(revision, 2);
    assert_eq!(importer, "alice");
    assert_eq!(
        history,
        vec![
            OntologyRevisionEntry {
                digest: Digest256::sha256(EX_A.as_bytes()),
                revision: 1,
                importer: "alice".to_string(),
                at: 10,
            },
            OntologyRevisionEntry {
                digest: Digest256::sha256(EX_B.as_bytes()),
                revision: 2,
                importer: "alice".to_string(),
                at: 20,
            },
        ]
    );
    // Metadata only: the stored body is the active revision's, nothing older.
    let source = core.schema_sources().dynamic.get(&key()).cloned().unwrap();
    assert_eq!(source.ontology_ttl.as_deref(), Some(EX_B));
    let classes = composed_classes(&core);
    assert!(classes.iter().any(|class| class.contains("Beta")));
    assert!(!classes.iter().any(|class| class.contains("Alpha")));
}

// spec: MEDULLA-TYPED-PACKS-R103.2
#[test]
fn history_keeps_the_newest_bounded_entries() {
    let core = GraphCore::new();
    for index in 0..(MAX_ONTOLOGY_REVISION_HISTORY as u64 + 1) {
        let body = format!(
            "<http://example.org/onto#C{index}> a <http://www.w3.org/2002/07/owl#Class> .\n"
        );
        import_ontology(
            &core,
            "g",
            &import_op(&body, None),
            &authority("alice", index),
        )
        .unwrap();
    }
    let (revision, _, history) = record(&core);
    assert_eq!(revision, MAX_ONTOLOGY_REVISION_HISTORY as u64 + 1);
    assert_eq!(history.len(), MAX_ONTOLOGY_REVISION_HISTORY);
    assert_eq!(history.first().unwrap().revision, 2);
}

/// Verifies MEDULLA-TYPED-PACKS-R103.4: Runtime ontology import refusals
// spec: MEDULLA-TYPED-PACKS-R103.4
#[test]
fn ontology_import_refusals_leave_schema_unchanged() {
    let core = GraphCore::new();
    import_ontology(&core, "g", &import_op(EX_A, None), &authority("alice", 1)).unwrap();
    let before = core.schema_sources();

    let malformed = import_op("this is not turtle", None);
    let error = import_ontology(&core, "g", &malformed, &authority("alice", 2)).unwrap_err();
    assert!(error.starts_with("ONTOLOGY_INVALID"), "{error}");

    let mut wrong_digest = import_op(EX_B, None);
    if let GraphSchemaOp::ImportOntology { content_digest, .. } = &mut wrong_digest {
        *content_digest = Digest256::sha256(b"other");
    }
    let error = import_ontology(&core, "g", &wrong_digest, &authority("alice", 3)).unwrap_err();
    assert!(error.starts_with("ONTOLOGY_DIGEST_MISMATCH"), "{error}");

    let error = import_ontology(
        &core,
        "g",
        &import_op(EX_B, Some(7)),
        &authority("alice", 4),
    )
    .unwrap_err();
    assert!(error.starts_with("ONTOLOGY_REVISION_CONFLICT"), "{error}");

    let error =
        import_ontology(&core, "g", &import_op(EX_B, None), &authority("mallory", 5)).unwrap_err();
    assert!(error.starts_with("IMPORTER_MISMATCH"), "{error}");
    let error =
        withdraw_ontology(&core, "g", &withdraw_op(1), &authority("mallory", 5)).unwrap_err();
    assert!(error.starts_with("IMPORTER_MISMATCH"), "{error}");

    let error = withdraw_ontology(&core, "g", &withdraw_op(9), &authority("alice", 6)).unwrap_err();
    assert!(error.starts_with("ONTOLOGY_REVISION_CONFLICT"), "{error}");

    let mut foreign = authority("alice", 7);
    foreign.tenant = String::new();
    let error = import_ontology(&core, "g", &import_op(EX_B, None), &foreign).unwrap_err();
    assert!(error.starts_with("TENANT_DENIED"), "{error}");

    let core_redeclaration = "@prefix owl: <http://www.w3.org/2002/07/owl#> . \
         <https://knuckles-team.github.io/ontology/core#Agent> a owl:ObjectProperty .\n";
    let error = import_ontology(
        &core,
        "g",
        &import_op(core_redeclaration, None),
        &authority("alice", 8),
    )
    .unwrap_err();
    assert!(
        error.starts_with("ONTOLOGY_CORE_CONFLICT") || error.starts_with("OWL_"),
        "{error}"
    );

    assert_eq!(core.schema_sources(), before);
}

// spec: MEDULLA-TYPED-PACKS-R103.2
#[test]
fn withdrawal_detaches_the_source_and_its_record() {
    let core = GraphCore::new();
    import_ontology(&core, "g", &import_op(EX_A, None), &authority("alice", 1)).unwrap();
    let withdrawn = withdraw_ontology(&core, "g", &withdraw_op(1), &authority("alice", 2)).unwrap();
    assert!(withdrawn.changed);
    assert!(prior_record(&core, &key()).is_none());
    assert!(composed_classes(&core).is_empty());
}

mod served {
    use std::sync::Arc;

    use tokio::sync::RwLock;

    use medulla_catalog_types::graph_schema::{
        GraphSchemaClassesView, GraphSchemaOp, GraphSchemaSourcesView, SchemaSourceOriginView,
    };
    use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
    use medulla_server_api::state::ServerState;
    use medulla_server_auth::auth::{build_current_test_request, dispatch_test_on_heap};

    use super::{import_op, withdraw_op, EX_A, EX_B};

    const SECRET: &str = "runtime-ontology-import-served-secret";
    const AGENT: &str = "ontology-importer";
    /// The fixed `cfg(test)` deployment tenant the request-context policy admits.
    const TENANT: &str = "tenant-shared";
    const GRAPH: &str = "onto-g";

    /// A served engine with a durable graph backend: the import commits through
    /// the authoritative MutationBatch path, which needs one.
    async fn state() -> (tempfile::TempDir, Arc<RwLock<ServerState>>) {
        let dir = tempfile::tempdir().unwrap();
        let mut state = medulla_server::seams_install::new_state_for_test(
            SECRET,
            ServerState::test_isolation(AGENT),
        );
        state.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        let backend = medulla_persistence::redb_backend::RedbBackend::open(
            dir.path().join("graphs").to_string_lossy().into_owned(),
            64,
        )
        .expect("open the served graph backend");
        state.persistence = Some(Arc::new(backend));
        let state = Arc::new(RwLock::new(state));
        let request = build_current_test_request(
            SECRET,
            TENANT,
            0,
            "__commons__",
            AGENT,
            Method::CreateGraph {
                graph_name: GRAPH.to_string(),
                graph_type: GraphType::Global,
            },
        );
        let created = dispatch_test_on_heap(&state, request).await;
        assert!(created.error.is_none(), "{:?}", created.error_detail);
        (dir, state)
    }

    async fn call(state: &Arc<RwLock<ServerState>>, req_id: u64, method: Method) -> Response {
        let request = build_current_test_request(SECRET, TENANT, req_id, GRAPH, AGENT, method);
        dispatch_test_on_heap(state, request).await
    }

    fn raw<T: serde::de::DeserializeOwned>(response: &Response) -> T {
        assert!(response.error.is_none(), "refused: {:?}", response.error);
        match &response.result {
            Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).expect("decodes"),
            other => panic!("expected a Raw result, got {other:?}"),
        }
    }

    fn schema(op: GraphSchemaOp) -> Method {
        Method::GraphSchema { op: Box::new(op) }
    }

    async fn ontology_origin(state: &Arc<RwLock<ServerState>>) -> Option<SchemaSourceOriginView> {
        let view: GraphSchemaSourcesView = raw(&call(state, 90, Method::GraphSchemaList).await);
        view.dynamic_sources
            .as_slice()
            .iter()
            .find(|source| source.source_id.starts_with("ontology:"))
            .map(|source| source.origin.clone())
    }

    async fn class_revisions(state: &Arc<RwLock<ServerState>>) -> Vec<(String, Option<u64>)> {
        let method = Method::GraphSchemaClasses {
            kind: None,
            cursor: None,
            limit: 1000,
        };
        let page: GraphSchemaClassesView = raw(&call(state, 91, method).await);
        page.terms
            .as_slice()
            .iter()
            .filter(|term| term.iri.starts_with("http://example.org/onto#"))
            .map(|term| (term.iri.clone(), term.ontology_revision))
            .collect()
    }

    // spec: MEDULLA-TYPED-PACKS-R103.3
    #[tokio::test]
    async fn gateway_routes_ontology_import() {
        let (_dir, state) = state().await;
        let committed = call(&state, 1, schema(import_op(EX_A, None))).await;
        assert!(committed.error.is_none(), "{:?}", committed.error_detail);
        match ontology_origin(&state).await {
            Some(SchemaSourceOriginView::Ontology {
                revision, importer, ..
            }) => {
                assert_eq!(revision, 1);
                assert_eq!(importer, AGENT, "the importer is the request principal");
            }
            other => panic!("expected an ontology origin, got {other:?}"),
        }
        let withdrawn = call(&state, 2, schema(withdraw_op(1))).await;
        assert!(withdrawn.error.is_none(), "{:?}", withdrawn.error_detail);
        assert!(ontology_origin(&state).await.is_none());
    }

    // spec: MEDULLA-TYPED-PACKS-R103.5
    #[tokio::test]
    async fn revision_records_digest_importer_and_classes() {
        let (_dir, state) = state().await;
        assert!(call(&state, 1, schema(import_op(EX_A, None)))
            .await
            .error
            .is_none());
        assert_eq!(
            class_revisions(&state).await,
            vec![("http://example.org/onto#Alpha".to_string(), Some(1))]
        );
        assert!(call(&state, 2, schema(import_op(EX_B, Some(1))))
            .await
            .error
            .is_none());
        assert_eq!(
            class_revisions(&state).await,
            vec![("http://example.org/onto#Beta".to_string(), Some(2))],
            "a class the later revision dropped is no longer listed"
        );
        let Some(SchemaSourceOriginView::Ontology {
            connector,
            ontology_iri,
            revision,
            history,
            ..
        }) = ontology_origin(&state).await
        else {
            panic!("expected an ontology origin");
        };
        assert_eq!(connector, "weather");
        assert_eq!(ontology_iri, super::IRI);
        assert_eq!(revision, 2);
        let digests: Vec<_> = history.iter().map(|entry| entry.digest).collect();
        assert_eq!(
            digests,
            vec![
                medulla_types::contract::Digest256::sha256(EX_A.as_bytes()),
                medulla_types::contract::Digest256::sha256(EX_B.as_bytes()),
            ]
        );
        assert!(history
            .iter()
            .all(|entry| entry.importer == AGENT && entry.at > 0));
        assert_eq!(
            history
                .iter()
                .map(|entry| entry.revision)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }
}
