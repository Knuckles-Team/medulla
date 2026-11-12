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

/// Stage `bytes` as an engine-owned body committed at `at_ms`, the state a
/// pack import leaves when it faults after the copy and before the catalog
/// commit: stored, held by the staging holder, referenced by no revision.
#[cfg(feature = "blob")]
fn stage_orphan(
    blob: &medulla_server_blob::blob::BlobCursors,
    bytes: &[u8],
    at_ms: u64,
) -> medulla_server_blob::blob::engine_bodies::StoredEngineBody {
    use medulla_server_blob::blob::engine_bodies::EngineBody;
    use medulla_types::contract::Digest256;
    use sha2::{Digest, Sha256};

    blob.store
        .put_engine_bodies(
            TENANT,
            &[EngineBody {
                sha256: Digest256::from_bytes(Sha256::digest(bytes).into()),
                body: bytes.to_vec(),
            }],
            at_ms,
        )
        .unwrap()
        .remove(0)
}

// spec: MEDULLA-TYPED-PACKS-R002.5
#[cfg(feature = "blob")]
#[tokio::test]
async fn orphan_grace_fault() {
    use medulla_catalog_types::connector_pack::{
        ConnectorPackOp, ConnectorPackReconcileRequest, PackBodyReconcileReport,
    };
    use medulla_server_blob::blob::engine_bodies::read_engine_body;
    use medulla_server_blob::blob::BlobStateExt;
    use medulla_types::contract::Digest256;
    use sha2::{Digest, Sha256};

    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    let kept = tool(CONNECTOR, "kept", "A tool a revision holds.");
    imported(&served, &build_pack(CONNECTOR, &[kept.clone()]), None).await;

    // Two bodies staged and then abandoned by a fault before owner commit:
    // one staged long ago (its grace window has run out), one staged now.
    let old_bytes = b"orphan staged before the grace window".to_vec();
    let new_bytes = b"orphan staged inside the grace window".to_vec();
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let (old, new) = {
        let blob = served.state.read().await.blob_cursors().cloned().unwrap();
        (
            stage_orphan(&blob, &old_bytes, 1),
            stage_orphan(&blob, &new_bytes, now_ms),
        )
    };

    let key = "reconcile:storage-acceptance-grace";
    let report: PackBodyReconcileReport = ok(
        "ReconcileBodies",
        served
            .pack(
                key,
                ConnectorPackOp::ReconcileBodies {
                    request: ConnectorPackReconcileRequest {
                        context: super::context(key),
                    },
                },
            )
            .await,
    );
    assert_eq!(report.orphaned, 2, "both abandoned holders are released");

    // The sweep reclaims only the orphan whose grace window has elapsed.
    let blob = served.state.read().await.blob_cursors().cloned().unwrap();
    let swept = blob.store.sweep().unwrap();
    assert!(
        swept.blobs_reclaimed >= 1,
        "the expired orphan is reclaimed"
    );
    drop(blob);

    // Restart the blob substrate over the same directory: survival is
    // durable, not an in-memory artefact.
    let blob_dir = served.dir.path().join("blob");
    {
        let mut state = served.state.write().await;
        state.set_blob_cursors(None);
        state.set_blob_cursors(Some(medulla_server_ops::api::ingest::cursors_for_test(
            blob_dir.to_str().unwrap(),
        )));
    }
    let blob = served.state.read().await.blob_cursors().cloned().unwrap();
    let digest = |bytes: &[u8]| Digest256::from_bytes(Sha256::digest(bytes).into());
    let expired_read = read_engine_body(
        blob.store.as_ref(),
        TENANT,
        &old.manifest_digest,
        digest(&old_bytes),
        old_bytes.len() as u64,
    );
    assert!(
        expired_read.unwrap_err().starts_with("BODY_MISSING"),
        "the orphan past its grace is gone"
    );
    let survivor = read_engine_body(
        blob.store.as_ref(),
        TENANT,
        &new.manifest_digest,
        digest(&new_bytes),
        new_bytes.len() as u64,
    )
    .unwrap();
    assert_eq!(survivor, new_bytes, "an unexpired orphan survives restart");
    let content: medulla_agent_types::agent_component::AgentComponentContentResult = ok(
        "Content",
        served
            .call(
                "content:storage-acceptance-grace",
                medulla_protocol_types::protocol::Method::AgentComponent {
                    op: medulla_agent_types::agent_component::AgentComponentOp::Content {
                        request:
                            medulla_agent_types::agent_component::AgentComponentContentRequest {
                                tenant_id: TENANT.to_string(),
                                component_id: format!("mcp:{CONNECTOR}/tool/kept"),
                                entry_revision: None,
                            },
                    },
                },
            )
            .await,
    );
    assert_eq!(content.body, kept.body, "a held body survives restart");
}

/// A chunk store that serves a flipped byte from `get_chunk` once armed; every
/// other call is the inner store's.
#[cfg(feature = "blob")]
struct CorruptingStore {
    inner: std::sync::Arc<dyn medulla_server_blob::blob::store::ChunkStore>,
    armed: std::sync::atomic::AtomicBool,
}

#[cfg(feature = "blob")]
impl medulla_server_blob::blob::store::ChunkStore for CorruptingStore {
    fn put_chunk(&self, bytes: &[u8]) -> Result<(String, bool), String> {
        self.inner.put_chunk(bytes)
    }
    fn get_chunk(&self, digest: &str) -> Result<Option<Vec<u8>>, String> {
        let mut chunk = self.inner.get_chunk(digest)?;
        if self.armed.load(std::sync::atomic::Ordering::SeqCst) {
            if let Some(first) = chunk.as_mut().and_then(|bytes| bytes.first_mut()) {
                *first ^= 0xff;
            }
        }
        Ok(chunk)
    }
    fn put_manifest(
        &self,
        blob_digest: &str,
        manifest: &medulla_server_blob::blob::BlobManifest,
    ) -> Result<(), String> {
        self.inner.put_manifest(blob_digest, manifest)
    }
    fn get_manifest(
        &self,
        blob_digest: &str,
    ) -> Result<Option<medulla_server_blob::blob::BlobManifest>, String> {
        self.inner.get_manifest(blob_digest)
    }
    fn incref(&self, blob_digest: &str) -> Result<u64, String> {
        self.inner.incref(blob_digest)
    }
    fn decref(&self, blob_digest: &str) -> Result<u64, String> {
        self.inner.decref(blob_digest)
    }
    fn refcount(&self, blob_digest: &str) -> Result<u64, String> {
        self.inner.refcount(blob_digest)
    }
    fn sweep(&self) -> Result<medulla_server_blob::blob::store::SweepStats, String> {
        self.inner.sweep()
    }
    fn chunk_count(&self) -> Result<u64, String> {
        self.inner.chunk_count()
    }
    fn blob_count(&self) -> Result<u64, String> {
        self.inner.blob_count()
    }
}

// spec: MEDULLA-TYPED-PACKS-R002.6
#[cfg(feature = "blob")]
#[tokio::test]
async fn verified_body_read() {
    use medulla_agent_types::agent_component::{
        AgentComponentContentRequest, AgentComponentContentResult, AgentComponentOp,
    };
    use medulla_protocol_types::protocol::Method;
    use medulla_server_blob::blob::BlobStateExt;

    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    let verified = tool(CONNECTOR, "verified", "A tool whose body is verified.");
    imported(&served, &build_pack(CONNECTOR, &[verified.clone()]), None).await;
    let request = |tenant: &str| Method::AgentComponent {
        op: AgentComponentOp::Content {
            request: AgentComponentContentRequest {
                tenant_id: tenant.to_string(),
                component_id: format!("mcp:{CONNECTOR}/tool/verified"),
                entry_revision: None,
            },
        },
    };

    // The pinned revision returns its exact body.
    let content: AgentComponentContentResult = ok(
        "Content",
        served.call("verified:read", request(TENANT)).await,
    );
    assert_eq!(content.body, verified.body);

    // A tenant other than the verified caller's is refused before any read.
    let denied = refused(
        "Content for another tenant",
        served
            .call("verified:denied", request("tenant-other"))
            .await,
    );
    assert!(denied.starts_with("ACCESS_DENIED"), "{denied}");
    assert!(
        !denied.contains(&String::from_utf8_lossy(&verified.body).to_string()),
        "a denied read carries no body data"
    );

    // One stored chunk comes back corrupted: the read refuses, never partial.
    let inner = served
        .state
        .read()
        .await
        .blob_cursors()
        .cloned()
        .unwrap()
        .store
        .clone();
    let corrupting = std::sync::Arc::new(CorruptingStore {
        inner,
        armed: std::sync::atomic::AtomicBool::new(true),
    });
    served
        .state
        .write()
        .await
        .set_blob_cursors(Some(std::sync::Arc::new(
            medulla_server_blob::blob::BlobCursors::new(corrupting),
        )));
    let corrupt = refused(
        "Content over a corrupt chunk",
        served.call("verified:corrupt", request(TENANT)).await,
    );
    assert!(corrupt.starts_with("BODY_DIGEST_MISMATCH"), "{corrupt}");
}
