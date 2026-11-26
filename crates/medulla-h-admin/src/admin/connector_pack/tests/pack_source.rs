//! R722.2 registration and digest behaviour, proven end to end through the
//! served `ConnectorPack.Import`, `ApproveSource` and foreign-read path: a
//! held mapping serves no rows, an approved one serves rows, an unchanged
//! re-import changes nothing, a changed contract returns the mapping to held,
//! and a publisher cannot import an approval.

use std::collections::BTreeMap;

use medulla_catalog_types::connector_pack::{
    ConnectorPackSourceApprovalResult, PackAnnotations, PackEntryKind, PackImportResult,
    PackSourceApproval,
};
use medulla_catalog_types::foreign_read::{
    ForeignReadChunkRequest, ForeignReadChunkResult, ForeignReadCompleteRequest,
    ForeignReadCompleteResult, ForeignReadLimits, ForeignReadOpenRequest, ForeignReadOpenResult,
    ForeignReadRow, FOREIGN_READ_SCHEMA_VERSION,
};
#[cfg(feature = "redb")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_protocol_types::protocol::{Method, ResultPayload};
use medulla_types::contract::{Digest256, ResourceId};

use super::{
    approve_source, bind, build_pack, head_of, import, imported, ok, refused, tool, Content,
    Served, ADMIN, TENANT,
};

const CONNECTOR: &str = "r722-pack-source";

fn contract_body(operation: &str, extra: &str) -> String {
    format!(
        r#"@prefix ac: <https://knuckles-team.github.io/ontology/access#> .
<data://{CONNECTOR}/suppliers#access> a ac:AccessContract ;
 ac:classIri "urn:Supplier" ; ac:sourceId "{CONNECTOR}/suppliers" ;
 ac:entity "suppliers" ; ac:keyField "id" ; ac:accessKind "mcp_tool" ;
 ac:operation "{operation}" ; ac:pagination "none" {extra}."#
    )
}

fn source_contract(body: String) -> Content {
    Content {
        kind: PackEntryKind::Resource,
        uri: format!("contract://{CONNECTOR}"),
        name: "source-contract".to_string(),
        media_type: "text/turtle",
        body: body.into_bytes(),
        input_schema: None,
        annotations: PackAnnotations::default(),
        references: Vec::new(),
    }
}

/// The current source member: its registration and the body digest the
/// engine recorded for it.
async fn current_source(
    served: &Served,
) -> (
    medulla_catalog_types::connector_pack::PackSourceRegistration,
    Digest256,
) {
    let connector = ResourceId::new(CONNECTOR).unwrap();
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let member = store
        .connector_pack_members(TENANT, &connector)
        .unwrap()
        .into_iter()
        .find(|member| !member.source_registrations.is_empty())
        .expect("the contract member carries one source registration");
    (member.source_registrations[0].clone(), member.body_sha256)
}

/// One served foreign read over the named source: open with the approval
/// receipt, deliver one row, complete, then run the served `UnifiedQuery`.
/// Returns the refusal when the open is refused, so a held mapping is
/// observed to serve no rows at the served door.
async fn read_rows(
    served: &Served,
    tag: &str,
    source_name: &str,
    approval: &ConnectorPackSourceApprovalResult,
) -> Result<Vec<(String, Option<f32>)>, String> {
    let plan =
        medulla_query_types::wire::Plan::new(vec![medulla_query_types::wire::Op::ForeignScan {
            source: Box::new(medulla_query_types::wire::ForeignSourceSpec::Named {
                name: source_name.to_string(),
            }),
            join: false,
        }]);
    let plan_digest = Digest256::sha256(&rmp_serde::to_vec_named(&plan).unwrap());
    let open = served
        .call(
            &format!("r722-pack-source:{tag}:open"),
            Method::ForeignReadOpen {
                request: ForeignReadOpenRequest {
                    schema_version: FOREIGN_READ_SCHEMA_VERSION,
                    submission: None,
                    source_name: source_name.to_string(),
                    approval: approval.clone(),
                    plan_digest,
                    query_digest: plan_digest,
                    limits: ForeignReadLimits {
                        max_rows: 10,
                        max_bytes: 1024,
                        chunk_rows: 2,
                        chunk_bytes: 512,
                        deadline_ms: 10_000,
                    },
                },
            },
        )
        .await;
    if let Some(error) = open.error {
        return Err(error);
    }
    let opened: ForeignReadOpenResult = ok("open", open);
    let chunk: ForeignReadChunkResult = ok(
        "chunk",
        served
            .call(
                &format!("r722-pack-source:{tag}:chunk"),
                Method::ForeignReadChunk {
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
                            provenance: vec![format!("{CONNECTOR}:supplier-1")],
                        }],
                    },
                },
            )
            .await,
    );
    assert_eq!(chunk.accepted_rows, 1);
    let _: ForeignReadCompleteResult = ok(
        "complete",
        served
            .call(
                &format!("r722-pack-source:{tag}:complete"),
                Method::ForeignReadComplete {
                    request: ForeignReadCompleteRequest {
                        execution: opened.execution,
                        sequence: 1,
                    },
                },
            )
            .await,
    );
    let response = served
        .call(
            &format!("r722-pack-source:{tag}:query"),
            Method::UnifiedQuery {
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
    if let Some(error) = response.error {
        return Err(error);
    }
    match &response.result {
        Some(ResultPayload::Raw(bytes)) => {
            let rows: Vec<medulla_types::unified_row::UnifiedRow> =
                rmp_serde::from_slice(bytes).unwrap();
            Ok(rows
                .iter()
                .map(medulla_types::unified_row::UnifiedRow::pair)
                .collect())
        }
        other => panic!("expected foreign rows, got {other:?}"),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R722.2
#[tokio::test]
async fn held_serves_none_approved_serves_rows_and_changed_contract_returns_to_held() {
    let served = Served::new();
    let _env = served.with_graph_persistence().await;
    bind(&served, CONNECTOR, ADMIN).await;

    // A publisher cannot import an approval: the contract is refused whole.
    let self_approved = build_pack(
        CONNECTOR,
        &[
            tool(CONNECTOR, "a", "A."),
            source_contract(contract_body(
                "list_suppliers",
                "; ac:approval \"approved\" ",
            )),
        ],
    );
    let response = import(&served, &self_approved, None).await;
    let refusal = match response.error.clone() {
        Some(error) => error,
        None => match ok::<PackImportResult>("self-approved import", response) {
            PackImportResult::Rejected { violations, .. } => format!("{violations:?}"),
            other => panic!("a publisher-approved contract must not import: {other:?}"),
        },
    };
    assert!(refusal.contains("CONTRACT_APPROVAL"), "{refusal}");

    // Import registers the declared resource as a held source and mapping.
    let body = contract_body("list_suppliers", "");
    let pack = build_pack(
        CONNECTOR,
        &[tool(CONNECTOR, "a", "A."), source_contract(body.clone())],
    );
    let first = imported(&served, &pack, None).await;
    let (registration, digest) = current_source(&served).await;
    assert_eq!(registration.approval, PackSourceApproval::Held);
    // The digest is the body_sha256 of the authoritative member body.
    assert_eq!(digest, Digest256::sha256(body.as_bytes()));

    let source_name = registration.source_id.clone();
    let registered: String = ok(
        "register foreign source",
        served
            .call(
                "r722-pack-source:register",
                Method::RegisterForeignSource {
                    name: source_name.clone(),
                    source: medulla_query_types::wire::ForeignSourceSpec::Mcp {
                        server: CONNECTOR.to_string(),
                        tool: "list_suppliers".to_string(),
                        capabilities: Default::default(),
                    },
                    quota: None,
                    acceleration: None,
                },
            )
            .await,
    );
    assert_eq!(registered, source_name);

    let approved: ConnectorPackSourceApprovalResult = ok(
        "approve source",
        approve_source(
            &served,
            CONNECTOR,
            &registration.contract_iri,
            &registration.resource_uri,
            head_of(&first),
            digest,
        )
        .await,
    );
    assert_eq!(approved.registration.approval, PackSourceApproval::Approved);
    assert_eq!(approved.contract_digest, digest);

    // A held mapping serves none.
    let mut held = approved.clone();
    held.registration.approval = PackSourceApproval::Held;
    let held_refusal = read_rows(&served, "held", &source_name, &held)
        .await
        .expect_err("a held mapping serves no rows");
    assert!(
        held_refusal.contains("SOURCE_APPROVAL_NOT_ADMITTED"),
        "{held_refusal}"
    );

    // An approved mapping serves rows.
    let rows = read_rows(&served, "approved", &source_name, &approved)
        .await
        .expect("an approved mapping serves rows");
    assert_eq!(rows, vec![("supplier-1".to_string(), Some(0.5))]);

    // A re-import with an unchanged contract digest changes nothing.
    match ok::<PackImportResult>(
        "unchanged re-import",
        import(&served, &pack, Some(head_of(&first))).await,
    ) {
        PackImportResult::Unchanged {
            pack_digest,
            binding_revision,
        } => {
            assert_eq!(pack_digest, first.pack_digest);
            assert_eq!(binding_revision, first.binding_revision);
        }
        other => panic!("an unchanged re-import must change nothing: {other:?}"),
    }
    let (unchanged, unchanged_digest) = current_source(&served).await;
    assert_eq!(unchanged.approval, PackSourceApproval::Approved);
    assert_eq!(unchanged_digest, digest);
    let rows = read_rows(&served, "unchanged", &source_name, &approved)
        .await
        .expect("the approval survives an unchanged re-import");
    assert_eq!(rows, vec![("supplier-1".to_string(), Some(0.5))]);

    // A changed contract replaces the mapping and returns it to held.
    let changed_body = contract_body("list_customers", "");
    let changed = build_pack(
        CONNECTOR,
        &[
            tool(CONNECTOR, "a", "A."),
            source_contract(changed_body.clone()),
        ],
    );
    let second = imported(&served, &changed, Some(head_of(&first))).await;
    let (replaced, replaced_digest) = current_source(&served).await;
    assert_eq!(replaced.approval, PackSourceApproval::Held);
    assert_eq!(replaced.operation, "list_customers");
    assert_eq!(replaced_digest, Digest256::sha256(changed_body.as_bytes()));
    assert_ne!(replaced_digest, digest);

    // The earlier approval no longer serves: the held mapping serves none.
    let stale = read_rows(&served, "stale", &source_name, &approved)
        .await
        .expect_err("a held replacement serves no rows");
    assert!(
        stale.contains("PACK_HEAD_CONFLICT") || stale.contains("SOURCE_CONTRACT_MISMATCH"),
        "{stale}"
    );
    // Re-approval must name the new body digest, never the old one.
    let mismatch = refused(
        "approval naming the old digest",
        approve_source(
            &served,
            CONNECTOR,
            &replaced.contract_iri,
            &replaced.resource_uri,
            head_of(&second),
            digest,
        )
        .await,
    );
    assert!(mismatch.contains("SOURCE_CONTRACT_MISMATCH"), "{mismatch}");
}
