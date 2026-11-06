//! Bounded R073 storage acceptance through signed, served pack imports.

#[cfg(feature = "redb")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use std::collections::BTreeSet;

use medulla_agent_types::agent_component::AgentComponentOutboxEvent;
use medulla_catalog_types::{
    connector_pack::PackImportResult, connector_pack::CONNECTOR_PACK_IMPORT_TOPIC,
};
use medulla_mutation_types::mutation_scope::MutationOutboxRecord;
use medulla_types::contract::ResourceId;

use super::{
    bind, build_pack, head_of, import, import_uploaded, imported, ok, refused, tool, Served, ADMIN,
    TENANT,
};

mod receipt;

const CONNECTOR: &str = "storage-acceptance";

async fn snapshot(served: &Served) -> serde_json::Value {
    served
        .state
        .write()
        .await
        .ensure_agent_library()
        .unwrap()
        .connector_pack_refusal_snapshot(TENANT)
}

// spec: MEDULLA-DECISION-ENGINE-R073.1
#[tokio::test]
async fn multi_component_import_emits_one_durable_event_per_component() {
    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    let pack = build_pack(
        CONNECTOR,
        &[tool(CONNECTOR, "a", "A."), tool(CONNECTOR, "b", "B.")],
    );
    let receipt = imported(&served, &pack, None).await;
    let before = snapshot(&served).await;
    let rows: Vec<MutationOutboxRecord> = serde_json::from_value(before["outbox"].clone()).unwrap();
    let rows: Vec<_> = rows
        .iter()
        .filter(|row| row.batch_id == receipt.batch_id)
        .collect();
    assert_eq!(rows.len(), 4, "server, two tools, and the pack import");
    let mut component_ids = BTreeSet::new();
    for (ordinal, row) in rows[..3].iter().enumerate() {
        assert_eq!(row.ordinal, ordinal as u32);
        assert_eq!(row.intent.topic, "medulla.agent-component.revision.v1");
        let event: AgentComponentOutboxEvent =
            medulla_storage::decode_ledger_record(&row.intent.payload).unwrap();
        event.validate().unwrap();
        assert_eq!(event.component.tenant_id, TENANT);
        assert_eq!(event.component.entry_revision, 1);
        assert!(component_ids.insert(event.component.component_id.clone()));
        assert_eq!(
            row.intent.key,
            format!("{TENANT}:{}:1", event.component.component_id)
        );
    }
    assert!(component_ids.contains(&format!("mcp:{CONNECTOR}/tool/a")));
    assert!(component_ids.contains(&format!("mcp:{CONNECTOR}/tool/b")));
    assert_eq!(rows[3].ordinal, 3);
    assert_eq!(rows[3].intent.topic, CONNECTOR_PACK_IMPORT_TOPIC);
    assert_eq!(rows[3].intent.headers["record_id"], receipt.record_id);
    let repeated: PackImportResult = ok(
        "unchanged import",
        import(&served, &pack, Some(head_of(&receipt))).await,
    );
    assert!(matches!(repeated, PackImportResult::Unchanged { .. }));
    assert_eq!(snapshot(&served).await, before);
}

// spec: MEDULLA-DECISION-ENGINE-R073.2
#[tokio::test]
async fn competing_imports_have_one_head_winner_without_loser_effects() {
    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    let initial = imported(
        &served,
        &build_pack(CONNECTOR, &[tool(CONNECTOR, "a", "Initial.")]),
        None,
    )
    .await;
    let left = build_pack(CONNECTOR, &[tool(CONNECTOR, "a", "Left.")]);
    let right = build_pack(CONNECTOR, &[tool(CONNECTOR, "a", "Right.")]);
    // Prepare both inputs before racing only the pack head compare-and-set.
    let mut left_index = left.0.clone();
    left_index.archive.blob_digest = served.upload(&left.1).await;
    let mut right_index = right.0.clone();
    right_index.archive.blob_digest = served.upload(&right.1).await;
    let (left_result, right_result) = tokio::join!(
        import_uploaded(&served, left_index, Some(head_of(&initial))),
        import_uploaded(&served, right_index, Some(head_of(&initial)))
    );
    assert_ne!(left_result.error.is_some(), right_result.error.is_some());
    let (winner, loser, loser_pack) = if left_result.error.is_none() {
        (left_result, right_result, &right)
    } else {
        (right_result, left_result, &left)
    };
    assert!(refused("competing head", loser).contains("PACK_HEAD_CONFLICT"));
    let winner: PackImportResult = ok("winning import", winner);
    let PackImportResult::Imported { receipt } = winner else {
        panic!("one changed pack must commit");
    };
    assert_eq!(receipt.binding_revision, initial.binding_revision + 1);
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let status = store
        .connector_pack_status(TENANT, &ResourceId::new(CONNECTOR).unwrap())
        .unwrap();
    assert_eq!(status.head.unwrap().pack_digest, receipt.pack_digest);
    let before = snapshot(&served).await;
    assert_eq!(
        before["CONNECTOR_PACK_IMPORTS"].as_array().unwrap().len(),
        2
    );
    assert!(refused(
        "stale-head retry",
        import(&served, loser_pack, Some(head_of(&initial))).await
    )
    .contains("PACK_HEAD_CONFLICT"));
    assert_eq!(snapshot(&served).await, before);
}

// spec: MEDULLA-TYPED-PACKS-R002.1
#[tokio::test]
async fn body_staging() {
    use medulla_server_blob::blob::engine_bodies::{read_engine_body, EngineBody};
    use medulla_server_blob::blob::BlobStateExt;
    use medulla_types::contract::Digest256;
    use sha2::{Digest, Sha256};

    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    let blob = served.state.read().await.blob_cursors().cloned().unwrap();
    let bytes = b"a staged connector body".to_vec();
    let address = Digest256::from_bytes(Sha256::digest(&bytes).into());
    // A declared address that differs from the bytes refuses before staging.
    let wrong = Digest256::from_bytes(Sha256::digest(b"other bytes").into());
    let error = blob
        .store
        .put_engine_bodies(
            TENANT,
            &[EngineBody {
                sha256: wrong,
                body: bytes.clone(),
            }],
            1,
        )
        .unwrap_err();
    assert!(error.starts_with("BODY_DIGEST_MISMATCH"), "{error}");
    // Verified bytes stage under their own SHA-256 address.
    let stored = blob
        .store
        .put_engine_bodies(
            TENANT,
            &[EngineBody {
                sha256: address,
                body: bytes.clone(),
            }],
            1,
        )
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].sha256, address);
    assert_eq!(
        stored[0].holder_id,
        format!("pack:{TENANT}:{}", address.to_hex())
    );
    let read = read_engine_body(
        blob.store.as_ref(),
        TENANT,
        &stored[0].manifest_digest,
        address,
        bytes.len() as u64,
    )
    .unwrap();
    assert_eq!(read, bytes);
    let error = read_engine_body(
        blob.store.as_ref(),
        TENANT,
        &stored[0].manifest_digest,
        wrong,
        bytes.len() as u64,
    )
    .unwrap_err();
    assert!(error.starts_with("BODY_DIGEST_MISMATCH"), "{error}");
    // Staging alone exposes no component: the served catalog has no entry.
    let error = refused(
        "Content of a staged-only body",
        served
            .call(
                "content:storage-acceptance:staged",
                medulla_protocol_types::protocol::Method::AgentComponent {
                    op: medulla_agent_types::agent_component::AgentComponentOp::Content {
                        request:
                            medulla_agent_types::agent_component::AgentComponentContentRequest {
                                tenant_id: TENANT.to_string(),
                                component_id: format!("mcp:{CONNECTOR}/tool/staged"),
                                entry_revision: None,
                            },
                    },
                },
            )
            .await,
    );
    assert!(!error.is_empty());
}

// spec: MEDULLA-TYPED-PACKS-R002.6
#[tokio::test]
async fn verified_body_read() {
    use medulla_agent_types::agent_component::{
        AgentComponentContentRequest, AgentComponentContentResult, AgentComponentOp,
    };
    use medulla_protocol_types::protocol::Method;
    use medulla_server_blob::blob::engine_bodies::EngineBody;
    use medulla_server_blob::blob::BlobStateExt;
    use medulla_types::contract::Digest256;
    use sha2::{Digest, Sha256};

    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    let entry = tool(CONNECTOR, "verified", "A verified tool.");
    imported(&served, &build_pack(CONNECTOR, &[entry.clone()]), None).await;
    let content = |tenant: &str| Method::AgentComponent {
        op: AgentComponentOp::Content {
            request: AgentComponentContentRequest {
                tenant_id: tenant.to_string(),
                component_id: format!("mcp:{CONNECTOR}/tool/verified"),
                entry_revision: None,
            },
        },
    };
    // The pinned revision's exact body reads back through the served import.
    let read: AgentComponentContentResult = ok(
        "Content",
        served
            .call("content:storage-acceptance:verified", content(TENANT))
            .await,
    );
    assert_eq!(read.body, entry.body);
    // A tenant other than the verified one refuses before any body read.
    let denied = refused(
        "Content of a foreign tenant",
        served
            .call(
                "content:storage-acceptance:foreign",
                content("tenant-other"),
            )
            .await,
    );
    assert!(denied.contains("ACCESS_DENIED"), "{denied}");
    // Corrupt one committed chunk of the stored body (same length, one byte off).
    let blob = served.state.read().await.blob_cursors().cloned().unwrap();
    let address = Digest256::from_bytes(Sha256::digest(&entry.body).into());
    let stored = blob
        .store
        .put_engine_bodies(
            TENANT,
            &[EngineBody {
                sha256: address,
                body: entry.body.clone(),
            }],
            1,
        )
        .unwrap();
    let manifest = blob
        .store
        .get_manifest(&stored[0].manifest_digest)
        .unwrap()
        .unwrap();
    let chunk = blob.store.get_chunk(&manifest.chunks[0]).unwrap().unwrap();
    let mut corrupt = chunk.clone();
    corrupt[0] ^= 0xff;
    blob.store
        .corrupt_chunk_for_test(&manifest.chunks[0], &corrupt)
        .unwrap();
    // The verified read refuses with the registered code and returns no bytes.
    let response = served
        .call("content:storage-acceptance:corrupt", content(TENANT))
        .await;
    assert!(response.result.is_none(), "a corrupt body returns no data");
    let refusal = refused("Content of a corrupt body", response);
    assert!(refusal.contains("BODY_DIGEST_MISMATCH"), "{refusal}");
}
