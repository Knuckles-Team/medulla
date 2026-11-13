//! Query-federation handlers (CONCEPT:MEDULLA-KG.query.query-federation, Lane P).
//!
//! `RegisterForeignSource` records a named EXTERNAL source (a remote medulla
//! engine or an HTTP/JSON API — [`medulla_query_types::wire::ForeignSourceSpec`]) in the
//! owner-scoped [`medulla_server_api::api::query_txn::foreign_catalog::ForeignSourceCatalog`] on
//! `ServerState`, under the caller's VERIFIED owner — tenant+principal (EH-373). The actual
//! cross-engine / HTTP fetch is driven by the unified-query handlers: an inline-spec
//! `Op::ForeignScan` resolves itself, and a `Named` `Op::ForeignScan` / an `Op::Foreign`
//! (the UQL `FOREIGN "<name>"` marker) resolves through the registry
//! `ForeignSourceCatalog::registry_for` builds from the caller's own entries only
//! (CONCEPT:MEDULLA-KG.query.closure-backed-source). One principal can therefore neither use
//! nor overwrite another principal's registration, even under the same name (one engine
//! is bound to one tenant; the tenant stays inside the owner key). Rows a foreign
//! source returns are not RLS-filtered. A lightweight, non-blocking insert — no
//! off-reactor work needed.

use medulla_catalog_types::foreign_read::{
    ForeignReadAbortRequest, ForeignReadChunkRequest, ForeignReadCompleteRequest,
    ForeignReadOpenRequest, ForeignReadSubmission, ForeignReadSubmissionEvidence,
    FOREIGN_READ_SCHEMA_VERSION,
};
#[cfg(feature = "redb")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_auth::access::OwnerScopedCall;
use medulla_server_ops::api::query_txn::ForeignStateExt;

/// Where a registration writes its `MetadataContract` (MEDULLA-UNIFIED-DATA-PLANE-R038.2.1): the
/// request's graph, through the same durable mutation path as every other graph write.
pub struct ContractWriteTarget<'a> {
    pub core: &'a std::sync::Arc<medulla_graph_core::graph::GraphCore>,
    pub persistence:
        Option<&'a std::sync::Arc<dyn medulla_server_api::api::persistence::PersistenceBackend>>,
    pub graph: &'a str,
    pub principal: Option<&'a str>,
    pub tenant_scope: &'a str,
}

/// The `vg:MetadataContract` class every registered source's discovery writes.
const METADATA_CONTRACT_CLASS: &str =
    "https://knuckles-team.github.io/ontology/virtual-graph#MetadataContract";

/// The graph node id of `owner_scope`'s contract for source `name`: a digest, so no name or
/// scope text can shape the id.
pub fn contract_node_id(owner_scope: &str, name: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update((owner_scope.len() as u64).to_be_bytes());
    digest.update(owner_scope.as_bytes());
    digest.update(name.as_bytes());
    format!("vg-contract:{}", hex::encode(digest.finalize()))
}

/// The contract stored for `node_id`, if the graph holds one.
fn stored_contract(
    core: &medulla_graph_core::graph::GraphCore,
    node_id: &str,
) -> Option<medulla_types::metadata_acceleration::MetadataContract> {
    let blob = core.get_node_properties(node_id)?;
    let value: serde_json::Value = rmp_serde::from_slice(&blob).ok()?;
    Some(medulla_types::metadata_acceleration::MetadataContract {
        source_kind: value.get("source_kind")?.as_str()?.to_string(),
        source_name: value.get("source_name")?.as_str()?.to_string(),
        schema_version: u32::try_from(value.get("schema_version")?.as_u64()?).ok()?,
        content_hash: value.get("content_hash")?.as_str()?.to_string(),
    })
}

/// Write `contract` as one `AddNode` MutationBatch. A contract unchanged since the stored one is
/// not rewritten.
async fn write_contract(
    target: &ContractWriteTarget<'_>,
    req_id: u64,
    owner: &medulla_server_api::carrier::CarrierAuthority,
    node_id: String,
    contract: &medulla_types::metadata_acceleration::MetadataContract,
    stored: Option<&medulla_types::metadata_acceleration::MetadataContract>,
) -> Result<(), String> {
    if stored == Some(contract) {
        return Ok(());
    }
    let failed = |detail: String| {
        format!(
            "{}: {detail}",
            medulla_types::contract::ServerErrorCode::DiscoveryContractWriteFailed
        )
    };
    let properties = serde_json::json!({
        "type": "MetadataContract",
        "rdf_type": METADATA_CONTRACT_CLASS,
        "source_kind": contract.source_kind,
        "source_name": contract.source_name,
        "schema_version": contract.schema_version,
        "content_hash": contract.content_hash,
        "_owner": owner.agent_id(),
        "_visibility": "private",
    });
    let properties_msgpack = rmp_serde::to_vec_named(&properties)
        .map_err(|error| failed(format!("contract encoding failed: {error}")))?;
    let method = Method::AddNode {
        node_id,
        properties_msgpack,
    };
    let batch_id = medulla_server_api::batch_compile::opaque_request_key(
        "foreign-contract",
        target.graph,
        req_id,
        &method,
    );
    medulla_server_mutation::mutation_batch::commit_internal_graph_methods_with_nonce(
        medulla_server_mutation::mutation_batch::InternalGraphCommitRequest::new(
            target.persistence,
            target.core,
            medulla_server_mutation::mutation_batch::CommitOrigin {
                request_id: req_id,
                principal: target.principal,
            },
            target.graph,
            &batch_id,
            vec![method],
            &ResultPayload::Bool(true),
        )
        .with_tenant_scope(target.tenant_scope),
    )
    .await
    .map(|_| ())
    .map_err(failed)
}

/// Try to handle a federation method. `Ok(resp)` = handled; `Err(method)` = not mine.
pub async fn try_handle(
    call: OwnerScopedCall<'_>,
    method: Method,
    target: &ContractWriteTarget<'_>,
) -> Result<Response, Method> {
    let req_id = call.req_id;
    match method {
        Method::RegisterForeignSource {
            name,
            source,
            quota,
        } => {
            let owner = match call.owner("RegisterForeignSource") {
                Ok(owner) => owner,
                Err(refusal) => return Ok(refusal),
            };
            if let Err(error) = medulla_federation::federation::validate_column_mapping(&source) {
                return Ok(Response::err(req_id, error));
            }
            // MEDULLA-UNIFIED-DATA-PLANE-R038.2.1: discovery reads catalog metadata only and runs
            // before anything is registered, so an unreachable source leaves no catalog entry.
            let node_id = contract_node_id(owner.owner_scope(), &name);
            let stored = stored_contract(target.core, &node_id);
            let contract =
                match medulla_server_ops::api::query_txn::foreign_discovery::discover_source(
                    &name,
                    &source,
                    stored.as_ref(),
                )
                .await
                {
                    Ok(contract) => contract,
                    Err(refusal) => return Ok(Response::err(req_id, refusal.to_string())),
                };
            // EH-378: provision the source's share role (assigned to nobody) under the
            // same write lock as the registration, so a registered source always has one.
            let mut s = call.state.write().await;
            if let Err(error) =
                medulla_server_ops::api::query_txn::foreign_share::provision_share_role(
                    &mut s.isolation,
                    owner.agent_id(),
                    &name,
                )
            {
                return Ok(Response::err(req_id, error));
            }
            // MEDULLA-UNIFIED-DATA-PLANE-R031.2: a registration without a quota takes the
            // resolved `foreign.default_quota.*` knobs.
            let knobs = medulla_server_api::knob_config::knobs();
            let quota = quota.unwrap_or(medulla_types::consolidation_isolation::SourceQuota {
                max_concurrent: u32::try_from(knobs.foreign_default_quota_max_concurrent)
                    .unwrap_or(u32::MAX),
                max_rows: knobs.foreign_default_quota_max_rows,
                max_wall_ms: knobs.foreign_default_quota_max_wall_ms,
            });
            let catalog = s.foreign_sources();
            let prior = catalog.prior_registration(&owner, &name);
            if let Err(error) = catalog.register(&owner, name.clone(), source, quota) {
                return Ok(Response::err(req_id, error));
            }
            drop(s);
            // The contract is written in the same call as the catalog insert; when the durable
            // write fails the insert is undone and the registration is refused.
            if let Err(error) =
                write_contract(target, req_id, &owner, node_id, &contract, stored.as_ref()).await
            {
                catalog.restore_registration(&owner, &name, prior);
                return Ok(Response::err(req_id, error));
            }
            // MEDULLA-UNIFIED-DATA-PLANE-R014.2.3: registration is the synchronous record
            // write above. A MySQL or MariaDB source is probed by a separate job, whose
            // outcome is the source's probe state, so nothing here waits for it.
            #[cfg(feature = "federation-sql")]
            if catalog.needs_catalog_probe(&owner, &name) {
                let owner_scope = owner.owner_scope().to_string();
                let probed = name.clone();
                tokio::spawn(async move {
                    let _ = catalog.run_catalog_probe(&owner_scope, &probed).await;
                });
            }
            Ok(Response::ok(
                req_id,
                ResultPayload::scalar::<
                    medulla_protocol_types::result_contract::cluster::RegisterForeignSource,
                >(name),
            ))
        }
        Method::RegisterGraphView {
            name,
            view,
            mapping,
            tables,
            sources,
        } => {
            let owner = match call.owner("RegisterGraphView") {
                Ok(owner) => owner,
                Err(refusal) => return Ok(refusal),
            };
            let s = call.state.read().await;
            let registered = s.foreign_sources().views().register(
                &owner,
                name.clone(),
                medulla_server_ops::api::query_txn::foreign_catalog::RegisteredGraphView {
                    view,
                    mapping,
                    tables,
                    sources,
                },
            );
            drop(s);
            match registered {
                Ok(()) => Ok(Response::ok(
                    req_id,
                    ResultPayload::scalar::<
                        medulla_protocol_types::result_contract::cluster::RegisterGraphView,
                    >(name),
                )),
                Err(error) => Ok(Response::err(req_id, error)),
            }
        }
        Method::QueryForeignColumns {
            name,
            columns,
            predicates,
        } => Ok(query_columns(call, name, columns, predicates).await),
        Method::ForeignReadOpen { request } => Ok(open_foreign_read(call, request).await),
        Method::ForeignReadChunk { request } => Ok(chunk_foreign_read(call, request).await),
        Method::ForeignReadComplete { request } => Ok(complete_foreign_read(call, request).await),
        Method::ForeignReadAbort { request } => Ok(abort_foreign_read(call, request).await),
        other => Err(other),
    }
}

/// MEDULLA-FEDERATED-QUERY-R053.701: validate a job-form submission against the
/// admitted approval and derive its fence evidence. The one preset a source
/// offers is its approved operation, the one snapshot it knows is the pack
/// digest the approval pins (both server-derived, never host-chosen), and the
/// submission id is `SubmissionId::derive`, the only id function.
fn submission_evidence(
    approval: &medulla_catalog_types::connector_pack::ConnectorPackSourceApprovalResult,
    submission: &ForeignReadSubmission,
) -> Result<ForeignReadSubmissionEvidence, String> {
    use medulla_federation::federation_spark::{
        check_snapshot, derive_submission, SubmissionFence,
    };
    let registration = &approval.registration;
    let digests: Vec<&str> = submission
        .input_digests
        .iter()
        .map(String::as_str)
        .collect();
    let id = derive_submission(
        &[registration.operation.as_str()],
        &submission.preset_id,
        &digests,
        &submission.parameters,
    )
    .map_err(|refusal| {
        format!(
            "{}: preset is not offered by the approved source",
            refusal.code().as_str()
        )
    })?;
    let fence = SubmissionFence {
        source_id: Some(registration.source_id.clone()),
        snapshot_id: Some(submission.snapshot_id.clone()),
        submission_id: Some(id.as_str().to_string()),
        generation: Some(approval.head.binding_revision),
        producer_revision: Some(submission.producer_revision.clone()),
    };
    fence.check().map_err(|refusal| {
        let medulla_federation::federation_spark::SubmissionRefusal::FenceFieldMissing(field) =
            &refusal
        else {
            return format!("{}: fence check failed", refusal.code().as_str());
        };
        format!("{}: {field} is missing", refusal.code().as_str())
    })?;
    let pinned = approval.head.pack_digest.to_hex();
    check_snapshot(&[pinned.as_str()], &submission.snapshot_id).map_err(|code| {
        format!(
            "{}: snapshot is not one the approved source registered",
            code.as_str()
        )
    })?;
    Ok(ForeignReadSubmissionEvidence {
        source_id: registration.source_id.clone(),
        snapshot_id: submission.snapshot_id.clone(),
        submission_id: id.as_str().to_string(),
        generation: approval.head.binding_revision,
        producer_revision: submission.producer_revision.clone(),
    })
}

async fn open_foreign_read(call: OwnerScopedCall<'_>, request: ForeignReadOpenRequest) -> Response {
    let req_id = call.req_id;
    let owner = match call.owner("ForeignReadOpen") {
        Ok(owner) => owner,
        Err(refusal) => return refusal,
    };
    if !owner.can_read() {
        medulla_kernel::metrics::process().access_denied();
        return Response::err(req_id, "ACCESS_DENIED: ForeignReadOpen requires kg:read");
    }
    if request.schema_version != FOREIGN_READ_SCHEMA_VERSION {
        return Response::err(
            req_id,
            "FOREIGN_READ_SCHEMA_MISMATCH: unsupported schema version",
        );
    }
    if let Err(error) = request.limits.validate() {
        return Response::err(req_id, error);
    }
    let store = {
        let state = call.state.read().await;
        state.agent_library()
    };
    let Some(store) = store else {
        return Response::err(
            req_id,
            "SOURCE_APPROVAL_STORE_UNAVAILABLE: foreign read requires the Agent Library",
        );
    };
    let ForeignReadOpenRequest {
        source_name,
        approval,
        plan_digest,
        query_digest,
        limits,
        submission,
        ..
    } = request;
    let evidence = match submission.as_ref() {
        Some(submission) => match submission_evidence(&approval, submission) {
            Ok(evidence) => Some(evidence),
            Err(error) => return Response::err(req_id, error),
        },
        None => None,
    };
    let opened = {
        let state = call.state.read().await;
        match state.foreign_reads().open_submission(
            &owner,
            source_name.clone(),
            approval.clone(),
            plan_digest,
            query_digest,
            limits,
            evidence.as_ref().map(|e| e.submission_id.clone()),
        ) {
            Ok(opened) => opened,
            Err(error) => return Response::err(req_id, error),
        }
    };
    let (execution, expires_at_ms, source_identity) = (
        opened.execution,
        opened.expires_at_ms,
        opened.source_identity,
    );
    if opened.resumed {
        // The identical submission already runs: name that run, never bind or
        // start a second one.
        let result = medulla_catalog_types::foreign_read::ForeignReadOpenResult {
            schema_version: FOREIGN_READ_SCHEMA_VERSION,
            execution,
            expires_at_ms,
            next_sequence: opened.next_sequence,
            source_identity,
            submission: evidence,
        };
        return Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::query::ForeignReadOpen>(
                result,
            ),
        );
    }
    let binding = {
        let state = call.state.read().await;
        let page_source = match state.foreign_reads().page_source(execution.execution_id) {
            Ok(source) => source,
            Err(error) => {
                state.foreign_reads().remove(execution.execution_id);
                return Response::err(req_id, error);
            }
        };
        state.foreign_sources().bind_registered_for(
            &owner,
            store.as_ref(),
            &state.isolation,
            &approval,
            &source_name,
            page_source,
        )
    };
    if let Err(error) = binding {
        let state = call.state.read().await;
        state.foreign_reads().remove(execution.execution_id);
        return Response::err(req_id, error);
    }
    let result = medulla_catalog_types::foreign_read::ForeignReadOpenResult {
        schema_version: FOREIGN_READ_SCHEMA_VERSION,
        execution,
        expires_at_ms,
        next_sequence: 0,
        source_identity,
        submission: evidence,
    };
    Response::ok(
        req_id,
        ResultPayload::of::<medulla_protocol_types::result_contract::query::ForeignReadOpen>(
            result,
        ),
    )
}

async fn chunk_foreign_read(
    call: OwnerScopedCall<'_>,
    request: ForeignReadChunkRequest,
) -> Response {
    let req_id = call.req_id;
    let owner = match call.owner("ForeignReadChunk") {
        Ok(owner) => owner,
        Err(refusal) => return refusal,
    };
    if !owner.can_read() {
        medulla_kernel::metrics::process().access_denied();
        return Response::err(req_id, "ACCESS_DENIED: ForeignReadChunk requires kg:read");
    }
    let result = {
        let state = call.state.read().await;
        let store = state.agent_library();
        let Some(store) = store.as_deref() else {
            return Response::err(
                req_id,
                "SOURCE_APPROVAL_STORE_UNAVAILABLE: foreign read requires the Agent Library",
            );
        };
        state
            .foreign_reads()
            .append(&owner, store, &state.foreign_sources(), request)
    };
    match result {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::query::ForeignReadChunk>(
                result,
            ),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

async fn complete_foreign_read(
    call: OwnerScopedCall<'_>,
    request: ForeignReadCompleteRequest,
) -> Response {
    let req_id = call.req_id;
    let owner = match call.owner("ForeignReadComplete") {
        Ok(owner) => owner,
        Err(refusal) => return refusal,
    };
    if !owner.can_read() {
        medulla_kernel::metrics::process().access_denied();
        return Response::err(
            req_id,
            "ACCESS_DENIED: ForeignReadComplete requires kg:read",
        );
    }
    let result = {
        let state = call.state.read().await;
        let store = state.agent_library();
        let Some(store) = store.as_deref() else {
            return Response::err(
                req_id,
                "SOURCE_APPROVAL_STORE_UNAVAILABLE: foreign read requires the Agent Library",
            );
        };
        state
            .foreign_reads()
            .complete(&owner, store, &state.foreign_sources(), request)
    };
    match result {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::query::ForeignReadComplete>(
                result,
            ),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

async fn abort_foreign_read(
    call: OwnerScopedCall<'_>,
    request: ForeignReadAbortRequest,
) -> Response {
    let req_id = call.req_id;
    let owner = match call.owner("ForeignReadAbort") {
        Ok(owner) => owner,
        Err(refusal) => return refusal,
    };
    if !owner.can_read() {
        medulla_kernel::metrics::process().access_denied();
        return Response::err(req_id, "ACCESS_DENIED: ForeignReadAbort requires kg:read");
    }
    let result = {
        let state = call.state.read().await;
        let store = state.agent_library();
        let Some(store) = store.as_deref() else {
            return Response::err(
                req_id,
                "SOURCE_APPROVAL_STORE_UNAVAILABLE: foreign read requires the Agent Library",
            );
        };
        state
            .foreign_reads()
            .abort(&owner, store, &state.foreign_sources(), request)
    };
    match result {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::query::ForeignReadAbort>(
                result,
            ),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// Build the caller's registry while the catalog is locked, then release the
/// lock before the blocking SQL fetch. Only the verified carrier chooses scope.
async fn query_columns(
    call: OwnerScopedCall<'_>,
    name: String,
    columns: Vec<String>,
    predicates: Vec<medulla_query_types::wire::ForeignColumnPredicate>,
) -> Response {
    let req_id = call.req_id;
    let owner = match call.owner("QueryForeignColumns") {
        Ok(owner) => owner,
        Err(refusal) => return refusal,
    };
    if !owner.can_read() {
        medulla_kernel::metrics::process().access_denied();
        return Response::err(
            req_id,
            "ACCESS_DENIED: QueryForeignColumns requires kg:read",
        );
    }
    if columns.len() > 128 || predicates.len() > 128 {
        return Response::err(
            req_id,
            "INVALID_ARGUMENT: foreign column request exceeds 128 fields or predicates",
        );
    }
    let registry = {
        let state = call.state.read().await;
        state
            .foreign_sources()
            .registry_for(&owner, &state.isolation)
    };
    let result = tokio::task::spawn_blocking(move || {
        use medulla_federation::{
            federation_opt::ColumnPredicate, federation_opt::Comparison,
            federation_opt::FederationSession,
        };
        use medulla_query_types::wire::ForeignColumnComparison as WireComparison;

        let predicates = predicates
            .into_iter()
            .map(|predicate| ColumnPredicate {
                column: predicate.column,
                comparison: match predicate.comparison {
                    WireComparison::Eq => Comparison::Eq,
                    WireComparison::Ne => Comparison::Ne,
                    WireComparison::Lt => Comparison::Lt,
                    WireComparison::Le => Comparison::Le,
                    WireComparison::Gt => Comparison::Gt,
                    WireComparison::Ge => Comparison::Ge,
                },
                value: predicate.value,
            })
            .collect::<Vec<_>>();
        let session = FederationSession::from_env();
        registry
            .registry()
            .query_columns(&name, &columns, &predicates, &session)
            .map(|rows| {
                rows.rows()
                    .iter()
                    .map(|row| medulla_query_types::wire::ForeignColumnRow {
                        id: row.id.clone(),
                        score: row.score,
                        columns: row.columns.clone(),
                    })
                    .collect::<Vec<_>>()
            })
    })
    .await;
    match result {
        Ok(Ok(rows)) => Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::query::QueryForeignColumns>(
                rows,
            ),
        ),
        Ok(Err(error)) => Response::err(req_id, column_query_error(&error)),
        Err(_) => Response::err(req_id, "INTERNAL: foreign column worker failed"),
    }
}

/// Map connector and planner prose to the codes declared for this read method.
/// Do not promote a source-supplied prefix into an arbitrary wire code.
fn column_query_error(error: &str) -> String {
    if error.starts_with(medulla_federation::federation_opt::BUDGET_EXCEEDED) {
        if error.contains(":wall_ms:") {
            return "ENGINE_DEADLINE_EXCEEDED: foreign column read exceeded time budget".into();
        }
        return "ENGINE_RESOURCE_EXHAUSTED: foreign column read exceeded resource budget".into();
    }
    if error.starts_with(medulla_federation::federation_opt::RESULT_INCOMPLETE) {
        return "ENGINE_RESOURCE_EXHAUSTED: foreign column result incomplete".into();
    }
    if error.contains("HTTP column projection exceeds limit") {
        return "ENGINE_RESOURCE_EXHAUSTED: foreign column projection exceeded size limit".into();
    }
    if [
        "not exposed",
        "column mapping",
        "column query requires a registered source",
        "SQL query is not read-only",
        "SQL column predicates require string values",
        "unsupported SQL connection scheme",
    ]
    .iter()
    .any(|message| error.contains(message))
        || error.starts_with(medulla_federation::federation_opt::REQUIRES_KEYS)
    {
        return "INVALID_ARGUMENT: foreign column request is invalid".into();
    }
    "ENGINE_UNAVAILABLE: foreign column source unavailable".into()
}

#[cfg(test)]
mod tests {
    use super::column_query_error;

    #[test]
    fn foreign_column_argument_errors_preserve_matching_and_priority() {
        for message in [
            "not exposed",
            "column mapping",
            "column query requires a registered source",
            "SQL query is not read-only",
            "SQL column predicates require string values",
            "unsupported SQL connection scheme",
        ] {
            assert_eq!(
                column_query_error(&format!("federation: {message}: detail")),
                "INVALID_ARGUMENT: foreign column request is invalid"
            );
            assert_eq!(
                column_query_error(&format!("FEDERATION_BUDGET_EXCEEDED:rows: {message}")),
                "ENGINE_RESOURCE_EXHAUSTED: foreign column read exceeded resource budget"
            );
        }
        assert_eq!(
            column_query_error("FEDERATION_SOURCE_REQUIRES_KEYS: detail"),
            "INVALID_ARGUMENT: foreign column request is invalid"
        );
        assert_eq!(
            column_query_error("backend: FEDERATION_SOURCE_REQUIRES_KEYS: detail"),
            "ENGINE_UNAVAILABLE: foreign column source unavailable"
        );
    }

    #[test]
    fn foreign_column_refusals_use_declared_read_codes() {
        assert!(column_query_error("FEDERATION_BUDGET_EXCEEDED:rows: limit")
            .starts_with("ENGINE_RESOURCE_EXHAUSTED:"));
        assert!(
            column_query_error("FEDERATION_BUDGET_EXCEEDED:wall_ms: limit")
                .starts_with("ENGINE_DEADLINE_EXCEEDED:")
        );
        assert!(
            column_query_error("federation: this source has no column mapping")
                .starts_with("INVALID_ARGUMENT:")
        );
        assert!(
            column_query_error("federation: HTTP column projection exceeds limit")
                .starts_with("ENGINE_RESOURCE_EXHAUSTED:")
        );
        let unexpected = column_query_error("UNDECLARED_CODE: secret backend detail");
        assert!(unexpected.starts_with("ENGINE_UNAVAILABLE:"));
        assert!(!unexpected.contains("secret backend detail"));
    }
}
