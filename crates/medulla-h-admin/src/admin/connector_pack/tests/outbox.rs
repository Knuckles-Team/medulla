//! PB1 + X10: the projection worker is the first Agent Library outbox
//! consumer, a pack is dark until its projection applies, and the operator
//! surface reads and rewinds the worker's stream.

#[cfg(feature = "redb")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use std::sync::atomic::AtomicBool;

#[cfg(feature = "redb")]
use medulla_server_blob::blob::BlobStateExt;

use medulla_agent_types::{
    agent_component::AgentComponentKind, agent_component::AgentComponentOp,
    agent_component::AgentComponentSearchPage, agent_component::AgentComponentSearchRequest,
};
use medulla_catalog_types::{
    connector_pack::PackAnnotations, connector_pack::PackEntryKind,
    connector_pack::CONNECTOR_PACK_IMPORT_TOPIC,
};
use medulla_mutation_types::{
    mutation_outbox::MutationOutboxOp, mutation_outbox::MutationOutboxStatusView,
    mutation_outbox::NativeOutboxScope, mutation_outbox::NativeOutboxStore,
    mutation_outbox::OutboxDeadLetterPage, mutation_outbox::OutboxRewindReceipt,
    mutation_outbox::OutboxTarget, mutation_outbox::RewindTarget,
};
use medulla_types::contract::ResourceId;

use super::{bind, build_pack, imported, ok, tool, Content, Served, ADMIN, TENANT};
use crate::connector_pack_projection::worker::sweep;
use crate::connector_pack_projection::worker::CONSUMER;
use medulla_protocol_types::protocol::Method;

const CONNECTOR: &str = "pack-outbox";

async fn outbox(
    served: &Served,
    key: &str,
    op: MutationOutboxOp,
) -> medulla_protocol_types::protocol::Response {
    served
        .call(key, Method::MutationOutbox { op: Box::new(op) })
        .await
}

fn target() -> OutboxTarget {
    OutboxTarget::NativeStore {
        store: NativeOutboxStore::AgentLibrary,
        tenant_id: TENANT.to_string(),
        scope: NativeOutboxScope::Tenant,
    }
}

async fn searchable_ids(served: &Served) -> Vec<String> {
    let page: AgentComponentSearchPage = ok(
        "Search",
        served
            .call(
                &format!(
                    "search:{}",
                    served.state.read().await.persist_dir.clone().unwrap()
                ),
                Method::AgentComponent {
                    op: AgentComponentOp::Search {
                        request: AgentComponentSearchRequest {
                            tenant_id: TENANT.to_string(),
                            task: None,
                            capabilities: Vec::new(),
                            kinds: vec![AgentComponentKind::Tool],
                            read_only: false,
                            limit: None,
                            cursor: None,
                        },
                    },
                },
            )
            .await,
    );
    page.entries
        .into_iter()
        .map(|entry| entry.component_id)
        .collect()
}

// spec: MEDULLA-TYPED-PACKS-R052
// The projection consumer is not a pack-specific stand-in: it is driven
// through the general Agent Library MutationOutbox (status/rewind) that
// MEDULLA-TYPED-PACKS-R054/R055 already land on main, so this capability was
// shipped only once that general outbox authority existed.
#[tokio::test]
async fn the_worker_consumes_an_import_and_the_operator_can_rewind_it() {
    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    imported(
        &served,
        &build_pack(CONNECTOR, &[tool(CONNECTOR, "a", "Tool a.")]),
        None,
    )
    .await;
    let subscribed = AtomicBool::new(false);
    assert!(
        sweep(&served.state, &subscribed).await.unwrap(),
        "the import row is consumed"
    );
    assert!(
        !sweep(&served.state, &subscribed).await.unwrap(),
        "and only once"
    );
    let status: MutationOutboxStatusView = ok(
        "Status",
        outbox(
            &served,
            "outbox:status",
            MutationOutboxOp::Status {
                target: target(),
                consumer: CONSUMER.to_string(),
            },
        )
        .await,
    );
    assert!(status.status.live);
    assert_eq!((status.status.delivered, status.status.pending), (1, 0));
    let rewound: OutboxRewindReceipt = ok(
        "Rewind",
        outbox(
            &served,
            "outbox:rewind",
            MutationOutboxOp::Rewind {
                target: target(),
                consumer: CONSUMER.to_string(),
                to: RewindTarget::Start,
            },
        )
        .await,
    );
    assert!(rewound.completed);
    assert_eq!(rewound.deleted_deliveries, 1);
    assert!(
        sweep(&served.state, &subscribed).await.unwrap(),
        "a rewound row is delivered again"
    );
}

// spec: MEDULLA-TYPED-PACKS-R009
#[tokio::test]
async fn a_pack_without_schema_is_visible_at_once_and_one_with_schema_after_projection() {
    let served = Served::new();
    let _env = served.with_graph_persistence().await;
    bind(&served, CONNECTOR, ADMIN).await;
    imported(
        &served,
        &build_pack(CONNECTOR, &[tool(CONNECTOR, "a", "Tool a.")]),
        None,
    )
    .await;
    assert!(searchable_ids(&served)
        .await
        .contains(&"mcp:pack-outbox/tool/a".to_string()));

    let schema = "schema-pack";
    bind(&served, schema, ADMIN).await;
    let ontology = Content {
        kind: PackEntryKind::Ontology,
        uri: format!("ontology://{schema}/core.ttl"),
        name: "core.ttl".to_string(),
        media_type: "text/turtle",
        body: b"<https://example.org/Thing> a <http://www.w3.org/2002/07/owl#Class> ; <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://knuckles.team/kg#DomainEntity> .\n".to_vec(),
        input_schema: None,
        annotations: PackAnnotations::default(),
        references: Vec::new(),
    };
    imported(
        &served,
        &build_pack(schema, &[tool(schema, "b", "Tool b."), ontology]),
        None,
    )
    .await;
    let tool_b = "mcp:schema-pack/tool/b".to_string();
    assert!(
        !searchable_ids(&served).await.contains(&tool_b),
        "a pack with schema is dark until its projection applies"
    );
    let subscribed = AtomicBool::new(false);
    while sweep(&served.state, &subscribed).await.unwrap() {}
    if !searchable_ids(&served).await.contains(&tool_b) {
        // A failed projection is recorded on the head with its cause.
        let store = served.state.write().await.ensure_agent_library().unwrap();
        let status = store
            .connector_pack_status(TENANT, &ResourceId::new(schema).unwrap())
            .unwrap();
        panic!(
            "the projection did not flip the head visible: {:?}",
            status.projection
        );
    }
}

// spec: MEDULLA-TYPED-PACKS-R006, MEDULLA-TYPED-PACKS-R093.2
#[tokio::test]
async fn an_admin_forced_reproject_redrives_a_stale_projection_without_losing_later_work() {
    let served = Served::new();
    let _env = served.with_graph_persistence().await;
    let schema = "schema-pack-reproject";
    bind(&served, schema, ADMIN).await;
    let ontology = Content {
        kind: PackEntryKind::Ontology,
        uri: format!("ontology://{schema}/core.ttl"),
        name: "core.ttl".to_string(),
        media_type: "text/turtle",
        body: b"_:b0 <https://example.org/member> <https://example.org/first> .\n".to_vec(),
        input_schema: None,
        annotations: PackAnnotations::default(),
        references: Vec::new(),
    };
    let mut other_ontology = ontology.clone();
    other_ontology.uri = format!("ontology://{schema}/other.ttl");
    other_ontology.name = "other.ttl".to_string();
    other_ontology.body =
        b"_:b0 <https://example.org/member> <https://example.org/second> .\n".to_vec();
    let first = super::imported(
        &served,
        &build_pack(
            schema,
            &[
                tool(schema, "b", "Tool b."),
                ontology.clone(),
                other_ontology.clone(),
            ],
        ),
        None,
    )
    .await;
    let tool_b = "mcp:schema-pack-reproject/tool/b".to_string();
    assert!(
        !searchable_ids(&served).await.contains(&tool_b),
        "a pack with schema stays dark until its projection applies"
    );
    // The administrator forces a Reproject directly, without ever running the
    // worker's sweep: a never-run projection is re-driven on demand.
    let key = "reproject:schema-pack-reproject";
    let _: medulla_catalog_types::connector_pack::PackImportReceipt = ok(
        "Reproject",
        served
            .pack(
                key,
                medulla_catalog_types::connector_pack::ConnectorPackOp::Reproject {
                    request: medulla_catalog_types::connector_pack::ConnectorPackReprojectRequest {
                        context: super::context(key),
                        connector: ResourceId::new(schema).unwrap(),
                    },
                },
            )
            .await,
    );
    assert!(
        searchable_ids(&served).await.contains(&tool_b),
        "the forced Reproject flips the head visible"
    );
    // Read the durable backend, not a helper composition or the resident cache.
    let sources = projected_schema(&served, schema).await;
    let source = sources.dynamic.get(&format!("pack:{schema}")).unwrap();
    let triples =
        medulla_rdf::mapping::parse_turtle(source.ontology_ttl.as_deref().unwrap()).unwrap();
    assert_eq!(triples.len(), 2);
    let identities: std::collections::BTreeSet<_> = triples
        .iter()
        .map(|triple| {
            triple
                .to_string()
                .split_whitespace()
                .next()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        identities.len(),
        2,
        "each member's _:b0 must remain distinct"
    );
    assert!(identities.iter().all(|identity| identity.starts_with("_:")));
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let connector = ResourceId::new(schema).unwrap();
    let before = store.connector_pack_status(TENANT, &connector).unwrap();
    let key = "reproject:schema-pack-reproject:fresh";
    let _: medulla_catalog_types::connector_pack::PackImportReceipt = ok(
        "Reproject",
        served
            .pack(
                key,
                medulla_catalog_types::connector_pack::ConnectorPackOp::Reproject {
                    request: medulla_catalog_types::connector_pack::ConnectorPackReprojectRequest {
                        context: super::context(key),
                        connector: connector.clone(),
                    },
                },
            )
            .await,
    );
    let repeated = projected_schema(&served, schema).await;
    assert_eq!(repeated.composed_digest(), sources.composed_digest());
    assert_eq!(repeated, sources);
    assert_eq!(
        store
            .connector_pack_status(TENANT, &connector)
            .unwrap()
            .head,
        before.head,
        "fresh-key reproject preserves pack and catalog identity"
    );
    // Later work: a second import lands after the forced Reproject and still
    // becomes visible once its own projection runs -- the forced reproject did
    // not lose or block it.
    let tool_c = "mcp:schema-pack-reproject/tool/c".to_string();
    super::imported(
        &served,
        &build_pack(
            schema,
            &[
                tool(schema, "b", "Tool b."),
                tool(schema, "c", "Tool c."),
                ontology,
                other_ontology,
            ],
        ),
        Some(super::head_of(&first)),
    )
    .await;
    let subscribed = AtomicBool::new(false);
    while sweep(&served.state, &subscribed).await.unwrap() {}
    assert!(
        searchable_ids(&served).await.contains(&tool_c),
        "later work after the forced reproject still projects"
    );
}

#[tokio::test]
async fn every_native_store_is_reachable_through_its_scope_selector() {
    let served = Served::new();
    let status = |store, scope| MutationOutboxOp::Status {
        target: OutboxTarget::NativeStore {
            store,
            tenant_id: TENANT.to_string(),
            scope,
        },
        consumer: "operator-probe".to_string(),
    };
    let sql = super::refused(
        "unknown SQL resource",
        outbox(
            &served,
            "outbox:sql",
            status(
                NativeOutboxStore::SqlCatalog,
                NativeOutboxScope::SqlResource {
                    resource: "never-written".to_string(),
                },
            ),
        )
        .await,
    );
    assert_eq!(sql, "OUTBOX_OWNER_UNKNOWN");
    let unknown = super::refused(
        "unknown semantic binding",
        outbox(
            &served,
            "outbox:semantic",
            status(
                NativeOutboxStore::SemanticIndex,
                NativeOutboxScope::SemanticBinding {
                    binding_id: "never-admitted".to_string(),
                },
            ),
        )
        .await,
    );
    assert_eq!(unknown, "OUTBOX_OWNER_UNKNOWN");
    let mismatched = super::refused(
        "tenant scope on a semantic index",
        outbox(
            &served,
            "outbox:mismatch",
            status(NativeOutboxStore::SemanticIndex, NativeOutboxScope::Tenant),
        )
        .await,
    );
    assert_eq!(mismatched, "OUTBOX_SCOPE_MISMATCH");
}

async fn projected_schema(
    served: &Served,
    connector: &str,
) -> std::sync::Arc<medulla_graph_core::graph::GraphSchemaSources> {
    let graph = crate::graph_schema::pack_projection_graph_name(
        TENANT,
        &ResourceId::new(connector).unwrap(),
    )
    .unwrap();
    let backend = served.state.read().await.persistence.clone().unwrap();
    backend
        .read_authoritative_graph_snapshot(&medulla_kernel::persist::sanitize(&graph))
        .await
        .unwrap()
        .unwrap()
        .0
        .schema_sources
}

// spec: MEDULLA-TYPED-PACKS-R074
/// A projection failure sets a durable failed status on the outbox row and
/// keeps the row visible for dead-letter inspection and retry: the worker
/// rejects the row with its stable failure code, the operator's status and
/// dead-letter view show it, and a rewind re-delivers it so the worker can
/// retry.
#[tokio::test]
async fn failed_projection_records_durable_failed_status() {
    let served = Served::new();
    let _env = served.with_graph_persistence().await;
    let schema = "schema-pack-failed";
    bind(&served, schema, ADMIN).await;
    let ontology = Content {
        kind: PackEntryKind::Ontology,
        uri: format!("ontology://{schema}/core.ttl"),
        name: "core.ttl".to_string(),
        media_type: "text/turtle",
        body: b"<https://example.org/Thing> a <http://www.w3.org/2002/07/owl#Class> ; <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://knuckles.team/kg#DomainEntity> .\n".to_vec(),
        input_schema: None,
        annotations: PackAnnotations::default(),
        references: Vec::new(),
    };
    imported(
        &served,
        &build_pack(schema, &[tool(schema, "b", "Tool b."), ontology]),
        None,
    )
    .await;
    assert!(
        !searchable_ids(&served)
            .await
            .contains(&"mcp:schema-pack-failed/tool/b".to_string()),
        "the schema pack is dark until its projection applies"
    );
    // Withdraw the schema body after the import committed: release its engine
    // holder and sweep the CAS, so the projection's body read fails.
    {
        let (blob, store) = {
            let state = served.state.read().await;
            let blob = state.blob_cursors().cloned().unwrap();
            let store = state.agent_library().clone().unwrap();
            (blob, store)
        };
        let plan = store
            .prepare_connector_pack_projection(TENANT, &ResourceId::new(schema).unwrap())
            .unwrap();
        let body = plan
            .bodies
            .iter()
            .find(|body| body.kind == PackEntryKind::Ontology)
            .unwrap();
        let caller = medulla_server_api::carrier::CarrierAuthority::from_verified(
            &medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test(
                ADMIN,
            ),
        )
        .unwrap();
        let method = medulla_protocol_types::protocol::Method::BlobUnref {
            digest: body.engine_manifest_digest.clone(),
        };
        let (batch, now) = medulla_server_ops::api::ingest::compile_blob_batch(
            blob.store.as_ref(),
            7,
            &caller,
            &method,
        )
        .unwrap();
        let change = medulla_server_blob::blob::store::HolderChange::release(
            &body.engine_manifest_digest,
            medulla_server_blob::blob::store::HolderId::new(
                &medulla_server_blob::blob::engine_bodies::engine_body_holder(
                    TENANT,
                    &body.body_sha256,
                ),
            )
            .unwrap(),
        )
        .unwrap();
        blob.store.holder_batch(&change, &batch, now).unwrap();
        // The default sweep keeps unheld blobs for a day of grace; this
        // reclamation pass is immediate, so the body read fails right away.
        let gc_method = medulla_protocol_types::protocol::Method::BlobGc;
        let (gc_batch, gc_now) = medulla_server_ops::api::ingest::compile_blob_batch(
            blob.store.as_ref(),
            8,
            &caller,
            &gc_method,
        )
        .unwrap();
        let request = medulla_server_blob::blob::store::SweepRequest::new(
            medulla_server_blob::blob::store::GcOwnerScope::AllOwners,
            medulla_server_blob::blob::store::BlobRetentionPolicy::new(
                0,
                medulla_server_blob::blob::store::DEFAULT_UPLOAD_TTL_MS,
            )
            .unwrap(),
        );
        let reclaimed = blob.store.sweep_batch(&request, &gc_batch, gc_now).unwrap();
        assert!(
            blob.store
                .get_manifest(&body.engine_manifest_digest)
                .unwrap()
                .is_none(),
            "the withdrawn body must be gone: {reclaimed:?}"
        );
    }
    // The worker's sweep claims the row; the projection's body read fails; the
    // worker rejects the row instead of advancing.
    let subscribed = AtomicBool::new(false);
    assert!(
        sweep(&served.state, &subscribed).await.unwrap(),
        "the failed projection is still work for the sweep"
    );
    let status: MutationOutboxStatusView = ok(
        "Status",
        outbox(
            &served,
            "outbox:status",
            MutationOutboxOp::Status {
                target: target(),
                consumer: CONSUMER.to_string(),
            },
        )
        .await,
    );
    assert_eq!(status.status.dead_lettered, 1, "{status:?}");
    assert_eq!(status.status.pending, 0, "{status:?}");
    // The head records the failure's stable code and cause, not a bare error.
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let failed = store
        .connector_pack_status(TENANT, &ResourceId::new(schema).unwrap())
        .unwrap()
        .projection;
    match &failed {
        medulla_catalog_types::connector_pack::PackProjectionState::Failed { code, detail } => {
            assert_eq!(code, "BODY_MISSING", "{failed:?}");
            assert!(detail.as_deref().unwrap().contains("BODY_MISSING"));
        }
        other => panic!("the head must record the failed projection: {other:?}"),
    }
    // The failed row stays visible for dead-letter inspection...
    let letters: OutboxDeadLetterPage = ok(
        "DeadLetters",
        outbox(
            &served,
            "outbox:letters",
            MutationOutboxOp::DeadLetters {
                target: target(),
                consumer: CONSUMER.to_string(),
                after: None,
                limit: 4,
            },
        )
        .await,
    );
    assert_eq!(letters.items.len(), 1, "{letters:?}");
    assert_eq!(
        letters.items.as_slice()[0].topic,
        CONNECTOR_PACK_IMPORT_TOPIC
    );
    // ...and a rewind re-delivers it, so the worker can retry.
    let rewound: OutboxRewindReceipt = ok(
        "Rewind",
        outbox(
            &served,
            "outbox:rewind",
            MutationOutboxOp::Rewind {
                target: target(),
                consumer: CONSUMER.to_string(),
                to: RewindTarget::Start,
            },
        )
        .await,
    );
    assert!(rewound.completed);
    assert_eq!(rewound.deleted_deliveries, 1);
    assert!(
        sweep(&served.state, &subscribed).await.unwrap(),
        "the rewound row is claimed again for retry"
    );
}

// spec: MEDULLA-TYPED-PACKS-R096.1
/// A retryable projection rejection never leaves the head dark without a
/// state: the head reports `failed` with the stable code, the worker leaves
/// the lease unresolved (no dead letter) so the kernel redelivers the row, and
/// the failed state stays visible until a served reproject succeeds.
#[tokio::test]
async fn rejected_projection_is_visible_and_retried() {
    use crate::admin::connector_pack::reproject::{
        is_retryable_projection_error, INJECT_BLOB_UNAVAILABLE, RETRYABLE_PROJECTION_CODES,
    };
    for code in RETRYABLE_PROJECTION_CODES {
        assert!(is_retryable_projection_error(&format!("{code}: cause")));
    }
    assert!(!is_retryable_projection_error("BODY_MISSING: gone"));
    assert!(!is_retryable_projection_error("PACK_PROJECTION_FAILED"));

    let served = Served::new();
    let _env = served.with_graph_persistence().await;
    let schema = "schema-pack-retryable";
    bind(&served, schema, ADMIN).await;
    let ontology = Content {
        kind: PackEntryKind::Ontology,
        uri: format!("ontology://{schema}/core.ttl"),
        name: "core.ttl".to_string(),
        media_type: "text/turtle",
        body: b"<https://example.org/Thing> a <http://www.w3.org/2002/07/owl#Class> ; <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://knuckles.team/kg#DomainEntity> .\n".to_vec(),
        input_schema: None,
        annotations: PackAnnotations::default(),
        references: Vec::new(),
    };
    imported(
        &served,
        &build_pack(schema, &[tool(schema, "b", "Tool b."), ontology]),
        None,
    )
    .await;
    *INJECT_BLOB_UNAVAILABLE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(schema.to_string());
    let subscribed = AtomicBool::new(false);
    sweep(&served.state, &subscribed).await.unwrap();
    let status: MutationOutboxStatusView = ok(
        "Status",
        outbox(
            &served,
            "outbox:status:retryable",
            MutationOutboxOp::Status {
                target: target(),
                consumer: CONSUMER.to_string(),
            },
        )
        .await,
    );
    assert_eq!(
        status.status.dead_lettered, 0,
        "a retryable rejection is redelivered, not dead-lettered: {status:?}"
    );
    let tool_b = format!("mcp:{schema}/tool/b");
    assert!(
        !searchable_ids(&served).await.contains(&tool_b),
        "a dark head never reads as applied"
    );
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let failed = store
        .connector_pack_status(TENANT, &ResourceId::new(schema).unwrap())
        .unwrap()
        .projection;
    match &failed {
        medulla_catalog_types::connector_pack::PackProjectionState::Failed { code, detail } => {
            assert_eq!(code, "PACK_PROJECTION_BLOB_UNAVAILABLE", "{failed:?}");
            assert!(detail.is_some());
        }
        other => panic!("the head must record the failed projection: {other:?}"),
    }
    // The served reproject clears the failed state.
    let key = "reproject:schema-pack-retryable";
    let _: medulla_catalog_types::connector_pack::PackImportReceipt = ok(
        "Reproject",
        served
            .pack(
                key,
                medulla_catalog_types::connector_pack::ConnectorPackOp::Reproject {
                    request: medulla_catalog_types::connector_pack::ConnectorPackReprojectRequest {
                        context: super::context(key),
                        connector: ResourceId::new(schema).unwrap(),
                    },
                },
            )
            .await,
    );
    assert!(searchable_ids(&served).await.contains(&tool_b));
}
