//! PB3 (ruling D7): the admin binding decides who may import, retirement is
//! permanent, and the reconciler releases only what nothing holds.

use medulla_catalog_types::connector_pack::{
    ConnectorPackOp, ConnectorPackReconcileRequest, ConnectorPackRetireRequest,
    ConnectorPackUnbindRequest, PackBodyReconcileReport, PackImportResult, PackRetireResult,
    PackViolationCode,
};
#[cfg(feature = "redb")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_server_blob::blob::BlobStateExt;
use medulla_types::contract::{BoundedVec, Digest256, ResourceId};
use sha2::{Digest, Sha256};

use super::{
    bind, build_pack, context, head_of, import, imported, ok, persistence_id, refused, tool,
    Served, ADMIN, TENANT,
};

const CONNECTOR: &str = "pack-admin";

async fn catalog_status(
    served: &Served,
    connector: &str,
) -> medulla_catalog_types::connector_pack::ConnectorPackStatus {
    served
        .state
        .write()
        .await
        .ensure_agent_library()
        .unwrap()
        .connector_pack_status(TENANT, &ResourceId::new(connector).unwrap())
        .unwrap()
}

fn rejected_codes(result: PackImportResult) -> Vec<PackViolationCode> {
    match result {
        PackImportResult::Rejected { violations, .. } => {
            violations.iter().map(|violation| violation.code).collect()
        }
        other => panic!("expected a rejection, got {other:?}"),
    }
}

/// R015: an environment-configured importer is honored only as the initial
/// bootstrap default, before any administrator binds one; once an admin
/// `Bind` names a different importer, the binding governs even though the
/// environment value is still configured and still names the principal that
/// imported successfully a moment ago.
// spec: MEDULLA-TYPED-PACKS-R015
#[tokio::test]
async fn an_environment_importer_is_a_bootstrap_default_superseded_by_an_admin_bind() {
    const ENV_VAR: &str = "MEDULLA_CONNECTOR_PACK_IMPORTERS";
    const ENV_CONNECTOR: &str = "pack-admin-env-bootstrap";

    struct RestoreEnv(Option<std::ffi::OsString>);
    impl Drop for RestoreEnv {
        fn drop(&mut self) {
            match self.0.take() {
                Some(previous) => std::env::set_var(ENV_VAR, previous),
                None => std::env::remove_var(ENV_VAR),
            }
        }
    }

    let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
    let _restore = RestoreEnv(std::env::var_os(ENV_VAR));
    std::env::set_var(
        ENV_VAR,
        format!("{ENV_CONNECTOR}={}", super::persistence_id(ADMIN)),
    );

    let served = Served::new();
    let pack = build_pack(ENV_CONNECTOR, &[tool(ENV_CONNECTOR, "a", "Tool a.")]);

    // No admin Bind yet for this connector: the environment-configured
    // importer is honored as the bootstrap default, so the admin principal
    // it names may import.
    imported(&served, &pack, None).await;

    // An administrator now binds a DIFFERENT importer. From here the admin
    // binding governs pack admission; the environment value stays
    // configured (still naming the admin principal) but no longer applies.
    bind(&served, ENV_CONNECTOR, "someone-else").await;
    assert_eq!(
        rejected_codes(ok("Import", import(&served, &pack, None).await)),
        [PackViolationCode::ImporterMismatch],
        "an admin Bind supersedes the environment bootstrap default"
    );
}

#[tokio::test]
async fn only_the_bound_importer_may_import() {
    let served = Served::new();
    let pack = build_pack(CONNECTOR, &[tool(CONNECTOR, "a", "Tool a.")]);
    // No binding and no bootstrap importer: nobody may import.
    assert_eq!(
        rejected_codes(ok("Import", import(&served, &pack, None).await)),
        [PackViolationCode::ImporterMismatch]
    );
    bind(&served, CONNECTOR, "someone-else").await;
    assert_eq!(
        rejected_codes(ok("Import", import(&served, &pack, None).await)),
        [PackViolationCode::ImporterMismatch]
    );
    bind(&served, CONNECTOR, ADMIN).await;
    imported(&served, &pack, None).await;
    let key = "unbind:pack-admin";
    let _: serde_json::Value = ok(
        "Unbind",
        served
            .pack(
                key,
                ConnectorPackOp::Unbind {
                    request: ConnectorPackUnbindRequest {
                        context: context(key),
                        connector: ResourceId::new(CONNECTOR).unwrap(),
                    },
                },
            )
            .await,
    );
    let error = refused(
        "second Unbind",
        served
            .pack(
                "unbind:pack-admin:again",
                ConnectorPackOp::Unbind {
                    request: ConnectorPackUnbindRequest {
                        context: context("unbind:pack-admin:again"),
                        connector: ResourceId::new(CONNECTOR).unwrap(),
                    },
                },
            )
            .await,
    );
    assert_eq!(error, "CONNECTOR_PACK_UNBOUND");
}

#[tokio::test]
async fn a_retired_entry_can_never_return() {
    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    let tools: Vec<_> = ["a", "b", "c"]
        .into_iter()
        .map(|name| tool(CONNECTOR, name, "A tool."))
        .collect();
    let first = imported(&served, &build_pack(CONNECTOR, &tools), None).await;
    let key = "retire:pack-admin:c";
    let retired: PackRetireResult = ok(
        "Retire",
        served
            .pack(
                key,
                ConnectorPackOp::Retire {
                    request: ConnectorPackRetireRequest {
                        context: context(key),
                        connector: ResourceId::new(CONNECTOR).unwrap(),
                        uris: BoundedVec::new(vec![tools[2].uri.clone()]).unwrap(),
                    },
                },
            )
            .await,
    );
    assert_eq!(retired.retired.len(), 1);
    // A new release that serves the retired entry again (plus a new tool, so
    // it is not simply the unchanged head).
    let mut returning = tools.clone();
    returning.push(tool(CONNECTOR, "d", "A new tool."));
    assert_eq!(
        rejected_codes(ok(
            "Import",
            import(
                &served,
                &build_pack(CONNECTOR, &returning),
                Some(head_of(&first))
            )
            .await
        )),
        [PackViolationCode::RetiredEntryReturned]
    );
}

#[tokio::test]
async fn the_reconciler_releases_an_orphan_body_and_keeps_every_held_one() {
    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    let pack = build_pack(CONNECTOR, &[tool(CONNECTOR, "a", "Tool a.")]);
    imported(&served, &pack, None).await;
    // A body copied by an import whose catalog commit then failed: stored
    // engine-owned, held by nothing the catalog knows about.
    let orphan = b"an orphaned connector body".to_vec();
    let blob = served.state.read().await.blob_cursors().cloned().unwrap();
    blob.store
        .put_engine_bodies(
            TENANT,
            &[medulla_server_blob::blob::engine_bodies::EngineBody {
                sha256: Digest256::from_bytes(Sha256::digest(&orphan).into()),
                body: orphan,
            }],
            1,
        )
        .unwrap();
    let key = "reconcile:pack-admin";
    let report: PackBodyReconcileReport = ok(
        "ReconcileBodies",
        served
            .pack(
                key,
                ConnectorPackOp::ReconcileBodies {
                    request: ConnectorPackReconcileRequest {
                        context: context(key),
                    },
                },
            )
            .await,
    );
    assert_eq!(report.orphaned, 1, "exactly the orphan is released");
    assert_eq!(report.scanned, 2, "the server and the tool body stay held");
    let content: medulla_agent_types::agent_component::AgentComponentContentResult = ok(
        "Content",
        served
            .call(
                "content:pack-admin:a",
                medulla_protocol_types::protocol::Method::AgentComponent {
                    op: medulla_agent_types::agent_component::AgentComponentOp::Content {
                        request:
                            medulla_agent_types::agent_component::AgentComponentContentRequest {
                                tenant_id: TENANT.to_string(),
                                component_id: "mcp:pack-admin/tool/a".to_string(),
                                entry_revision: None,
                            },
                    },
                },
            )
            .await,
    );
    assert_eq!(
        content.body,
        tool(CONNECTOR, "a", "Tool a.").body,
        "a held body still reads back after the sweep"
    );
}

// spec: MEDULLA-TYPED-PACKS-R018
// spec: MEDULLA-TYPED-PACKS-R002
// spec: MEDULLA-TYPED-PACKS-R002.2
// spec: MEDULLA-TYPED-PACKS-R002.3
// spec: MEDULLA-TYPED-PACKS-R002.4
#[tokio::test]
async fn holder_refcounts_transition_across_unchanged_revised_and_withdrawn_entries() {
    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    let a = tool(CONNECTOR, "a", "Tool a.");
    let b = tool(CONNECTOR, "b", "Tool b.");
    let c = tool(CONNECTOR, "c", "Tool c.");
    let first = imported(
        &served,
        &build_pack(CONNECTOR, &[a.clone(), b.clone(), c.clone()]),
        None,
    )
    .await;
    // Next release: `a` unchanged, `b` revised (new body, new holder), `c`
    // withdrawn (dropped from the pack, its historical holder stays live).
    let b_revised = tool(CONNECTOR, "b", "Tool b, revised.");
    let second = imported(
        &served,
        &build_pack(CONNECTOR, &[a.clone(), b_revised.clone()]),
        Some(head_of(&first)),
    )
    .await;
    assert!(
        second.counts.unchanged >= 1 && second.counts.revised >= 1 && second.counts.withdrawn >= 1,
        "unchanged/revised/withdrawn dispositions all land in one commit: {:?}",
        second.counts
    );
    // Retained revisions are still pinned: neither the old `b` body nor
    // withdrawn `c` is an orphan. The sweep must preserve their exact bytes.
    let key = "reconcile:pack-admin-transitions";
    let report: PackBodyReconcileReport = ok(
        "ReconcileBodies",
        served
            .pack(
                key,
                ConnectorPackOp::ReconcileBodies {
                    request: ConnectorPackReconcileRequest {
                        context: context(key),
                    },
                },
            )
            .await,
    );
    assert_eq!(
        report.scanned, 5,
        "all distinct revision bodies remain held"
    );
    assert_eq!(report.orphaned, 0, "historical bodies are not orphans");
    assert_eq!(
        report.released, 0,
        "no historical reference may be released"
    );
    for (name, revision, expected) in [
        ("a", Some(1), &a.body),
        ("b", Some(1), &b.body),
        ("c", Some(1), &c.body),
        ("b", None, &b_revised.body),
    ] {
        let content: medulla_agent_types::agent_component::AgentComponentContentResult = ok(
            "Content",
            served
                .call(
                    &format!("content:pack-admin-transitions:{name}:{revision:?}"),
                    medulla_protocol_types::protocol::Method::AgentComponent {
                        op: medulla_agent_types::agent_component::AgentComponentOp::Content {
                            request:
                                medulla_agent_types::agent_component::AgentComponentContentRequest {
                                    tenant_id: TENANT.to_string(),
                                    component_id: format!("mcp:pack-admin/tool/{name}"),
                                    entry_revision: revision,
                                },
                        },
                    },
                )
                .await,
        );
        assert_eq!(&content.body, expected, "retained body {name}:{revision:?}");
    }
    // A repeated identical import changes no holder and no refcount.
    let repeated = import(
        &served,
        &build_pack(CONNECTOR, &[a.clone(), b_revised.clone()]),
        Some(head_of(&second)),
    )
    .await;
    assert!(
        matches!(
            ok::<PackImportResult>("repeated import", repeated),
            PackImportResult::Unchanged { .. }
        ),
        "an identical re-import is unchanged"
    );
    let key = "reconcile:pack-admin-transitions-repeat";
    let again: PackBodyReconcileReport = ok(
        "ReconcileBodies",
        served
            .pack(
                key,
                ConnectorPackOp::ReconcileBodies {
                    request: ConnectorPackReconcileRequest {
                        context: context(key),
                    },
                },
            )
            .await,
    );
    assert_eq!(again.scanned, report.scanned, "holder set is unchanged");
    assert_eq!(again.orphaned, 0);
    assert_eq!(again.released, 0);
}

// spec: MEDULLA-TYPED-PACKS-R007
#[tokio::test]
async fn bind_unbind_and_retire_require_admin_connector_pack_and_the_head_survives_a_retired_reimport(
) {
    let served = Served::new();
    let connector = "pack-admin-scoped";
    let key = format!("bind:{connector}:scoped");
    let unscoped = served
        .call_as(
            "no-admin-scope-caller",
            &["connector:catalog-attest"],
            &key,
            medulla_protocol_types::protocol::Method::ConnectorPack {
                op: Box::new(ConnectorPackOp::Bind {
                    request: medulla_catalog_types::connector_pack::ConnectorPackBindRequest {
                        context: context(&key),
                        connector: ResourceId::new(connector).unwrap(),
                        importer: persistence_id(ADMIN),
                    },
                }),
            },
        )
        .await;
    assert!(
        unscoped.error.is_some(),
        "Bind must be refused without admin:connector-pack"
    );
    bind(&served, connector, ADMIN).await;
    let tools: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|name| tool(connector, name, "A tool."))
        .collect();
    let first = imported(&served, &build_pack(connector, &tools), None).await;
    let retire_key = format!("retire:{connector}:b");
    let retired: PackRetireResult = ok(
        "Retire",
        served
            .pack(
                &retire_key,
                ConnectorPackOp::Retire {
                    request: ConnectorPackRetireRequest {
                        context: context(&retire_key),
                        connector: ResourceId::new(connector).unwrap(),
                        uris: BoundedVec::new(vec![tools[1].uri.clone()]).unwrap(),
                    },
                },
            )
            .await,
    );
    assert_eq!(retired.retired.len(), 1);
    let before = catalog_status(&served, connector).await;
    let mut returning = tools.clone();
    returning.push(tool(connector, "d", "A new tool."));
    assert_eq!(
        rejected_codes(ok(
            "Import",
            import(
                &served,
                &build_pack(connector, &returning),
                Some(head_of(&first)),
            )
            .await,
        )),
        [PackViolationCode::RetiredEntryReturned],
    );
    let after = catalog_status(&served, connector).await;
    assert_eq!(
        before, after,
        "the rejected reimport must not mutate the head"
    );
}

/// Owner rows plus exact body reads and liveness of every admitted revision.
async fn refusal_snapshot(served: &Served, connector: &str) -> serde_json::Value {
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let blob = served.state.read().await.blob_cursors().cloned().unwrap();
    let mut bodies = Vec::new();
    for member in store
        .connector_pack_members(TENANT, &ResourceId::new(connector).unwrap())
        .unwrap()
    {
        let entry = store
            .current_component(TENANT, &member.component_id)
            .unwrap()
            .unwrap();
        for revision in 1..=entry.entry_revision {
            let pointer = store
                .connector_pack_content_pointer(TENANT, &member.component_id, Some(revision))
                .unwrap();
            let bytes = medulla_server_blob::blob::engine_bodies::read_engine_body(
                blob.store.as_ref(),
                TENANT,
                &pointer.engine_manifest_digest,
                pointer.body_sha256,
                pointer.length,
            )
            .unwrap();
            let holder = medulla_server_blob::blob::engine_bodies::engine_body_holder(
                TENANT,
                &pointer.body_sha256,
            );
            let live = blob
                .store
                .has_named_holder(&pointer.engine_manifest_digest, &holder)
                .unwrap();
            assert!(live);
            bodies.push(serde_json::json!([
                format!("{pointer:?}"),
                bytes,
                live,
                blob.store
                    .refcount(&pointer.engine_manifest_digest)
                    .unwrap()
            ]));
        }
    }
    serde_json::json!({"owner": store.connector_pack_refusal_snapshot(TENANT), "bodies": bodies})
}

/// Release one engine body's pack holder and sweep it out of the CAS.
async fn withdraw_engine_body(served: &Served, manifest_digest: &str, body_sha256: &Digest256) {
    let (blob, _store) = {
        let state = served.state.read().await;
        (
            state.blob_cursors().cloned().unwrap(),
            state.agent_library().clone().unwrap(),
        )
    };
    let caller = medulla_server_api::carrier::CarrierAuthority::from_verified(
        &medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test(ADMIN),
    )
    .unwrap();
    let method = medulla_protocol_types::protocol::Method::BlobUnref {
        digest: manifest_digest.to_string(),
    };
    let (batch, now) = medulla_server_ops::api::ingest::compile_blob_batch(
        blob.store.as_ref(),
        9,
        &caller,
        &method,
    )
    .unwrap();
    let change = medulla_server_blob::blob::store::HolderChange::release(
        manifest_digest,
        medulla_server_blob::blob::store::HolderId::new(
            &medulla_server_blob::blob::engine_bodies::engine_body_holder(TENANT, body_sha256),
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
        10,
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
        blob.store.get_manifest(manifest_digest).unwrap().is_none(),
        "the withdrawn body must be gone: {reclaimed:?}"
    );
}

/// Write-time refusal codes leave the catalog untouched: a moved head, a
/// stale projection plan and a withdrawn body each refuse with its stable
/// code, and the head, members, body holders and outbox are exactly as they
/// were before the attempt. A direct `AgentComponent` write of a
/// pack-reserved component id refuses with `RESERVED_COMPONENT_ID` and
/// leaves the same state untouched.
// spec: MEDULLA-TYPED-PACKS-R041
#[tokio::test]
async fn pack_import_refusals_leave_state_unchanged() {
    let served = Served::new();
    let connector = "pack-r041-refusals";
    bind(&served, connector, ADMIN).await;
    let first = imported(
        &served,
        &build_pack(
            connector,
            &[
                tool(connector, "a", "Tool a."),
                tool(connector, "b", "Tool b."),
            ],
        ),
        None,
    )
    .await;

    // PACK_HEAD_CONFLICT: the expected head no longer matches the committed one.
    {
        let before = refusal_snapshot(&served, connector).await;
        let stale_head = medulla_catalog_types::connector_pack::PackHeadRef {
            binding_revision: first.binding_revision + 1,
            pack_digest: first.pack_digest,
        };
        let response = import(
            &served,
            &build_pack(connector, &[tool(connector, "c", "Tool c.")]),
            Some(stale_head),
        )
        .await;
        assert_eq!(refused("the moved head", response), "PACK_HEAD_CONFLICT");
        assert_eq!(
            refusal_snapshot(&served, connector).await,
            before,
            "a head-conflict refusal must not mutate head, members, holders or outbox"
        );
    }

    // PACK_PLAN_STALE: a projection plan built against the old head cannot
    // commit once a newer import has moved the head past it.
    let second = {
        let store = served.state.write().await.ensure_agent_library().unwrap();
        let stale = store
            .prepare_connector_pack_projection(TENANT, &ResourceId::new(connector).unwrap())
            .unwrap();
        let second = imported(
            &served,
            &build_pack(
                connector,
                &[
                    tool(connector, "a", "Tool a."),
                    tool(connector, "b", "Tool b."),
                    tool(connector, "c", "Tool c."),
                ],
            ),
            Some(head_of(&first)),
        )
        .await;
        drop(store);
        let before = refusal_snapshot(&served, connector).await;
        let base =
            medulla_server_api::authority_context::VerifiedRequestContext::verified_for_test(ADMIN);
        let verified =
            medulla_server_api::authority_context::VerifiedRequestContext::from_verified_claims_with_nonce(
                base.claims().clone(),
                "r041-stale-projection".to_string(),
                Some(medulla_types::contract::Nonce::minted()),
            );
        let context =
            medulla_persistence_records::agent_library::context::bind_agent_library_context(
                served.state.write().await.agent_library().unwrap().as_ref(),
                0,
                &verified,
                context("r041-stale-projection"),
                "connector-pack:reproject",
                true,
            )
            .unwrap();
        let store = served.state.write().await.ensure_agent_library().unwrap();
        let error = store
            .commit_connector_pack_projection(
                context,
                &stale,
                medulla_catalog_types::connector_pack::PackProjectionState::Applied {
                    graph: format!("pack__{}", "a".repeat(64)),
                    graph_version: 1,
                },
            )
            .unwrap_err();
        assert!(error.starts_with("PACK_PLAN_STALE:"), "{error}");
        assert_eq!(
            refusal_snapshot(&served, connector).await,
            before,
            "a stale-plan refusal must not mutate head, members, holders or outbox"
        );
        second
    };

    // BODY_MISSING: the body a to-be-withdrawn revision points at was
    // reclaimed, so the import cannot copy it.
    {
        let pointer = {
            let store = served.state.write().await.ensure_agent_library().unwrap();
            let member = store
                .connector_pack_members(TENANT, &ResourceId::new(connector).unwrap())
                .unwrap()
                .into_iter()
                .find(|member| member.component_id.ends_with("/a"))
                .unwrap();
            let entry = store
                .current_component(TENANT, &member.component_id)
                .unwrap()
                .unwrap();
            store
                .connector_pack_content_pointer(
                    TENANT,
                    &member.component_id,
                    Some(entry.entry_revision),
                )
                .unwrap()
        };
        withdraw_engine_body(
            &served,
            &pointer.engine_manifest_digest,
            &pointer.body_sha256,
        )
        .await;
        let before = served
            .state
            .write()
            .await
            .ensure_agent_library()
            .unwrap()
            .connector_pack_refusal_snapshot(TENANT);
        let response = import(
            &served,
            &build_pack(
                connector,
                &[
                    tool(connector, "b", "Tool b."),
                    tool(connector, "c", "Tool c."),
                    tool(connector, "d", "Tool d."),
                ],
            ),
            Some(head_of(&second)),
        )
        .await;
        assert_eq!(refused("the withdrawn body", response), "BODY_MISSING");
        assert_eq!(
            served
                .state
                .write()
                .await
                .ensure_agent_library()
                .unwrap()
                .connector_pack_refusal_snapshot(TENANT),
            before,
            "a missing-body refusal must not mutate head, members, holders or outbox"
        );
    }

    // RESERVED_COMPONENT_ID: a caller writes a pack member's reserved id
    // directly instead of importing it through the pack.
    {
        let before = served
            .state
            .write()
            .await
            .ensure_agent_library()
            .unwrap()
            .connector_pack_refusal_snapshot(TENANT);
        let mut publish_context = context("r041-reserved-id");
        publish_context.principal = persistence_id(ADMIN);
        publish_context.caller_principal = persistence_id(ADMIN);
        let response = served
            .call(
                "r041-reserved-id",
                medulla_protocol_types::protocol::Method::AgentComponent {
                    op: medulla_agent_types::agent_component::AgentComponentOp::Publish {
                        request: Box::new(
                            medulla_agent_types::agent_component::AgentComponentPublishRequest {
                                context: publish_context,
                                component:
                                    medulla_persistence_records::agent_component::test_component_draft(
                                        TENANT,
                                        &format!("mcp:{connector}/tool/b"),
                                    ),
                                evaluation_receipt_digest: None,
                            },
                        ),
                    },
                },
            )
            .await;
        assert_eq!(
            refused("the reserved component id", response),
            "RESERVED_COMPONENT_ID"
        );
        assert_eq!(
            served
                .state
                .write()
                .await
                .ensure_agent_library()
                .unwrap()
                .connector_pack_refusal_snapshot(TENANT),
            before,
            "a reserved-id refusal must not mutate head, members, holders or outbox"
        );
    }
}

// spec: MEDULLA-TYPED-PACKS-R003, MEDULLA-TYPED-PACKS-R042.2
#[tokio::test]
async fn a_rejected_import_mutates_nothing_in_the_catalog() {
    let served = Served::new();
    let connector = "pack-admin-zero-mutation";
    bind(&served, connector, ADMIN).await;
    let first = imported(
        &served,
        &build_pack(connector, &[tool(connector, "a", "Tool a.")]),
        None,
    )
    .await;
    let before = refusal_snapshot(&served, connector).await;
    // A second release whose entry list is out of its required sort order
    // (validate_header requires ascending, unique URIs) -- malformed, so a
    // rejection that must leave the catalog untouched.
    let (mut index, archive) = build_pack(
        connector,
        &[
            tool(connector, "b", "Tool b."),
            tool(connector, "z", "Tool z."),
        ],
    );
    let mut entries: Vec<_> = index.entries.iter().cloned().collect();
    entries.reverse();
    index.entries = BoundedVec::new(entries).unwrap();
    index.archive.blob_digest = served.upload(&archive).await;
    let key = "import:pack-admin-zero-mutation:unsorted";
    let rejected = served
        .pack(
            key,
            ConnectorPackOp::Import {
                request: Box::new(
                    medulla_catalog_types::connector_pack::ConnectorPackImportRequest {
                        context: context(key),
                        index: index.clone(),
                        expected_head: Some(head_of(&first)),
                        allow_mass_withdrawal: false,
                    },
                ),
            },
        )
        .await;
    assert_eq!(
        refused("noncanonical import key", rejected),
        "IDEMPOTENCY_CONFLICT",
    );
    assert_eq!(
        rejected_codes(ok(
            "Import",
            import(&served, &(index, archive), Some(head_of(&first))).await,
        )),
        [PackViolationCode::MalformedIndex],
    );
    let after = refusal_snapshot(&served, connector).await;
    assert_eq!(
        before, after,
        "a rejected import must commit zero catalog mutation"
    );

    for field in ["provides", "requires_capabilities"] {
        for (iri, code) in [
            (
                "eg:capability/retrieval/web-searc",
                PackViolationCode::UnknownCapabilityIri,
            ),
            ("1vendor:search", PackViolationCode::InvalidAnnotation),
        ] {
            let mut candidate = tool(connector, "a", &format!("Rejected {field}: {iri}"));
            candidate.annotations =
                serde_json::from_value(serde_json::json!({field: [iri]})).unwrap();
            let pack = build_pack(connector, &[candidate]);
            served.upload(&pack.1).await;
            let blob = served.state.read().await.blob_cursors().cloned().unwrap();
            let blob_counts = (
                blob.store.blob_count().unwrap(),
                blob.store.chunk_count().unwrap(),
            );
            let before = refusal_snapshot(&served, connector).await;
            assert_eq!(
                rejected_codes(ok(
                    "Import",
                    import(&served, &pack, Some(head_of(&first))).await
                )),
                [code],
                "{field}: {iri}"
            );
            assert_eq!(
                refusal_snapshot(&served, connector).await,
                before,
                "{field}: {iri}"
            );
            assert_eq!(
                (
                    blob.store.blob_count().unwrap(),
                    blob.store.chunk_count().unwrap()
                ),
                blob_counts,
                "rejected bodies never enter the engine CAS"
            );
        }
    }
}

// spec: MEDULLA-TYPED-PACKS-R005
#[tokio::test]
async fn an_end_to_end_connector_fixture_imports_through_to_a_served_read() {
    let served = Served::new();
    let connector = "pack-admin-e2e";
    bind(&served, connector, ADMIN).await;
    let fixture = tool(connector, "fixture", "An end-to-end fixture tool.");
    imported(
        &served,
        &build_pack(connector, std::slice::from_ref(&fixture)),
        None,
    )
    .await;
    let content: medulla_agent_types::agent_component::AgentComponentContentResult = ok(
        "Content",
        served
            .call(
                "content:pack-admin-e2e:fixture",
                medulla_protocol_types::protocol::Method::AgentComponent {
                    op: medulla_agent_types::agent_component::AgentComponentOp::Content {
                        request:
                            medulla_agent_types::agent_component::AgentComponentContentRequest {
                                tenant_id: TENANT.to_string(),
                                component_id: "mcp:pack-admin-e2e/tool/fixture".to_string(),
                                entry_revision: None,
                            },
                    },
                },
            )
            .await,
    );
    assert_eq!(
        content.body, fixture.body,
        "the fixture's served read matches exactly what Import landed"
    );
}
