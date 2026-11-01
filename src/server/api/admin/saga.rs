//! Durable coordinator saga helpers shared by admin and transaction handlers.
//!
//! The saga ledger is reached through [`AdminSagaLedger`], implemented by the durable backend.

use crate::mutation_batch::MutationBatch;
use crate::mutation_batch::MutationBatchCommit;
use crate::mutation_batch::MutationBatchRecord;
use crate::protocol::Method;
use crate::server::access::CarrierAuthority;
use medulla_types::contract::Nonce;
use medulla_types::mutation_scope::DurabilityDomain;
use medulla_types::mutation_scope::MutationScopeIdentity;
use medulla_types::mutation_scope::MutationSurface;

/// The admin-mutations ledger operations an admin saga needs, implemented by the durable
/// backend.
#[cfg(feature = "redb")]
pub trait AdminSagaLedger {
    /// One scoped read over the admin-mutations ledger.
    fn admin_mutations_read(
        &self,
    ) -> Result<medulla_storage::ScopedRead<'_, medulla_storage::LedgerOnlyOwner>, String>;

    /// Prepare one admin saga, with its sealed private recovery payload when it has one.
    fn admin_saga_step(
        &self,
        batch: &medulla_protocol_types::mutation_batch::MutationBatch,
        prepared_at_ms: u64,
        private_payload: Option<&[u8]>,
    ) -> Result<medulla_transaction::SagaBegin, String>;

    /// Terminalize one prepared admin saga; the flag is `true` when it had already committed.
    fn admin_saga_end(
        &self,
        batch: &medulla_protocol_types::mutation_batch::MutationBatch,
        result_msgpack: Vec<u8>,
        committed_at_ms: u64,
    ) -> Result<
        (
            medulla_protocol_types::mutation_batch::MutationBatchRecord,
            bool,
        ),
        String,
    >;
}

/// Fixed [`medulla_types::MutationScopeIdentity`] for the admin-mutations / "cluster-admin"
/// coordinator store (`{persist_dir}/admin-mutations.redb`) — see `AdminMutationStore`.
///
/// GRAPH-shaped, not native: every batch here is compiled with an "either"-family domain
/// (`ControlPlane`/`MultiGraph`), which `mutation_batch::finish_batch` maps to a
/// graph-shaped identity, so this identity must match that shape exactly or every
/// admin-saga commit fails closed with "mutation scope binding identity mismatch". Only
/// `AdminOwner`'s `OwnerLayout::LedgerOnly` can serve it: declaring no owner tables, it
/// is the one layout that places no shape requirement on the scopes it binds.
#[cfg(feature = "redb")]
pub fn cluster_admin_scope_identity() -> Result<medulla_types::MutationScopeIdentity, String> {
    medulla_types::MutationScopeIdentity::fixed_graph(
        "native",
        "cluster-admin",
        crate::server::mutation_batch::COMPILED_BATCH_INCARNATION,
    )
}

tokio::task_local! {
    static AUTHENTICATED_SAGA_AUTHORITY: CarrierAuthority;
}

/// Run `request` with `authority` as the authenticated admin-saga authority.
pub async fn scope_admin_saga_authority<F: std::future::Future>(
    authority: CarrierAuthority,
    request: F,
) -> F::Output {
    AUTHENTICATED_SAGA_AUTHORITY.scope(authority, request).await
}

/// The authenticated saga authority of the current request scope, or an error outside one.
pub fn current_admin_saga_authority() -> Result<CarrierAuthority, String> {
    AUTHENTICATED_SAGA_AUTHORITY
        .try_with(Clone::clone)
        .map_err(|_| "admin saga requires authenticated request authority".to_string())
}

/// A begun admin saga: its durable batch and whether it replayed or resumed a prepared receipt.
#[cfg(feature = "redb")]
pub struct AdminSaga {
    /// The durable batch carrying this saga.
    pub batch: AdminSagaBatch,
    /// Millisecond timestamp the saga was prepared (or committed, when resumed).
    pub created_at_ms: u64,
    /// The committed result when the saga already finished.
    pub replayed: Option<crate::protocol::ResultPayload>,
    /// Whether the saga is already Prepared and must be resumed, not re-executed.
    pub prepared: bool,
}

#[cfg(feature = "redb")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminSagaResultContract {
    Bool,
    Count,
    Text,
    GraphQlResponse,
    SparqlRecoveryOutcome,
    ShardReshardReport,
    RebalanceExecution,
    RestoreReceipt,
    MultiGraphBatchReport,
    ChannelCreated,
    ChannelDeparture,
    BeliefMaterialization,
}

/// The public wire result is untagged. Persist its variant separately: a
/// `Json("left")` channel departure otherwise reloads as `String("left")`.
#[cfg(feature = "redb")]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "value")]
enum DurableAdminResult {
    Bool(bool),
    Count(u64),
    Text(String),
    Raw(Vec<u8>),
    Json(serde_json::Value),
}

#[cfg(feature = "redb")]
impl DurableAdminResult {
    fn from_result(result: &crate::protocol::ResultPayload) -> Result<Self, String> {
        use crate::protocol::ResultPayload;
        match result {
            ResultPayload::Bool(value) => Ok(Self::Bool(*value)),
            ResultPayload::Count(value) => Ok(Self::Count(*value)),
            ResultPayload::String(value) => Ok(Self::Text(value.clone())),
            ResultPayload::Raw(value) => Ok(Self::Raw(value.clone())),
            ResultPayload::Json(value) => Ok(Self::Json(value.clone())),
            _ => Err("admin saga result has no durable encoding".to_string()),
        }
    }

    fn into_result(self) -> crate::protocol::ResultPayload {
        use crate::protocol::ResultPayload;
        match self {
            Self::Bool(value) => ResultPayload::Bool(value),
            Self::Count(value) => ResultPayload::Count(value),
            Self::Text(value) => ResultPayload::String(value),
            Self::Raw(value) => ResultPayload::Raw(value),
            Self::Json(value) => ResultPayload::Json(value),
        }
    }
}

#[cfg(feature = "redb")]
impl AdminSagaResultContract {
    fn for_method(method: &Method) -> Result<Self, String> {
        #[cfg(feature = "graphql")]
        let graphql_contract =
            matches!(method, Method::GraphQl { .. }).then_some(Self::GraphQlResponse);
        #[cfg(not(feature = "graphql"))]
        let graphql_contract = None;
        if let Some(contract) = graphql_contract.or_else(|| Self::for_core_method(method)) {
            return Ok(contract);
        }
        if let Some(contract) = Self::for_knowledge_method(method) {
            return Ok(contract);
        }
        if let Some(contract) = Self::for_streaming_method(method) {
            return Ok(contract);
        }
        if let Some(contract) = Self::for_extension_method(method) {
            return Ok(contract);
        }
        Err(format!(
            "admin saga for {} has no declared result contract",
            method.tag_name()
        ))
    }

    fn for_core_method(method: &Method) -> Option<Self> {
        match method {
            Method::Reshard { .. } => Some(Self::ShardReshardReport),
            Method::RebalanceExecute { .. } => Some(Self::RebalanceExecution),
            Method::Restore { .. } => Some(Self::RestoreReceipt),
            Method::MultiGraphBatchUpdate { .. } => Some(Self::MultiGraphBatchReport),
            Method::CatalogAssign { .. }
            | Method::CatalogReassign { .. }
            | Method::CatalogRemove { .. }
            | Method::TxnAddNode { .. }
            | Method::TxnRemoveNode { .. }
            | Method::TxnAddEdge { .. }
            | Method::TxnRemoveEdge { .. }
            | Method::TxnCas { .. }
            | Method::TxnAddEmbedding { .. }
            | Method::TxnBlobRef { .. }
            | Method::TxnAddMeasurement { .. }
            | Method::Rollback { .. } => Some(Self::Bool),
            Method::BeginTxn { .. } | Method::JoinChannel { .. } | Method::SendMessage { .. } => {
                Some(Self::Text)
            }
            Method::CreateChannel { .. } => Some(Self::ChannelCreated),
            Method::LeaveChannel { .. } | Method::CloseChannel { .. } => {
                Some(Self::ChannelDeparture)
            }
            _ => None,
        }
    }

    fn for_knowledge_method(method: &Method) -> Option<Self> {
        match method {
            #[cfg(feature = "owl")]
            Method::TxnAxiom { .. } => Some(Self::Bool),
            #[cfg(feature = "sparql")]
            Method::TxnConstruct { .. } => Some(Self::Bool),
            #[cfg(feature = "query")]
            Method::TxnPlanWriteback { .. } => Some(Self::Bool),
            #[cfg(feature = "epistemic")]
            Method::TxnMaterializeBelief { .. } => Some(Self::BeliefMaterialization),
            _ => None,
        }
    }

    fn for_streaming_method(method: &Method) -> Option<Self> {
        match method {
            #[cfg(feature = "streaming")]
            Method::RegisterContinuousQuery { .. } | Method::RegisterTrigger { .. } => {
                Some(Self::Text)
            }
            #[cfg(feature = "streaming")]
            Method::DropContinuousQuery { .. } | Method::DropTrigger { .. } => Some(Self::Bool),
            #[cfg(all(feature = "streaming", feature = "stream"))]
            Method::CepSubscribe { .. } => Some(Self::Count),
            #[cfg(all(feature = "streaming", feature = "stream"))]
            Method::CepUnsubscribe { .. } => Some(Self::Bool),
            _ => None,
        }
    }

    fn for_extension_method(method: &Method) -> Option<Self> {
        match method {
            #[cfg(feature = "wasm-udf")]
            Method::RegisterUdf { .. } => Some(Self::Text),
            #[cfg(feature = "federation")]
            Method::RegisterForeignSource { .. } => Some(Self::Text),
            #[cfg(feature = "compute-dist")]
            Method::CreateMatView { .. } | Method::RefreshMatView { .. } => Some(Self::Count),
            #[cfg(feature = "matview")]
            Method::PlanMatViewDefine { .. } | Method::PlanMatViewRefresh { .. } => {
                Some(Self::Count)
            }
            #[cfg(feature = "matview")]
            Method::PlanMatViewDrop { .. } => Some(Self::Bool),
            _ => None,
        }
    }

    fn for_private_event(event_type: &str) -> Result<Self, String> {
        match event_type {
            "transaction_recovery_plan" | "sparql_http_compensation_v1" => Ok(Self::Bool),
            "sparql_http_recovery_plan_v1" => Ok(Self::SparqlRecoveryOutcome),
            _ => Err(format!(
                "private admin saga event {event_type:?} has no declared result contract"
            )),
        }
    }

    pub(crate) fn validate(self, result: &crate::protocol::ResultPayload) -> Result<(), String> {
        use crate::protocol::ResultPayload;
        match self {
            Self::Bool => matches!(result, ResultPayload::Bool(_))
                .then_some(())
                .ok_or_else(|| "admin saga replay has the wrong result type".to_string()),
            Self::Count => matches!(result, ResultPayload::Count(_))
                .then_some(())
                .ok_or_else(|| "admin saga replay has the wrong result type".to_string()),
            Self::Text => matches!(result, ResultPayload::String(_))
                .then_some(())
                .ok_or_else(|| "admin saga replay has the wrong result type".to_string()),
            Self::GraphQlResponse => matches!(result, ResultPayload::Raw(_))
                .then_some(())
                .ok_or_else(|| "admin saga replay has the wrong GraphQL result type".to_string()),
            Self::SparqlRecoveryOutcome => validate_sparql_recovery_outcome(result),
            Self::ShardReshardReport => validate_admin_json::<
                medulla_protocol_types::result_contract::cluster::ShardReshardReport,
            >(result),
            Self::RebalanceExecution => validate_admin_json::<
                medulla_protocol_types::result_contract::cluster::RebalanceExecution,
            >(result),
            Self::RestoreReceipt => {
                validate_admin_json::<medulla_protocol_types::storage_wire::RestoreReceipt>(result)
            }
            Self::MultiGraphBatchReport => validate_admin_json::<
                medulla_protocol_types::result_contract::transactions::MultiGraphBatchReport,
            >(result),
            Self::ChannelCreated => validate_admin_json::<
                medulla_protocol_types::messaging_wire::ChannelCreated,
            >(result),
            Self::ChannelDeparture => validate_admin_json::<
                medulla_protocol_types::messaging_wire::ChannelDeparture,
            >(result),
            Self::BeliefMaterialization => validate_admin_json::<
                medulla_protocol_types::result_contract::transactions::BeliefMaterialization,
            >(result),
        }
    }

    fn public_discriminator(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Count => "count",
            Self::Text => "text",
            Self::GraphQlResponse => "graphql-response-v1",
            Self::ShardReshardReport => "shard-reshard-report-v1",
            Self::RebalanceExecution => "rebalance-execution-v1",
            Self::RestoreReceipt => "restore-receipt-v1",
            Self::MultiGraphBatchReport => "multi-graph-batch-report-v1",
            Self::ChannelCreated => "channel-created-v1",
            Self::ChannelDeparture => "channel-departure-v1",
            Self::BeliefMaterialization => "belief-materialization-v1",
            Self::SparqlRecoveryOutcome => unreachable!("private contract used by public saga"),
        }
    }

    fn for_public_discriminator(discriminator: &str) -> Result<Self, String> {
        (discriminator == "graphql-response-v1")
            .then_some(Self::GraphQlResponse)
            .or_else(|| Self::for_public_scalar_discriminator(discriminator))
            .or_else(|| Self::for_public_structured_discriminator(discriminator))
            .ok_or_else(|| {
                format!(
                    "public admin saga has unknown result-contract discriminator {discriminator:?}"
                )
            })
    }

    fn for_public_scalar_discriminator(discriminator: &str) -> Option<Self> {
        match discriminator {
            "bool" => Some(Self::Bool),
            "count" => Some(Self::Count),
            "text" => Some(Self::Text),
            _ => None,
        }
    }

    fn for_public_structured_discriminator(discriminator: &str) -> Option<Self> {
        match discriminator {
            "shard-reshard-report-v1" => Some(Self::ShardReshardReport),
            "rebalance-execution-v1" => Some(Self::RebalanceExecution),
            "restore-receipt-v1" => Some(Self::RestoreReceipt),
            "multi-graph-batch-report-v1" => Some(Self::MultiGraphBatchReport),
            "channel-created-v1" => Some(Self::ChannelCreated),
            "channel-departure-v1" => Some(Self::ChannelDeparture),
            "belief-materialization-v1" => Some(Self::BeliefMaterialization),
            _ => None,
        }
    }
}

#[cfg(feature = "redb")]
const PUBLIC_ADMIN_EVENT_PREFIX: &str = "cluster_admin_operation/";

#[cfg(feature = "redb")]
fn validate_sparql_recovery_outcome(result: &crate::protocol::ResultPayload) -> Result<(), String> {
    let crate::protocol::ResultPayload::Json(value) = result else {
        return Err("SPARQL recovery replay requires a JSON body".to_string());
    };
    let Some(body) = value.as_object() else {
        return Err("SPARQL recovery replay has an invalid typed JSON body".to_string());
    };
    // All five report fields are required, so a successful decode plus exact
    // object length also excludes unknown fields.
    let success = body.len() == 5
        && serde_json::from_value::<
            medulla_protocol_types::result_contract::transactions::SparqlUpdateReport,
        >(value.clone())
        .is_ok();
    let compensated = body.len() == 3
        && body.get("outcome").and_then(serde_json::Value::as_str) == Some("compensated")
        && body
            .get("updated_graphs")
            .and_then(serde_json::Value::as_u64)
            == Some(0)
        && body
            .get("created_graphs")
            .and_then(serde_json::Value::as_u64)
            == Some(0);
    (success || compensated)
        .then_some(())
        .ok_or_else(|| "SPARQL recovery replay has an invalid typed JSON body".to_string())
}

/// The durable mutation batch of an admin saga with its recorded result contract.
#[cfg(feature = "redb")]
pub struct AdminSagaBatch {
    durable: MutationBatch,
    pub(crate) result_contract: Option<AdminSagaResultContract>,
}

#[cfg(feature = "redb")]
impl std::ops::Deref for AdminSagaBatch {
    type Target = MutationBatch;

    fn deref(&self) -> &Self::Target {
        &self.durable
    }
}

#[cfg(feature = "redb")]
impl Clone for AdminSagaBatch {
    fn clone(&self) -> Self {
        Self {
            durable: self.durable.clone(),
            result_contract: self.result_contract,
        }
    }
}

#[cfg(feature = "redb")]
fn validate_admin_attempt_nonce(
    authority: &CarrierAuthority,
    attempt_nonce: Option<Nonce>,
) -> Result<(), String> {
    if attempt_nonce.is_some() && attempt_nonce != authority.attempt_nonce() {
        Err("admin saga nonce does not match authenticated request".to_string())
    } else {
        Ok(())
    }
}

#[cfg(feature = "redb")]
fn require_admin_saga_execution(saga: AdminSaga, refusal: &str) -> Result<AdminSaga, String> {
    if saga.prepared {
        Err(refusal.to_string())
    } else {
        Ok(saga)
    }
}

#[cfg(feature = "redb")]
fn resolve_admin_saga_step(
    identity: &MutationScopeIdentity,
    batch: &mut MutationBatch,
    step: medulla_transaction::SagaBegin,
    expected_contract: AdminSagaResultContract,
) -> Result<(Option<crate::protocol::ResultPayload>, bool), String> {
    match step {
        medulla_transaction::SagaBegin::Committed(record) => {
            let (record, result) = decode_admin_commit(record, identity, true, expected_contract)?;
            expected_contract.validate(&result)?;
            *batch = record.batch;
            Ok((Some(result), false))
        }
        medulla_transaction::SagaBegin::Execute => Ok((None, false)),
        medulla_transaction::SagaBegin::Resume(record) => {
            *batch = record.batch;
            Ok((None, true))
        }
    }
}

/// Begin an admin saga under the current request authority.
#[cfg(feature = "redb")]
pub fn begin_admin_saga(
    backend: &dyn AdminSagaLedger,
    req_id: u64,
    caller: Option<&str>,
    method: &Method,
    domain: DurabilityDomain,
) -> Result<AdminSaga, String> {
    begin_admin_saga_with_nonce(backend, req_id, caller, method, domain, None)
}

/// Begin an admin saga under the current request authority, checking the attempt nonce.
#[cfg(feature = "redb")]
pub fn begin_admin_saga_with_nonce(
    backend: &dyn AdminSagaLedger,
    req_id: u64,
    caller: Option<&str>,
    method: &Method,
    domain: DurabilityDomain,
    attempt_nonce: Option<Nonce>,
) -> Result<AdminSaga, String> {
    let authority = current_admin_saga_authority()?;
    let _ = caller; // The verified scope is the sole durable actor authority.
    validate_admin_attempt_nonce(&authority, attempt_nonce)?;
    let saga = begin_authenticated_admin_saga(
        backend,
        req_id,
        &authority,
        method,
        domain,
        authority.attempt_nonce(),
    )?;
    require_admin_saga_execution(
        saga,
        "admin saga is Prepared; refusing to re-execute its mutation",
    )
}

/// Begin an admin saga for an already verified `authority`.
#[cfg(feature = "redb")]
pub fn begin_authenticated_admin_saga(
    backend: &dyn AdminSagaLedger,
    req_id: u64,
    authority: &CarrierAuthority,
    method: &Method,
    domain: DurabilityDomain,
    attempt_nonce: Option<Nonce>,
) -> Result<AdminSaga, String> {
    let stable_id = authority.namespace("cluster-admin-authenticated", authority.idempotency_key());
    let saga = begin_named_admin_saga_with_nonce(
        backend,
        req_id,
        Some(authority.actor_scope()),
        method,
        domain,
        &stable_id,
        attempt_nonce,
    )?;
    if let Some(result) = saga.replayed.as_ref() {
        AdminSagaResultContract::for_method(method)?.validate(result)?;
    }
    Ok(saga)
}

#[cfg(feature = "redb")]
fn validate_admin_json<T: serde::de::DeserializeOwned>(
    result: &crate::protocol::ResultPayload,
) -> Result<(), String> {
    let crate::protocol::ResultPayload::Json(value) = result else {
        return Err("admin saga replay requires a JSON body".to_string());
    };
    serde_json::from_value::<T>(value.clone())
        .map(|_| ())
        .map_err(|_| "admin saga replay has an invalid typed JSON body".to_string())
}

#[cfg(feature = "redb")]
fn public_admin_event_type(
    existing: Option<&MutationBatchRecord>,
    result_contract: AdminSagaResultContract,
) -> String {
    let legacy = existing.is_some_and(|record| {
        matches!(&record.batch.operations[..], [operation]
            if matches!(&operation.method, Method::ApplyMutation { event_type, .. }
                if event_type == "cluster_admin_operation"))
    });
    if legacy {
        "cluster_admin_operation".to_string()
    } else {
        format!(
            "{PUBLIC_ADMIN_EVENT_PREFIX}{}",
            result_contract.public_discriminator()
        )
    }
}

/// Begin the admin saga stored under `batch_id`.
#[cfg(feature = "redb")]
pub fn begin_named_admin_saga_with_nonce(
    backend: &dyn AdminSagaLedger,
    req_id: u64,
    caller: Option<&str>,
    method: &Method,
    domain: DurabilityDomain,
    batch_id: &str,
    attempt_nonce: Option<Nonce>,
) -> Result<AdminSaga, String> {
    let result_contract = AdminSagaResultContract::for_method(method)?;
    let identity = cluster_admin_scope_identity()?;
    let now = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let read = backend.admin_mutations_read()?;
    let expected = medulla_transaction::version(&read)?;
    let existing = medulla_transaction::read_ledger(&read, batch_id)?;
    let event_type = public_admin_event_type(existing.as_ref(), result_contract);
    let mut batch = crate::server::mutation_batch::compile_opaque_method(
        crate::server::mutation_batch::CompileBatch {
            batch_id,
            request_id: req_id,
            attempt_nonce,
            principal: caller,
            tenant: "native",
            graph: "cluster-admin",
            placement_epoch: 0,
            idempotency_key: batch_id,
            expected_graph_version: Some(expected),
            fencing_token: None,
            created_at_ms: now,
            default_surface: MutationSurface::Other,
            authoritative_state: None,
        },
        method,
        MutationSurface::Other,
        domain,
        &event_type,
    )?;
    let step = backend.admin_saga_step(&batch, now, None)?;
    let (replayed, prepared) =
        resolve_admin_saga_step(&identity, &mut batch, step, result_contract)?;
    Ok(AdminSaga {
        batch: AdminSagaBatch {
            durable: batch,
            result_contract: Some(result_contract),
        },
        created_at_ms: now,
        replayed,
        prepared,
    })
}

/// The sealed private payload that opens a coordinator saga.
#[cfg(feature = "redb")]
pub struct AdminSagaPayload<'a> {
    /// Durability domain of the saga.
    pub domain: DurabilityDomain,
    /// Stable batch id the saga is stored under.
    pub batch_id: &'a str,
    /// Private event type naming the result contract.
    pub event_type: &'a str,
    /// Digest of the sealed payload.
    pub payload_digest: &'a str,
    /// The sealed recovery payload bytes.
    pub encrypted_payload: &'a [u8],
}

/// Begin the admin saga stored under the payload batch id, keeping a sealed recovery payload.
#[cfg(feature = "redb")]
pub fn begin_named_admin_saga_with_private_payload_and_nonce(
    backend: &dyn AdminSagaLedger,
    req_id: u64,
    caller: Option<&str>,
    attempt_nonce: Option<Nonce>,
    payload: AdminSagaPayload<'_>,
) -> Result<AdminSaga, String> {
    let AdminSagaPayload {
        domain,
        batch_id,
        event_type,
        payload_digest,
        encrypted_payload,
    } = payload;
    let result_contract = AdminSagaResultContract::for_private_event(event_type)?;
    let identity = cluster_admin_scope_identity()?;
    let now = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let expected = medulla_transaction::version(&backend.admin_mutations_read()?)?;
    let mut batch = crate::server::mutation_batch::compile_opaque_digest(
        crate::server::mutation_batch::CompileBatch {
            batch_id,
            request_id: req_id,
            attempt_nonce,
            principal: caller,
            tenant: "native",
            graph: "cluster-admin",
            placement_epoch: 0,
            idempotency_key: batch_id,
            expected_graph_version: Some(expected),
            fencing_token: None,
            created_at_ms: now,
            default_surface: MutationSurface::Other,
            authoritative_state: None,
        },
        payload_digest,
        MutationSurface::Transaction,
        domain,
        event_type,
    )?;
    let step = backend.admin_saga_step(&batch, now, Some(encrypted_payload))?;
    let (replayed, prepared) =
        resolve_admin_saga_step(&identity, &mut batch, step, result_contract)?;
    Ok(AdminSaga {
        batch: AdminSagaBatch {
            durable: batch,
            result_contract: Some(result_contract),
        },
        created_at_ms: now,
        replayed,
        prepared,
    })
}

/// Resume the admin saga stored under `batch_id`, if one exists.
#[cfg(feature = "redb")]
pub fn resume_named_admin_saga(
    backend: &dyn AdminSagaLedger,
    batch_id: &str,
    caller: Option<&str>,
) -> Result<Option<AdminSaga>, String> {
    let expected_principal = crate::server::mutation_batch::principal_fingerprint(
        caller.ok_or_else(|| "coordinator recovery requires a verified principal".to_string())?,
    )?;
    let identity = cluster_admin_scope_identity()?;
    let Some(record) =
        medulla_transaction::read_ledger(&backend.admin_mutations_read()?, batch_id)?
    else {
        return Ok(None);
    };
    validate_admin_record(&record, &identity)?;
    validate_admin_lookup_key(&record, batch_id)?;
    let result_contract = recovered_admin_result_contract(&record.batch)?;
    // The caller lives in the outbox `actor` header, never in
    // `context.principal` -- which is now the committing ledger's serving
    // principal on every domain (RF-RULING-004 application note). A batch with
    // no header is refused rather than matched.
    if record.committing_actor()? != expected_principal {
        return Err("coordinator receipt does not match caller scope".to_string());
    }
    let replayed = match record.status {
        crate::mutation_batch::MutationBatchStatus::Prepared => None,
        crate::mutation_batch::MutationBatchStatus::Committed => {
            return resume_committed_admin_saga(record, &identity, result_contract).map(Some);
        }
        crate::mutation_batch::MutationBatchStatus::Aborted => {
            return Err("coordinator receipt was aborted".to_string())
        }
    };
    Ok(Some(AdminSaga {
        batch: AdminSagaBatch {
            durable: record.batch,
            result_contract: Some(result_contract),
        },
        created_at_ms: record.committed_at_ms,
        replayed,
        prepared: true,
    }))
}

#[cfg(feature = "redb")]
fn resume_committed_admin_saga(
    record: MutationBatchRecord,
    identity: &MutationScopeIdentity,
    result_contract: AdminSagaResultContract,
) -> Result<AdminSaga, String> {
    let (record, result) = decode_admin_commit(record, identity, true, result_contract)?;
    result_contract.validate(&result)?;
    Ok(AdminSaga {
        batch: AdminSagaBatch {
            durable: record.batch,
            result_contract: Some(result_contract),
        },
        created_at_ms: record.committed_at_ms,
        replayed: Some(result),
        prepared: false,
    })
}

#[cfg(feature = "redb")]
fn recovered_admin_result_contract(
    batch: &MutationBatch,
) -> Result<AdminSagaResultContract, String> {
    let [operation] = batch.operations.as_slice() else {
        return Err("coordinator receipt has an invalid operation inventory".to_string());
    };
    let Method::ApplyMutation { event_type, .. } = &operation.method else {
        return Err("coordinator receipt has an invalid durable operation".to_string());
    };
    if let Some(discriminator) = event_type.strip_prefix(PUBLIC_ADMIN_EVENT_PREFIX) {
        AdminSagaResultContract::for_public_discriminator(discriminator)
    } else if event_type == "cluster_admin_operation" {
        Err("public admin saga recovery is missing its result-contract discriminator".to_string())
    } else {
        AdminSagaResultContract::for_private_event(event_type)
    }
}

/// Commit `result` as the saga outcome and return the durable result.
#[cfg(feature = "redb")]
pub fn finish_admin_saga(
    backend: &dyn AdminSagaLedger,
    batch: AdminSagaBatch,
    committed_at_ms: u64,
    result: crate::protocol::ResultPayload,
) -> Result<crate::protocol::ResultPayload, String> {
    let result_contract = batch.result_contract.ok_or_else(|| {
        "admin saga recovery requires an explicit original result contract".to_string()
    })?;
    result_contract.validate(&result)?;
    let encoded = rmp_serde::to_vec_named(&DurableAdminResult::from_result(&result)?)
        .map_err(|error| error.to_string())?;
    let (record, replayed) = backend.admin_saga_end(&batch.durable, encoded, committed_at_ms)?;
    let (_, durable_result) =
        decode_admin_commit(record, &batch.identity, replayed, result_contract)?;
    result_contract.validate(&durable_result)?;
    Ok(durable_result)
}

#[cfg(feature = "redb")]
fn validate_admin_record(
    record: &MutationBatchRecord,
    expected_identity: &MutationScopeIdentity,
) -> Result<(), String> {
    record.validate()?;
    if &record.identity != expected_identity {
        return Err("admin saga receipt does not match its requested scope".to_string());
    }
    Ok(())
}

#[cfg(feature = "redb")]
fn validate_admin_lookup_key(record: &MutationBatchRecord, batch_id: &str) -> Result<(), String> {
    if record.batch.batch_id != batch_id || record.batch.idempotency_key() != batch_id {
        return Err("coordinator receipt identity is corrupt".to_string());
    }
    Ok(())
}

#[cfg(feature = "redb")]
fn decode_admin_commit(
    record: MutationBatchRecord,
    expected_identity: &MutationScopeIdentity,
    replayed: bool,
    result_contract: AdminSagaResultContract,
) -> Result<(MutationBatchRecord, crate::protocol::ResultPayload), String> {
    let commit = MutationBatchCommit {
        record,
        identity: expected_identity.clone(),
        replayed,
    };
    commit.validate()?;
    let bytes = commit
        .record
        .result_msgpack
        .as_deref()
        .ok_or_else(|| "committed admin saga has no result".to_string())?;
    let result = decode_admin_result(bytes, result_contract)?;
    Ok((commit.record, result))
}

/// Decode a saga result stored in the tagged durable encoding, within
/// `limits`. Transaction commits persist their outcome through the same saga
/// ledger, so their receipts must be read back this way too.
#[cfg(feature = "redb")]
pub fn decode_durable_saga_result(
    bytes: &[u8],
    limits: medulla_types::msgpack::MsgpackLimits,
) -> Option<crate::protocol::ResultPayload> {
    medulla_types::msgpack::decode_bounded::<DurableAdminResult>(bytes, limits)
        .ok()
        .map(DurableAdminResult::into_result)
}

#[cfg(feature = "redb")]
fn decode_admin_result(
    bytes: &[u8],
    result_contract: AdminSagaResultContract,
) -> Result<crate::protocol::ResultPayload, String> {
    let result = match rmp_serde::from_slice::<DurableAdminResult>(bytes) {
        Ok(result) => result.into_result(),
        Err(_) => {
            // Receipts written before the tagged encoding used the untagged
            // wire enum. Its only declared scalar JSON result is a channel
            // departure; recover that exact legacy representation.
            let legacy: crate::protocol::ResultPayload =
                rmp_serde::from_slice(bytes).map_err(|error| error.to_string())?;
            match (result_contract, legacy) {
                (
                    AdminSagaResultContract::ChannelDeparture,
                    crate::protocol::ResultPayload::String(value),
                ) => crate::protocol::ResultPayload::Json(serde_json::Value::String(value)),
                (_, result) => result,
            }
        }
    };
    Ok(result)
}

#[cfg(test)]
#[path = "saga_tests.rs"]
mod saga_tests;
