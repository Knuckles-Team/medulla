//! R722.2 served source approval and current-contract reset proof.

#[cfg(feature = "redb")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_catalog_types::{
    connector_pack::PackAnnotations, connector_pack::PackEntryKind,
    connector_pack::PackSourceApproval,
};
use medulla_catalog_types::{
    foreign_read::ForeignReadAbortRequest, foreign_read::ForeignReadAbortResult,
    foreign_read::ForeignReadChunkRequest, foreign_read::ForeignReadChunkResult,
    foreign_read::ForeignReadCompleteRequest, foreign_read::ForeignReadCompleteResult,
    foreign_read::ForeignReadLimits, foreign_read::ForeignReadOpenRequest,
    foreign_read::ForeignReadOpenResult, foreign_read::ForeignReadRow,
    foreign_read::FOREIGN_READ_SCHEMA_VERSION,
};
use medulla_types::contract::Digest256;

use super::{
    approve_source, bind, build_pack, context, head_of, imported, ok, refused, tool, Content,
    Served, ADMIN, TENANT,
};

/// The quota a registration without one of its own takes: the resolved
/// `foreign.default_quota.*` knobs, as the register handler resolves them.
fn default_quota() -> medulla_catalog_types::consolidation_isolation::SourceQuota {
    let knobs = medulla_server_api::knob_config::knobs();
    medulla_catalog_types::consolidation_isolation::SourceQuota {
        max_concurrent: u32::try_from(knobs.foreign_default_quota_max_concurrent)
            .unwrap_or(u32::MAX),
        max_rows: knobs.foreign_default_quota_max_rows,
        max_wall_ms: knobs.foreign_default_quota_max_wall_ms,
    }
}

fn ontology(connector: &str, body: String) -> Content {
    Content {
        kind: PackEntryKind::Ontology,
        uri: format!("ontology://{connector}/core.ttl"),
        name: "core.ttl".to_string(),
        media_type: "text/turtle",
        body: body.into_bytes(),
        input_schema: None,
        annotations: PackAnnotations::default(),
        references: Vec::new(),
    }
}

fn source_contract(connector: &str, body: String) -> Content {
    Content {
        kind: PackEntryKind::Resource,
        uri: format!("contract://{connector}"),
        name: "source-contract".to_string(),
        media_type: "text/turtle",
        body: body.into_bytes(),
        input_schema: None,
        annotations: PackAnnotations::default(),
        references: Vec::new(),
    }
}

const CONTRACT: &str = r#"@prefix ac: <https://knuckles-team.github.io/ontology/access#> .
<urn:supplier-contract> a ac:AccessContract ; ac:classIri <urn:Supplier> ;
 ac:sourceId "suppliers" ; ac:accessKind "mcp_tool" ; ac:operation "list_suppliers" ;
 ac:entity "Supplier" ; ac:keyField "id" ."#;

// spec: MEDULLA-FEDERATED-QUERY-R722.2
// spec: MEDULLA-FEDERATED-QUERY-R722.7
#[tokio::test]
async fn approve_source_requires_current_head_and_contract_digest() {
    let connector = "r722-approval";
    let served = Served::new();
    bind(&served, connector, ADMIN).await;
    let initial = build_pack(
        connector,
        &[
            tool(connector, "a", "A."),
            ontology(connector, CONTRACT.to_string()),
        ],
    );
    let first = imported(&served, &initial, None).await;
    let connector_id = medulla_types::contract::ResourceId::new(connector).unwrap();
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let source = store
        .connector_pack_members(TENANT, &connector_id)
        .unwrap()
        .into_iter()
        .find(|member| !member.source_registrations.is_empty())
        .expect("the imported ontology has one source registration");
    let registration = &source.source_registrations[0];
    let contract_iri = registration.contract_iri.clone();
    let resource_uri = registration.resource_uri.clone();
    let contract_digest = source.body_sha256;
    drop(store);

    let first_head = head_of(&first);
    let wrong_key = "connector-pack:wrong-tenant:approve-source";
    let mut wrong_context = context(wrong_key);
    wrong_context.tenant_id = "tenant-other".to_string();
    let wrong_tenant = refused(
        "wrong-tenant source approval",
        served
            .call_as(
                ADMIN,
                &["*"],
                wrong_key,
                medulla_protocol_types::protocol::Method::ConnectorPack {
                    op: Box::new(
                        medulla_catalog_types::connector_pack::ConnectorPackOp::ApproveSource {
                            request:
                                medulla_catalog_types::connector_pack::ConnectorPackSourceApprovalRequest {
                                    context: wrong_context,
                                    connector: connector_id.clone(),
                                    contract_iri: contract_iri.clone(),
                                    resource_uri: resource_uri.clone(),
                                    expected_head: first_head.clone(),
                                    contract_digest,
                                },
                        },
                    ),
                },
            )
            .await,
    );
    assert!(wrong_tenant.contains("ACCESS_DENIED"), "{wrong_tenant}");

    let approved: medulla_catalog_types::connector_pack::ConnectorPackSourceApprovalResult = ok(
        "approve source",
        approve_source(
            &served,
            connector,
            &contract_iri,
            &resource_uri,
            first_head.clone(),
            contract_digest,
        )
        .await,
    );
    assert_eq!(approved.registration.approval, PackSourceApproval::Approved);
    assert_eq!(
        approved.schema_version,
        medulla_catalog_types::connector_pack::CONNECTOR_PACK_SCHEMA_VERSION
    );
    assert_eq!(approved.tenant_id, TENANT);
    assert_eq!(approved.connector, connector_id);
    assert_eq!(approved.head, first_head);
    assert!(!approved.batch_id.is_empty());
    assert!(approved.committed_version > 0);
    let store = served.state.write().await.ensure_agent_library().unwrap();
    assert!(store
        .verify_connector_pack_source_approval(&approved)
        .is_ok());
    let mut changed_receipt = approved.clone();
    changed_receipt.contract_digest = Digest256::from_bytes([0x5a; 32]);
    let refusal = store
        .verify_connector_pack_source_approval(&changed_receipt)
        .expect_err("a receipt with a changed body digest must not bind");
    assert!(refusal.contains("SOURCE_CONTRACT_MISMATCH"), "{refusal}");
    drop(store);
    let replay: medulla_catalog_types::connector_pack::ConnectorPackSourceApprovalResult = ok(
        "replay approved source",
        approve_source(
            &served,
            connector,
            &contract_iri,
            &resource_uri,
            first_head.clone(),
            contract_digest,
        )
        .await,
    );
    assert_eq!(replay, approved);

    // An unrelated member/head change keeps the same source body and contract
    // identity; the approved source must remain approved.
    let unrelated = build_pack(
        connector,
        &[
            tool(connector, "a", "A."),
            tool(connector, "b", "B."),
            ontology(connector, CONTRACT.to_string()),
        ],
    );
    let second = imported(&served, &unrelated, Some(first_head.clone())).await;
    assert_ne!(second.pack_digest, first.pack_digest);
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let current = store
        .connector_pack_members(TENANT, &connector_id)
        .unwrap()
        .into_iter()
        .find(|member| !member.source_registrations.is_empty())
        .expect("unchanged source member remains current");
    assert_eq!(
        current.source_registrations[0].approval,
        PackSourceApproval::Approved
    );
    drop(store);

    // A changed source body gets a new current head and is held again.
    let changed_body = CONTRACT.replace("list_suppliers", "list_customers");
    let changed = build_pack(
        connector,
        &[
            tool(connector, "a", "A."),
            ontology(connector, changed_body),
        ],
    );
    let third = imported(&served, &changed, Some(head_of(&second))).await;
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let changed_member = store
        .connector_pack_members(TENANT, &connector_id)
        .unwrap()
        .into_iter()
        .find(|member| !member.source_registrations.is_empty())
        .expect("changed source member remains current");
    assert_eq!(
        changed_member.source_registrations[0].approval,
        PackSourceApproval::Held
    );
    drop(store);

    let absent = refused(
        "absent source registration",
        approve_source(
            &served,
            connector,
            &contract_iri,
            "ontology://r722-approval/missing.ttl",
            head_of(&third),
            changed_member.body_sha256,
        )
        .await,
    );
    assert!(absent.contains("SOURCE_REGISTRATION_NOT_FOUND"), "{absent}");

    let stale = refused(
        "stale source approval",
        approve_source(
            &served,
            connector,
            &contract_iri,
            &resource_uri,
            head_of(&second),
            contract_digest,
        )
        .await,
    );
    assert!(stale.contains("PACK_HEAD_CONFLICT"), "{stale}");
    let mismatch = refused(
        "changed source digest",
        approve_source(
            &served,
            connector,
            &contract_iri,
            &resource_uri,
            head_of(&third),
            contract_digest,
        )
        .await,
    );
    assert!(mismatch.contains("SOURCE_CONTRACT_MISMATCH"), "{mismatch}");
}

/// The four host-driven lifecycle methods must be reachable through the same
/// authenticated dispatch boundary as pack admission. This proves the
/// registered, approved-source route; FleetSourceExecutor production fetching
/// remains a separate served acceptance leaf.
#[tokio::test]
async fn foreign_read_four_phase_uses_authenticated_dispatch() {
    let connector = "r722-four-phase";
    let served = Served::new();
    let _env = served.with_graph_persistence().await;
    bind(&served, connector, ADMIN).await;
    let initial = build_pack(
        connector,
        &[
            tool(connector, "a", "A."),
            source_contract(
                connector,
                format!(
                    r#"@prefix ac: <https://knuckles-team.github.io/ontology/access#> .
<data://{connector}/suppliers#access> a ac:AccessContract ;
 ac:classIri "urn:Supplier" ; ac:sourceId "{connector}/suppliers" ;
 ac:entity "suppliers" ; ac:keyField "id" ; ac:accessKind "mcp_tool" ;
 ac:operation "list_suppliers" ; ac:pagination "none" ."#
                ),
            ),
        ],
    );
    let first = imported(&served, &initial, None).await;
    let connector_id = medulla_types::contract::ResourceId::new(connector).unwrap();
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let member = store
        .connector_pack_members(TENANT, &connector_id)
        .unwrap()
        .into_iter()
        .find(|member| !member.source_registrations.is_empty())
        .expect("source registration");
    let registration = member.source_registrations[0].clone();
    let contract_digest = member.body_sha256;
    drop(store);

    let approved: medulla_catalog_types::connector_pack::ConnectorPackSourceApprovalResult = ok(
        "approve source for four-phase route",
        approve_source(
            &served,
            connector,
            &registration.contract_iri,
            &registration.resource_uri,
            head_of(&first),
            contract_digest,
        )
        .await,
    );

    let source_name = registration.source_id.clone();
    let registered: String = ok(
        "register foreign source",
        served
            .call(
                "r722-four-phase:register",
                medulla_protocol_types::protocol::Method::RegisterForeignSource {
                    name: source_name.clone(),
                    quota: None,
                    source: medulla_query_types::wire::ForeignSourceSpec::Mcp {
                        server: connector.to_string(),
                        tool: "list_suppliers".to_string(),
                        capabilities: Default::default(),
                    },
                    acceleration: None,
                },
            )
            .await,
    );
    assert_eq!(registered, source_name);

    let limits = ForeignReadLimits {
        max_rows: 10,
        max_bytes: 1024,
        chunk_rows: 2,
        chunk_bytes: 512,
        deadline_ms: 10_000,
    };
    let plan = medulla_exec::algebra::Plan::new(vec![medulla_exec::algebra::Op::ForeignScan {
        source: Box::new(medulla_query_types::wire::ForeignSourceSpec::Named {
            name: source_name.clone(),
        }),
        join: false,
    }]);
    let plan_digest = Digest256::sha256(&rmp_serde::to_vec_named(&plan).unwrap());
    let query_digest = plan_digest;
    let opened: ForeignReadOpenResult = ok(
        "authenticated foreign read open",
        served
            .call(
                "r722-four-phase:open",
                medulla_protocol_types::protocol::Method::ForeignReadOpen {
                    request: ForeignReadOpenRequest {
                        schema_version: FOREIGN_READ_SCHEMA_VERSION,
                        source_name: source_name.clone(),
                        approval: approved.clone(),
                        plan_digest,
                        query_digest,
                        limits,
                        submission: None,
                    },
                },
            )
            .await,
    );
    assert_eq!(opened.execution.query_digest, query_digest);
    assert_eq!(opened.next_sequence, 0);
    assert_eq!(opened.source_identity.source_ref, registration.resource_uri);

    let chunked: ForeignReadChunkResult = ok(
        "authenticated foreign read chunk",
        served
            .call(
                "r722-four-phase:chunk",
                medulla_protocol_types::protocol::Method::ForeignReadChunk {
                    request: ForeignReadChunkRequest {
                        execution: opened.execution,
                        sequence: 0,
                        source_identity: opened.source_identity.clone(),
                        rows: vec![ForeignReadRow {
                            id: "supplier-1".to_string(),
                            score: Some(0.5),
                            columns: BTreeMap::from([(
                                "name".to_string(),
                                serde_json::Value::String("Acme".to_string()),
                            )]),
                            provenance: vec!["r722-four-phase:supplier-1".to_string()],
                        }],
                    },
                },
            )
            .await,
    );
    assert_eq!(chunked.accepted_rows, 1);
    assert_eq!(chunked.next_sequence, 1);

    let completed: ForeignReadCompleteResult = ok(
        "authenticated foreign read complete",
        served
            .call(
                "r722-four-phase:complete",
                medulla_protocol_types::protocol::Method::ForeignReadComplete {
                    request: ForeignReadCompleteRequest {
                        execution: opened.execution,
                        sequence: 1,
                    },
                },
            )
            .await,
    );
    assert_eq!(completed.total_rows, 1);

    let response = served
        .call(
            "r722-four-phase:query",
            medulla_protocol_types::protocol::Method::UnifiedQuery {
                plan: plan.clone(),
                foreign_execution: Some(opened.execution),

                foreign_executions: Default::default(),
                code_map: None,
                spec_report: None,
                retrieval: None,
                latency_class: Default::default(),
            },
        )
        .await;
    let raw = match &response.result {
        Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) => bytes,
        other => panic!("expected foreign rows, got {other:?}"),
    };
    let rows: Vec<medulla_types::unified_row::UnifiedRow> = rmp_serde::from_slice(raw).unwrap();
    let ranked: Vec<(&str, Option<f32>)> = rows
        .iter()
        .map(|row| (row.id.as_str(), row.score))
        .collect();
    assert_eq!(ranked, vec![("supplier-1", Some(0.5))]);
    let replay = served
        .call(
            "r722-four-phase:query-replay",
            medulla_protocol_types::protocol::Method::UnifiedQuery {
                plan,
                foreign_execution: Some(opened.execution),

                foreign_executions: Default::default(),
                code_map: None,
                spec_report: None,
                retrieval: None,
                latency_class: Default::default(),
            },
        )
        .await;
    assert!(refused("replay", replay).contains("FOREIGN_READ_REPLAY"));

    let abort_query_digest = Digest256::sha256(b"r722-four-phase-abort-query");
    let abort_opened: ForeignReadOpenResult = ok(
        "authenticated foreign read abort open",
        served
            .call(
                "r722-four-phase:abort-open",
                medulla_protocol_types::protocol::Method::ForeignReadOpen {
                    request: ForeignReadOpenRequest {
                        schema_version: FOREIGN_READ_SCHEMA_VERSION,
                        source_name,
                        approval: approved,
                        plan_digest,
                        query_digest: abort_query_digest,
                        limits,
                        submission: None,
                    },
                },
            )
            .await,
    );
    let aborted: ForeignReadAbortResult = ok(
        "authenticated foreign read abort",
        served
            .call(
                "r722-four-phase:abort",
                medulla_protocol_types::protocol::Method::ForeignReadAbort {
                    request: ForeignReadAbortRequest {
                        execution: abort_opened.execution,
                        sequence: 0,
                        reason: "test cleanup".to_string(),
                    },
                },
            )
            .await,
    );
    assert_eq!(aborted.sequence, 0);
    assert_eq!(aborted.reason, "test cleanup");
}

struct BoundRows;

impl medulla_federation::federation::ForeignSource for BoundRows {
    fn fetch(&self) -> Result<medulla_exec::rowset::RowSet, String> {
        Ok(medulla_exec::rowset::RowSet::from_ids([
            "bound-row".to_string()
        ]))
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R722.3
#[tokio::test]
async fn bind_for_requires_owner_catalog_and_rechecks_approval() {
    let connector = "r722-bind";
    let served = Served::new();
    bind(&served, connector, ADMIN).await;
    let initial = build_pack(
        connector,
        &[
            tool(connector, "a", "A."),
            ontology(connector, CONTRACT.to_string()),
        ],
    );
    let first = imported(&served, &initial, None).await;
    let connector_id = medulla_types::contract::ResourceId::new(connector).unwrap();
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let member = store
        .connector_pack_members(TENANT, &connector_id)
        .unwrap()
        .into_iter()
        .find(|member| !member.source_registrations.is_empty())
        .expect("source registration");
    let registration = member.source_registrations[0].clone();
    let digest = member.body_sha256;
    drop(store);
    let approved: medulla_catalog_types::connector_pack::ConnectorPackSourceApprovalResult = ok(
        "approve source for binding",
        approve_source(
            &served,
            connector,
            &registration.contract_iri,
            &registration.resource_uri,
            head_of(&first),
            digest,
        )
        .await,
    );

    let caller = medulla_server_api::carrier::CarrierAuthority::from_verified(
        &medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test(ADMIN),
    )
    .unwrap();
    let spec = medulla_query_types::wire::ForeignSourceSpec::Mcp {
        server: connector.to_string(),
        tool: "list_suppliers".to_string(),
        capabilities: Default::default(),
    };
    let catalog =
        medulla_server_ops::api::query_txn::foreign_catalog::ForeignSourceCatalog::default();
    catalog
        .register(
            &caller,
            registration.source_id.clone(),
            spec.clone(),
            default_quota(),
            None,
        )
        .unwrap();
    let descriptor =
        medulla_server_ops::api::query_txn::foreign_catalog::ForeignSourceBindingDescriptor {
            name: registration.source_id.clone(),
            spec,
            registration: approved.registration.clone(),
            contract_digest: approved.contract_digest,
        };
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let scoped = catalog
        .bind_for(
            &caller,
            store.as_ref(),
            &medulla_isolation::isolation::IsolationLayer::new(),
            &approved,
            &descriptor,
            Arc::new(BoundRows),
        )
        .expect("approved source binds only from its owner's catalog");
    assert_eq!(
        scoped.registry().resolve(&descriptor.name).unwrap().ids(),
        vec!["bound-row"]
    );
    drop(store);

    // A durable approval for one MCP operation must never authorize another
    // operation or a different transport, even when both values are registered
    // in the same owner's catalog.
    for wrong_spec in [
        medulla_query_types::wire::ForeignSourceSpec::Mcp {
            server: connector.to_string(),
            tool: "list_customers".to_string(),
            capabilities: Default::default(),
        },
        medulla_query_types::wire::ForeignSourceSpec::Api {
            operation: "list_suppliers".to_string(),
            capabilities: Default::default(),
        },
    ] {
        let wrong_catalog =
            medulla_server_ops::api::query_txn::foreign_catalog::ForeignSourceCatalog::default();
        wrong_catalog
            .register(
                &caller,
                registration.source_id.clone(),
                wrong_spec.clone(),
                default_quota(),
                None,
            )
            .unwrap();
        let mut wrong_descriptor = descriptor.clone();
        wrong_descriptor.spec = wrong_spec;
        let store = served.state.write().await.ensure_agent_library().unwrap();
        let refusal = match wrong_catalog.bind_for(
            &caller,
            store.as_ref(),
            &medulla_isolation::isolation::IsolationLayer::new(),
            &approved,
            &wrong_descriptor,
            Arc::new(BoundRows),
        ) {
            Ok(_) => panic!("a different source operation must not bind"),
            Err(error) => error,
        };
        assert!(refusal.contains("SOURCE_BINDING_MISMATCH"), "{refusal}");
    }

    let mut held_receipt = approved.clone();
    held_receipt.registration.approval = PackSourceApproval::Held;
    let mut held_descriptor = descriptor.clone();
    held_descriptor.registration.approval = PackSourceApproval::Held;
    let held_refusal = match catalog.bind_for(
        &caller,
        served
            .state
            .write()
            .await
            .ensure_agent_library()
            .unwrap()
            .as_ref(),
        &medulla_isolation::isolation::IsolationLayer::new(),
        &held_receipt,
        &held_descriptor,
        Arc::new(BoundRows),
    ) {
        Ok(_) => panic!("a held registration must not bind"),
        Err(error) => error,
    };
    assert!(
        held_refusal.contains("SOURCE_APPROVAL_NOT_ADMITTED"),
        "{held_refusal}"
    );

    let mut stale_receipt = approved.clone();
    stale_receipt.head.binding_revision += 1;
    let stale_refusal = match catalog.bind_for(
        &caller,
        served
            .state
            .write()
            .await
            .ensure_agent_library()
            .unwrap()
            .as_ref(),
        &medulla_isolation::isolation::IsolationLayer::new(),
        &stale_receipt,
        &descriptor,
        Arc::new(BoundRows),
    ) {
        Ok(_) => panic!("a stale head receipt must not bind"),
        Err(error) => error,
    };
    assert!(
        stale_refusal.contains("PACK_HEAD_CONFLICT"),
        "{stale_refusal}"
    );

    let mut changed_body = approved.clone();
    changed_body.contract_digest = Digest256::from_bytes([0x33; 32]);
    let mut changed_descriptor = descriptor.clone();
    changed_descriptor.contract_digest = changed_body.contract_digest;
    let body_refusal = match catalog.bind_for(
        &caller,
        served
            .state
            .write()
            .await
            .ensure_agent_library()
            .unwrap()
            .as_ref(),
        &medulla_isolation::isolation::IsolationLayer::new(),
        &changed_body,
        &changed_descriptor,
        Arc::new(BoundRows),
    ) {
        Ok(_) => panic!("a receipt naming a changed body must not bind"),
        Err(error) => error,
    };
    assert!(
        body_refusal.contains("SOURCE_CONTRACT_MISMATCH"),
        "{body_refusal}"
    );

    let mut changed_spec = descriptor.clone();
    changed_spec.spec = medulla_query_types::wire::ForeignSourceSpec::Api {
        operation: "different_operation".to_string(),
        capabilities: Default::default(),
    };
    let spec_refusal = match catalog.bind_for(
        &caller,
        served
            .state
            .write()
            .await
            .ensure_agent_library()
            .unwrap()
            .as_ref(),
        &medulla_isolation::isolation::IsolationLayer::new(),
        &approved,
        &changed_spec,
        Arc::new(BoundRows),
    ) {
        Ok(_) => panic!("a descriptor different from the owner catalog must not bind"),
        Err(error) => error,
    };
    assert!(
        spec_refusal.contains("SOURCE_BINDING_MISMATCH"),
        "{spec_refusal}"
    );

    let other = medulla_server_api::carrier::CarrierAuthority::from_verified(
        &medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test(
            "other-agent",
        ),
    )
    .unwrap();
    let refused = match catalog.bind_for(
        &other,
        served
            .state
            .write()
            .await
            .ensure_agent_library()
            .unwrap()
            .as_ref(),
        &medulla_isolation::isolation::IsolationLayer::new(),
        &approved,
        &descriptor,
        Arc::new(BoundRows),
    ) {
        Ok(_) => panic!("same-tenant non-owner must not use the owner's catalog entry"),
        Err(error) => error,
    };
    assert!(
        refused.contains("SOURCE_BINDING_NOT_REGISTERED"),
        "{refused}"
    );

    let wrong_tenant = medulla_server_api::carrier::CarrierAuthority::from_verified(
        &medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test_in_tenant(
            ADMIN,
            "tenant-other",
        ),
    )
    .unwrap();
    let tenant_refusal = match catalog.bind_for(
        &wrong_tenant,
        served
            .state
            .write()
            .await
            .ensure_agent_library()
            .unwrap()
            .as_ref(),
        &medulla_isolation::isolation::IsolationLayer::new(),
        &approved,
        &descriptor,
        Arc::new(BoundRows),
    ) {
        Ok(_) => panic!("a source approval from another tenant must not bind"),
        Err(error) => error,
    };
    assert!(tenant_refusal.contains("ACCESS_DENIED"), "{tenant_refusal}");
}

/// One admitted query consumes two individually approved source executions.
#[tokio::test]
async fn two_approved_sources_join_once_and_duplicate_refuses_atomically() {
    use medulla_types::contract::BoundedVec;
    let connector = "r722-two-sources";
    let served = Served::new();
    let _env = served.with_graph_persistence().await;
    bind(&served, connector, ADMIN).await;
    let contract = format!(
        r#"@prefix ac: <https://knuckles-team.github.io/ontology/access#> .
<data://{connector}/first#access> a ac:AccessContract ; ac:classIri "urn:Item" ; ac:sourceId "{connector}/first" ; ac:entity "first" ; ac:keyField "id" ; ac:accessKind "mcp_tool" ; ac:operation "list_first" ; ac:pagination "none" .
<data://{connector}/second#access> a ac:AccessContract ; ac:classIri "urn:Item" ; ac:sourceId "{connector}/second" ; ac:entity "second" ; ac:keyField "id" ; ac:accessKind "mcp_tool" ; ac:operation "list_second" ; ac:pagination "none" ."#
    );
    let pack = build_pack(
        connector,
        &[
            tool(connector, "list_first", "First source."),
            tool(connector, "list_second", "Second source."),
            source_contract(connector, contract),
        ],
    );
    let imported_pack = imported(&served, &pack, None).await;
    let connector_id = medulla_types::contract::ResourceId::new(connector).unwrap();
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let member = store
        .connector_pack_members(TENANT, &connector_id)
        .unwrap()
        .into_iter()
        .find(|member| member.uri == format!("contract://{connector}"))
        .unwrap();
    let registrations = member.source_registrations.clone();
    let contract_digest = member.body_sha256;
    drop(store);
    assert_eq!(registrations.len(), 2);
    let names: Vec<String> = registrations
        .iter()
        .map(|item| item.source_id.clone())
        .collect();
    let plan = medulla_exec::algebra::Plan::new(
        names
            .iter()
            .enumerate()
            .map(|(index, name)| medulla_exec::algebra::Op::ForeignScan {
                source: Box::new(medulla_query_types::wire::ForeignSourceSpec::Named {
                    name: name.clone(),
                }),
                join: index != 0,
            })
            .collect(),
    );
    let digest = Digest256::sha256(&rmp_serde::to_vec_named(&plan).unwrap());
    let mut executions = Vec::new();
    for registration in &registrations {
        let approved: medulla_catalog_types::connector_pack::ConnectorPackSourceApprovalResult = ok(
            "approve source",
            approve_source(
                &served,
                connector,
                &registration.contract_iri,
                &registration.resource_uri,
                head_of(&imported_pack),
                contract_digest,
            )
            .await,
        );
        let _: String = ok(
            "register source",
            served
                .call(
                    &format!("r722-two-sources:register:{}", registration.source_id),
                    medulla_protocol_types::protocol::Method::RegisterForeignSource {
                        name: registration.source_id.clone(),
                        quota: None,
                        source: medulla_query_types::wire::ForeignSourceSpec::Mcp {
                            server: connector.to_string(),
                            tool: registration.operation.clone(),
                            capabilities: Default::default(),
                        },
                        acceleration: None,
                    },
                )
                .await,
        );
        let opened: ForeignReadOpenResult = ok(
            "open",
            served
                .call(
                    &format!("r722-two-sources:open:{}", registration.source_id),
                    medulla_protocol_types::protocol::Method::ForeignReadOpen {
                        request: ForeignReadOpenRequest {
                            schema_version: FOREIGN_READ_SCHEMA_VERSION,
                            source_name: registration.source_id.clone(),
                            approval: approved,
                            plan_digest: digest,
                            query_digest: digest,
                            limits: ForeignReadLimits {
                                max_rows: 10,
                                max_bytes: 1024,
                                chunk_rows: 2,
                                chunk_bytes: 512,
                                deadline_ms: 10_000,
                            },
                            submission: None,
                        },
                    },
                )
                .await,
        );
        let _: ForeignReadChunkResult = ok(
            "chunk",
            served
                .call(
                    &format!("r722-two-sources:chunk:{}", registration.source_id),
                    medulla_protocol_types::protocol::Method::ForeignReadChunk {
                        request: ForeignReadChunkRequest {
                            execution: opened.execution,
                            sequence: 0,
                            source_identity: opened.source_identity,
                            rows: vec![ForeignReadRow {
                                id: "shared-1".into(),
                                score: Some(0.5),
                                columns: BTreeMap::new(),
                                provenance: Vec::new(),
                            }],
                        },
                    },
                )
                .await,
        );
        let _: ForeignReadCompleteResult = ok(
            "complete",
            served
                .call(
                    &format!("r722-two-sources:complete:{}", registration.source_id),
                    medulla_protocol_types::protocol::Method::ForeignReadComplete {
                        request: ForeignReadCompleteRequest {
                            execution: opened.execution,
                            sequence: 1,
                        },
                    },
                )
                .await,
        );
        executions.push(opened.execution);
    }
    let duplicate = served
        .call(
            "r722-two-sources:duplicate",
            medulla_protocol_types::protocol::Method::UnifiedQuery {
                plan: plan.clone(),
                foreign_execution: None,
                foreign_executions: BoundedVec::new(vec![executions[0], executions[0]]).unwrap(),
                code_map: None,
                spec_report: None,
                retrieval: None,
                latency_class: Default::default(),
            },
        )
        .await;
    let duplicate_error = refused("duplicate", duplicate);
    assert!(
        duplicate_error.contains("INVALID_ARGUMENT"),
        "{duplicate_error}"
    );
    let mut wrong_query = executions.clone();
    wrong_query[1].query_digest = Digest256::sha256(b"different-query");
    let mismatch = served
        .call(
            "r722-two-sources:mismatch",
            medulla_protocol_types::protocol::Method::UnifiedQuery {
                plan: plan.clone(),
                foreign_execution: None,
                foreign_executions: BoundedVec::new(wrong_query).unwrap(),
                code_map: None,
                spec_report: None,
                retrieval: None,
                latency_class: Default::default(),
            },
        )
        .await;
    let mismatch_error = refused("mismatch", mismatch);
    assert!(
        mismatch_error.contains("FOREIGN_READ_QUERY_MISMATCH"),
        "{mismatch_error}"
    );
    let result = served
        .call(
            "r722-two-sources:query",
            medulla_protocol_types::protocol::Method::UnifiedQuery {
                plan: plan.clone(),
                foreign_execution: None,
                foreign_executions: BoundedVec::new(executions.clone()).unwrap(),
                code_map: None,
                spec_report: None,
                retrieval: None,
                latency_class: Default::default(),
            },
        )
        .await;
    let raw = match &result.result {
        Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) => bytes,
        other => panic!("expected joined rows, got {other:?}"),
    };
    let rows: Vec<medulla_types::unified_row::UnifiedRow> = rmp_serde::from_slice(raw).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "shared-1");
    let replay = served
        .call(
            "r722-two-sources:replay",
            medulla_protocol_types::protocol::Method::UnifiedQuery {
                plan,
                foreign_execution: None,
                foreign_executions: BoundedVec::new(executions).unwrap(),
                code_map: None,
                spec_report: None,
                retrieval: None,
                latency_class: Default::default(),
            },
        )
        .await;
    assert!(refused("replay", replay).contains("FOREIGN_READ_REPLAY"));
}

/// One imported pack with one approved, registered source: the receipt and the source name.
#[cfg(feature = "federation-sql")]
struct ApprovedSource {
    receipt: medulla_catalog_types::connector_pack::ConnectorPackSourceApprovalResult,
    source: String,
}

#[cfg(feature = "federation-sql")]
async fn approved_source(served: &Served, connector: &str) -> ApprovedSource {
    bind(served, connector, ADMIN).await;
    let pack = build_pack(
        connector,
        &[
            tool(connector, "a", "A."),
            source_contract(
                connector,
                format!(
                    r#"@prefix ac: <https://knuckles-team.github.io/ontology/access#> .
<data://{connector}/suppliers#access> a ac:AccessContract ;
 ac:classIri "urn:Supplier" ; ac:sourceId "{connector}/suppliers" ;
 ac:entity "suppliers" ; ac:keyField "id" ; ac:accessKind "mcp_tool" ;
 ac:operation "list_suppliers" ; ac:pagination "none" ."#
                ),
            ),
        ],
    );
    let first = imported(served, &pack, None).await;
    let connector_id = medulla_types::contract::ResourceId::new(connector).unwrap();
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let member = store
        .connector_pack_members(TENANT, &connector_id)
        .unwrap()
        .into_iter()
        .find(|member| !member.source_registrations.is_empty())
        .expect("source registration");
    let registration = member.source_registrations[0].clone();
    let contract_digest = member.body_sha256;
    drop(store);
    let receipt = ok(
        "approve source",
        approve_source(
            served,
            connector,
            &registration.contract_iri,
            &registration.resource_uri,
            head_of(&first),
            contract_digest,
        )
        .await,
    );
    let source = registration.source_id.clone();
    let registered: String = ok(
        "register foreign source",
        served
            .call(
                &format!("{connector}:register"),
                medulla_protocol_types::protocol::Method::RegisterForeignSource {
                    name: source.clone(),
                    quota: None,
                    source: medulla_query_types::wire::ForeignSourceSpec::Mcp {
                        server: connector.to_string(),
                        tool: "list_suppliers".to_string(),
                        capabilities: Default::default(),
                    },
                    acceleration: None,
                },
            )
            .await,
    );
    assert_eq!(registered, source);
    ApprovedSource { receipt, source }
}

/// The scope the dedicated source-query issuer requires.
#[cfg(feature = "federation-sql")]
const APPROVE_SCOPE: &str = "source:approve-query";

#[cfg(feature = "federation-sql")]
fn source_query(
    approved: &ApprovedSource,
    expires_in_ms: u64,
) -> medulla_result_types::source_query::SourceQueryRequest {
    use medulla_result_types::source_query::{
        SourceQueryDialect, SourceQueryLimits, SourceQueryParameter, SourceQueryRequest,
        SourceQueryValue,
    };
    SourceQueryRequest {
        schema_version: 1,
        source_ref: approved.source.clone(),
        source_revision: approved.receipt.committed_version.to_string(),
        schema_digest: approved.receipt.contract_digest.to_hex(),
        snapshot_ref: approved.receipt.head.pack_digest.to_hex(),
        dialect: SourceQueryDialect::Postgres,
        sql: "SELECT id FROM suppliers WHERE name = $1".into(),
        parameters: vec![SourceQueryParameter {
            position: 1,
            value: SourceQueryValue::Text("Acme".into()),
        }],
        limits: SourceQueryLimits {
            max_rows: 10,
            max_bytes: 1_048_576,
            chunk_rows: 5,
            chunk_bytes: 65_536,
            queue_depth: 2,
            queue_bytes: 1_048_576,
            deadline_ms: 10_000,
            max_resident_bytes: 8_388_608,
        },
        requested_expires_at_ms: medulla_server_api::txn::now_ms() + expires_in_ms,
        idempotency_key: "body-key".into(),
    }
}

/// Issue one approval through the served native issuer.
#[cfg(feature = "federation-sql")]
async fn issue_source_query(
    served: &Served,
    approved: &ApprovedSource,
    key: &str,
    expires_in_ms: u64,
) -> medulla_result_types::source_query::ApprovedSourceQuery {
    let issued: medulla_work_types::source_query_approval::SourceQueryApprovalIssued = ok(
        "ApproveSourceQuery",
        served
            .call_as(
                ADMIN,
                &[APPROVE_SCOPE],
                key,
                medulla_protocol_types::protocol::Method::ApproveSourceQuery {
                    request: Box::new(
                        medulla_work_types::source_query_approval::ApproveSourceQueryRequest {
                            tenant: TENANT.into(),
                            query: source_query(approved, expires_in_ms),
                            admission: None,
                        },
                    ),
                    source_approval: Box::new(approved.receipt.clone()),
                },
            )
            .await,
    );
    assert_eq!(
        issued.outcome,
        medulla_work_types::control_lease::ControlLeaseIssueOutcome::Issued
    );
    issued
        .approved
        .expect("an issued approval carries the approved query")
}

/// The check of `approved` against the identities it was issued for.
#[cfg(feature = "federation-sql")]
fn source_query_check(
    approved: &medulla_result_types::source_query::ApprovedSourceQuery,
) -> medulla_result_types::source_query::SourceQueryApprovalCheck {
    medulla_result_types::source_query::SourceQueryApprovalCheck {
        approved_query: approved.clone(),
        observed_source_revision: approved.request.source_revision.clone(),
        observed_schema_digest: approved.request.schema_digest.clone(),
        observed_snapshot_ref: approved.request.snapshot_ref.clone(),
        boundary: medulla_result_types::source_query::SourceQueryBoundary::Start,
    }
}

#[cfg(feature = "federation-sql")]
async fn verify_source_query(
    served: &Served,
    approved: &ApprovedSource,
    key: &str,
    check: medulla_result_types::source_query::SourceQueryApprovalCheck,
) -> medulla_protocol_types::protocol::Response {
    served
        .call(
            key,
            medulla_protocol_types::protocol::Method::VerifySourceQueryApproval {
                request: Box::new(check),
                source_approval: Box::new(approved.receipt.clone()),
            },
        )
        .await
}

/// Revoke the approval's lease at `revision`.
#[cfg(feature = "federation-sql")]
async fn revoke_source_query(served: &Served, approval_id: &str, revision: u64, key: &str) {
    let _: serde_json::Value = ok(
        "TransitionControlLease",
        served
            .call(
                key,
                medulla_protocol_types::protocol::Method::TransitionControlLease {
                    request: medulla_work_types::control_lease::TransitionControlLeaseRequest {
                        tenant: TENANT.into(),
                        lease_id: approval_id.to_string(),
                        expected_revision: revision,
                        to: medulla_work_types::control_lease::ControlLeaseTarget::Revoked,
                        decided_by: String::new(),
                        pending_call_consumption: None,
                        idempotency_key: key.to_string(),
                    },
                },
            )
            .await,
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R085.3
#[cfg(feature = "federation-sql")]
#[tokio::test]
async fn changed_or_revoked_approval_refuses() {
    use medulla_result_types::source_query::SourceQueryApprovalValidity;
    let connector = "r085-verify";
    let served = Served::new();
    let _env = served.with_graph_persistence().await;
    let source = approved_source(&served, connector).await;
    let approved = issue_source_query(&served, &source, "r085-verify:issue", 60_000).await;

    // A live approval verifies and the read writes nothing: the lease revision stands.
    let first: SourceQueryApprovalValidity = ok(
        "verify",
        verify_source_query(&served, &source, "r085-verify:ok-1", source_query_check(&approved))
            .await,
    );
    let again: SourceQueryApprovalValidity = ok(
        "verify again",
        verify_source_query(&served, &source, "r085-verify:ok-2", source_query_check(&approved))
            .await,
    );
    assert_eq!(first.approval_id, approved.approval_id);
    assert_eq!(first.query_id, approved.query_id);
    assert_eq!(first.query_digest, approved.query_digest);
    assert_eq!(first.lease_revision, again.lease_revision, "verify writes nothing");
    assert_eq!(
        first.valid_until_ms,
        approved.expires_at_ms.min(approved.hard_expires_at_ms)
    );

    // Changing any request field or expiry refuses SOURCE_QUERY_UNAUTHORIZED.
    let mut sql = approved.clone();
    sql.request.sql = "SELECT id FROM suppliers".into();
    let mut expiry = approved.clone();
    expiry.expires_at_ms += 1;
    for (label, changed) in [("sql", sql), ("expiry", expiry)] {
        let error = refused(
            label,
            verify_source_query(
                &served,
                &source,
                &format!("r085-verify:{label}"),
                source_query_check(&changed),
            )
            .await,
        );
        assert!(error.contains("SOURCE_QUERY_UNAUTHORIZED"), "{label}: {error}");
        assert!(!error.contains("SELECT") && !error.contains("Acme"), "{error}");
    }

    // A lease that was never issued refuses SOURCE_QUERY_UNAUTHORIZED.
    let mut missing = approved.clone();
    missing.approval_id = "source-query-approval:missing".into();
    missing.query_digest = missing.compute_query_digest().unwrap();
    let error = refused(
        "missing lease",
        verify_source_query(&served, &source, "r085-verify:missing", source_query_check(&missing))
            .await,
    );
    assert!(error.contains("SOURCE_QUERY_UNAUTHORIZED"), "{error}");

    // Another principal of the tenant refuses SOURCE_QUERY_UNAUTHORIZED.
    let other = served
        .call_as(
            "pack-served-other",
            &["*"],
            "r085-verify:other",
            medulla_protocol_types::protocol::Method::VerifySourceQueryApproval {
                request: Box::new(source_query_check(&approved)),
                source_approval: Box::new(source.receipt.clone()),
            },
        )
        .await;
    assert!(refused("other principal", other).contains("SOURCE_QUERY_UNAUTHORIZED"));

    // A changed observed source revision refuses SOURCE_QUERY_STALE_SOURCE.
    let mut stale = source_query_check(&approved);
    stale.observed_source_revision = "99".into();
    let error = refused(
        "stale",
        verify_source_query(&served, &source, "r085-verify:stale", stale).await,
    );
    assert!(error.contains("SOURCE_QUERY_STALE_SOURCE"), "{error}");

    // A revoked lease refuses SOURCE_QUERY_APPROVAL_REVOKED.
    revoke_source_query(
        &served,
        &approved.approval_id,
        first.lease_revision,
        "r085-verify:revoke",
    )
    .await;
    let error = refused(
        "revoked",
        verify_source_query(&served, &source, "r085-verify:revoked", source_query_check(&approved))
            .await,
    );
    assert!(error.contains("SOURCE_QUERY_APPROVAL_REVOKED"), "{error}");

    // An expired lease refuses SOURCE_QUERY_APPROVAL_EXPIRED.
    let short = issue_source_query(&served, &source, "r085-verify:issue-short", 1_000).await;
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    let error = refused(
        "expired",
        verify_source_query(&served, &source, "r085-verify:expired", source_query_check(&short))
            .await,
    );
    assert!(error.contains("SOURCE_QUERY_APPROVAL_EXPIRED"), "{error}");
}

// spec: MEDULLA-FEDERATED-QUERY-R085.5
#[cfg(feature = "federation-sql")]
#[tokio::test]
async fn real_dispatch_issues_and_verifies_approval() {
    use medulla_result_types::source_query::SourceQueryApprovalValidity;
    let connector = "r085-dispatch";
    let served = Served::new();
    let _env = served.with_graph_persistence().await;
    let source = approved_source(&served, connector).await;
    // Real authenticated dispatch issues one lease and verifies it.
    let approved = issue_source_query(&served, &source, "r085-dispatch:issue", 60_000).await;
    let validity: SourceQueryApprovalValidity = ok(
        "verify",
        verify_source_query(&served, &source, "r085-dispatch:verify", source_query_check(&approved))
            .await,
    );
    assert_eq!(validity.approval_id, approved.approval_id);
    assert_eq!(validity.lease_revision, 1, "an issued lease is revision 1");
    // No client-side approval authority is accepted: a caller without the issuer scope is
    // refused, and the foreign-source handler has no arm for the issuer.
    let denied = served
        .call_as(
            ADMIN,
            &["kg:read"],
            "r085-dispatch:no-scope",
            medulla_protocol_types::protocol::Method::ApproveSourceQuery {
                request: Box::new(
                    medulla_work_types::source_query_approval::ApproveSourceQueryRequest {
                        tenant: TENANT.into(),
                        query: source_query(&source, 60_000),
                        admission: None,
                    },
                ),
                source_approval: Box::new(source.receipt.clone()),
            },
        )
        .await;
    assert!(denied.error.is_some(), "the issuer scope is required");
    // `ApproveSourceQuery` sent to the foreign-source handler directly is not handled
    // there: `try_handle` hands the method back untouched.
    let core = Arc::new(medulla_graph_core::graph::GraphCore::new());
    let verified =
        medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test_with_scopes(
            ADMIN,
            TENANT,
            &["*"],
        );
    let target = medulla_h_sparql::foreign_source_handlers::ContractWriteTarget {
        core: &core,
        persistence: None,
        graph: "__commons__",
        principal: Some(ADMIN),
        tenant_scope: TENANT,
        verified_context: &verified,
    };
    let call = medulla_server_auth::access::OwnerScopedCall {
        state: &served.state,
        req_id: 1,
        carrier: None,
    };
    let method = medulla_protocol_types::protocol::Method::ApproveSourceQuery {
        request: Box::new(
            medulla_work_types::source_query_approval::ApproveSourceQueryRequest {
                tenant: TENANT.into(),
                query: source_query(&source, 60_000),
                admission: None,
            },
        ),
        source_approval: Box::new(source.receipt.clone()),
    };
    match medulla_h_sparql::foreign_source_handlers::try_handle(call, method, &target).await {
        Err(medulla_protocol_types::protocol::Method::ApproveSourceQuery { .. }) => {}
        Err(other) => panic!("the method came back changed: {other:?}"),
        Ok(response) => panic!(
            "the foreign-source handler answered ApproveSourceQuery: {:?}",
            response.error
        ),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R085.8
#[cfg(feature = "federation-sql")]
#[tokio::test]
async fn approval_to_verification_journey() {
    use medulla_result_types::source_query::SourceQueryApprovalValidity;
    let connector = "r085-journey";
    let served = Served::new();
    let _env = served.with_graph_persistence().await;
    let source = approved_source(&served, connector).await;
    let approved = issue_source_query(&served, &source, "r085-journey:issue", 60_000).await;
    // The approval holds digests that bind the request.
    assert_eq!(approved.query_id, approved.operation_digest);
    assert_eq!(approved.query_digest, approved.compute_query_digest().unwrap());
    let validity: SourceQueryApprovalValidity = ok(
        "verify",
        verify_source_query(&served, &source, "r085-journey:verify", source_query_check(&approved))
            .await,
    );
    assert_eq!(validity.query_digest, approved.query_digest);

    // Another tenant presenting this tenant's approval is refused.
    // The call reaches dispatch as the foreign tenant's verified context and misses the
    // lease, which is keyed by the verified tenant.
    let foreign = served
        .call_verified_as_tenant(
            "tenant-other",
            &["*"],
            medulla_protocol_types::protocol::Method::VerifySourceQueryApproval {
                request: Box::new(source_query_check(&approved)),
                source_approval: Box::new(source.receipt.clone()),
            },
        )
        .await;
    let error = refused("cross-tenant verification", foreign);
    assert!(error.contains("SOURCE_QUERY_UNAUTHORIZED"), "{error}");

    // After revocation verify refuses SOURCE_QUERY_APPROVAL_REVOKED, naming no SQL.
    revoke_source_query(
        &served,
        &approved.approval_id,
        validity.lease_revision,
        "r085-journey:revoke",
    )
    .await;
    let error = refused(
        "revoked",
        verify_source_query(&served, &source, "r085-journey:revoked", source_query_check(&approved))
            .await,
    );
    assert!(error.contains("SOURCE_QUERY_APPROVAL_REVOKED"), "{error}");
    assert!(!error.contains("SELECT") && !error.contains("Acme"), "{error}");
}

// spec: MEDULLA-FEDERATED-QUERY-R722.11
#[tokio::test]
async fn uql_joins_two_approved_sources_in_one_query() {
    use medulla_types::contract::BoundedVec;
    let connector = "r722-uql-two-sources";
    let served = Served::new();
    let _env = served.with_graph_persistence().await;
    bind(&served, connector, ADMIN).await;
    let contract = format!(
        r#"@prefix ac: <https://knuckles-team.github.io/ontology/access#> .
<data://{connector}/first#access> a ac:AccessContract ; ac:classIri "urn:Item" ; ac:sourceId "{connector}/first" ; ac:entity "first" ; ac:keyField "id" ; ac:accessKind "mcp_tool" ; ac:operation "list_first" ; ac:pagination "none" .
<data://{connector}/second#access> a ac:AccessContract ; ac:classIri "urn:Item" ; ac:sourceId "{connector}/second" ; ac:entity "second" ; ac:keyField "id" ; ac:accessKind "mcp_tool" ; ac:operation "list_second" ; ac:pagination "none" ."#
    );
    let pack = build_pack(
        connector,
        &[
            tool(connector, "list_first", "First source."),
            tool(connector, "list_second", "Second source."),
            source_contract(connector, contract),
        ],
    );
    let imported_pack = imported(&served, &pack, None).await;
    let connector_id = medulla_types::contract::ResourceId::new(connector).unwrap();
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let member = store
        .connector_pack_members(TENANT, &connector_id)
        .unwrap()
        .into_iter()
        .find(|member| member.uri == format!("contract://{connector}"))
        .unwrap();
    let registrations = member.source_registrations.clone();
    let contract_digest = member.body_sha256;
    drop(store);
    assert_eq!(registrations.len(), 2);
    let names: Vec<String> = registrations
        .iter()
        .map(|item| item.source_id.clone())
        .collect();
    // One UQL statement names both sources; the join leg follows the first.
    let text = format!("FOREIGN SCAN '{}' |> FOREIGN SCAN '{}' JOIN", names[0], names[1]);
    let plan = medulla_exec::algebra::Plan::new(
        names
            .iter()
            .enumerate()
            .map(|(index, name)| medulla_exec::algebra::Op::ForeignScan {
                source: Box::new(medulla_query_types::wire::ForeignSourceSpec::Named {
                    name: name.clone(),
                }),
                join: index != 0,
            })
            .collect(),
    );
    let plan_digest = Digest256::sha256(&rmp_serde::to_vec_named(&plan).unwrap());
    // The query pin the server computes for this text and parameter map.
    let params = BTreeMap::<String, medulla_query_types::wire::UqlParam>::new();
    let query_digest = Digest256::framed(
        b"eg/foreign-read/uql-query/v1",
        &[text.as_bytes(), &rmp_serde::to_vec_named(&params).unwrap()],
    )
    .unwrap();
    let mut executions = Vec::new();
    for registration in &registrations {
        let approved: medulla_catalog_types::connector_pack::ConnectorPackSourceApprovalResult = ok(
            "approve source",
            approve_source(
                &served,
                connector,
                &registration.contract_iri,
                &registration.resource_uri,
                head_of(&imported_pack),
                contract_digest,
            )
            .await,
        );
        let _: String = ok(
            "register source",
            served
                .call(
                    &format!("r722-uql-two:register:{}", registration.source_id),
                    medulla_protocol_types::protocol::Method::RegisterForeignSource {
                        name: registration.source_id.clone(),
                        quota: None,
                        source: medulla_query_types::wire::ForeignSourceSpec::Mcp {
                            server: connector.to_string(),
                            tool: registration.operation.clone(),
                            capabilities: Default::default(),
                        },
                        acceleration: None,
                    },
                )
                .await,
        );
        let opened: ForeignReadOpenResult = ok(
            "open",
            served
                .call(
                    &format!("r722-uql-two:open:{}", registration.source_id),
                    medulla_protocol_types::protocol::Method::ForeignReadOpen {
                        request: ForeignReadOpenRequest {
                            schema_version: FOREIGN_READ_SCHEMA_VERSION,
                            source_name: registration.source_id.clone(),
                            approval: approved,
                            plan_digest,
                            query_digest,
                            limits: ForeignReadLimits {
                                max_rows: 10,
                                max_bytes: 1024,
                                chunk_rows: 2,
                                chunk_bytes: 512,
                                deadline_ms: 10_000,
                            },
                            submission: None,
                        },
                    },
                )
                .await,
        );
        let _: ForeignReadChunkResult = ok(
            "chunk",
            served
                .call(
                    &format!("r722-uql-two:chunk:{}", registration.source_id),
                    medulla_protocol_types::protocol::Method::ForeignReadChunk {
                        request: ForeignReadChunkRequest {
                            execution: opened.execution,
                            sequence: 0,
                            source_identity: opened.source_identity,
                            rows: vec![ForeignReadRow {
                                id: "shared-1".into(),
                                score: Some(0.5),
                                columns: BTreeMap::new(),
                                provenance: Vec::new(),
                            }],
                        },
                    },
                )
                .await,
        );
        let _: ForeignReadCompleteResult = ok(
            "complete",
            served
                .call(
                    &format!("r722-uql-two:complete:{}", registration.source_id),
                    medulla_protocol_types::protocol::Method::ForeignReadComplete {
                        request: ForeignReadCompleteRequest {
                            execution: opened.execution,
                            sequence: 1,
                        },
                    },
                )
                .await,
        );
        executions.push(opened.execution);
    }
    let uql = |executions: Vec<_>| medulla_protocol_types::protocol::Method::Uql {
        text: text.clone(),
        params: BTreeMap::new(),
        prepare: false,
        foreign_executions: BoundedVec::new(executions).unwrap(),
        budget: None,
    };

    // Tenant B presenting tenant A's execution ids is refused.
    // The call reaches dispatch as the other tenant's verified context; the execution
    // record is owned by tenant A's scope, so the lookup refuses on the owner.
    let other_tenant = served
        .call_verified_as_tenant("tenant-other", &["*"], uql(executions.clone()))
        .await;
    let other_error = refused("another tenant's foreign executions", other_tenant);
    assert!(
        other_error.contains("ACCESS_DENIED: foreign read belongs to another owner"),
        "{other_error}"
    );

    // Two sources from two opened executions join in one served `Method::Uql`.
    let result = served.call("r722-uql-two:query", uql(executions)).await;
    let rows = match ok::<medulla_query_types::wire::UqlResult>("uql join", result) {
        medulla_query_types::wire::UqlResult::Rows { rows, .. } => rows,
        other => panic!("expected joined rows, got {other:?}"),
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "shared-1");
}
