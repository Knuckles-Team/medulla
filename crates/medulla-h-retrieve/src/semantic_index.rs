//! Dispatch handler for `Method::SemanticIndex` -- the S1-S6 tiered semantic
//! ingestion queue.
//!
//! This is the entry point an external connector actually reaches. Everything
//! that carries authority is derived HERE from the verified request context and
//! never read from the request body: the acting agent, the attempt nonce, the
//! admission clock, the owner handle, and the authoritative SQL source page.
//!
//! One shape to notice, because it is the reason the pipeline is drivable at
//! all: a stage completion is not "tell the engine you finished". It is a
//! leased, fenced, predecessor-proved transition, and the durable store refuses
//! it if the predecessor proof is absent. The handler's job is to bind the
//! caller to a lease and hand the transition to the store; it never decides
//! whether a stage may complete.

#![cfg(all(feature = "ann-redb", feature = "query"))]

#[path = "semantic_index/binding.rs"]
mod binding;
#[path = "semantic_index/contracts.rs"]
mod contracts;
#[path = "semantic_index/dispatch.rs"]
mod dispatch;
#[path = "semantic_index/reads.rs"]
mod reads;
#[path = "semantic_index/search.rs"]
pub mod search;
#[path = "semantic_index/source.rs"]
mod source;
#[path = "semantic_index/worker.rs"]
mod worker;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use medulla_ann_codes::semantic_ann_codes::SemanticCodeError;
use medulla_ann_codes::semantic_index_service::SemanticIndexService;
use medulla_index_types::embedding::EmbeddingSpaceRef;
use medulla_index_types::{
    semantic_index::SemanticBinding, semantic_index::SemanticIndexError,
    semantic_index::SemanticIndexOp, semantic_index::SemanticSourceSelector,
};
use tokio::sync::RwLock;

use crate::semantic_index_runtime::graph_source::{GraphSourceProvider, GraphSourceSnapshot};
use crate::semantic_index_runtime::open_semantic_service;
use crate::semantic_index_runtime::semantic_cursor_secret;
use crate::semantic_index_runtime::semantic_failure_message;
use crate::semantic_index_runtime::SemanticIndexServerAdapter;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::carrier::CarrierAuthority;
use medulla_server_api::state::ServerState;

pub(super) struct SemanticIndexContext<'a> {
    pub(super) req_id: u64,
    pub(super) authority: &'a CarrierAuthority,
    pub(super) persist_dir: &'a Path,
    pub(super) service: Arc<SemanticIndexService>,
    pub(super) now_ms: u64,
    /// The shared retrieval document-state store, resolved for a document commit
    /// (MEDULLA-FEDERATED-QUERY-R081.101.2).
    #[cfg(feature = "query")]
    pub(super) doc_state: Option<Arc<medulla_persistence::doc_state_store::DocumentStateStore>>,
    /// The caller's visible nodes of the binding's graph text property, read once for a source
    /// operation over a graph-sourced binding (MEDULLA-MEDIA-VOCAB-R001.102.2).
    pub(super) graph_source: Option<Arc<GraphSourceSnapshot>>,
}

/// Route one semantic-index operation.
pub async fn handle_semantic_index(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    graph_sources: &dyn GraphSourceProvider,
    bound_space: &(dyn Fn() -> Option<EmbeddingSpaceRef> + Sync),
    op: Box<SemanticIndexOp>,
) -> Response {
    let op = *op;
    let authority = match authorize(req_id, verified, &op) {
        Ok(authority) => authority,
        Err(response) => return *response,
    };
    let (persist_dir, service) = match open_service(state, req_id, &authority, &op).await {
        Ok(opened) => opened,
        Err(response) => return response,
    };
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    #[cfg(feature = "query")]
    let doc_state = if matches!(op, SemanticIndexOp::CompleteSqlSourceStage { .. })
        && medulla_server_api::knob_config::knobs().retrieval_state_builtin_encoder
    {
        let guard = state.read().await;
        medulla_persistence::doc_state_store::shared_doc_state_store(&guard).ok()
    } else {
        None
    };
    // A graph-sourced binding is woken by graph commits, which find it in the bound-source
    // registry; register it on every operation so a restart restores it, and again after a
    // mutation so a binding admitted by this very operation is registered.
    let bound_service = Arc::clone(&service);
    let mutation = op.is_mutation();
    if let Err(response) = register_graph_source(req_id, state, &bound_service).await {
        return response;
    }
    let graph_source = match graph_source_for(req_id, &service, &op, graph_sources).await {
        Ok(snapshot) => snapshot,
        Err(response) => return response,
    };
    let context = SemanticIndexContext {
        req_id,
        authority: &authority,
        persist_dir: &persist_dir,
        service,
        now_ms,
        #[cfg(feature = "query")]
        doc_state,
        graph_source,
    };
    let completes_generation = matches!(op, SemanticIndexOp::CompleteGenerationStage { .. });
    let admits_binding = matches!(op, SemanticIndexOp::AdmitBinding { .. });
    let response = dispatch::handle(&context, op).await;
    // The engine's S6 worker consumes the S6 intent that S5 publishes, so its consumer is
    // installed when the binding is admitted, before any stage can publish that intent.
    if admits_binding && response.error.is_none() {
        let worker = Arc::clone(&bound_service);
        if let Err(response) = blocking(req_id, move || worker.subscribe_s6_worker()).await {
            return response;
        }
    }
    if completes_generation && response.error.is_none() {
        run_s6_worker(&bound_service, bound_space, now_ms).await;
    }
    if mutation {
        if let Err(failure) = register_graph_source(req_id, state, &bound_service).await {
            return failure;
        }
    }
    response
}

/// Run the engine's S6 stage worker once after a generation stage completes. It activates the
/// generation when S5 has just published the S6 intent and the bound text embedder's pinned
/// space is the binding's model space (the one `pin_embedding_space` feeds the binding and the
/// image). A refusal, a blocked promotion included, is logged: the stage completion that woke
/// the worker has already committed, and the prior generation stays live.
async fn run_s6_worker(
    service: &Arc<SemanticIndexService>,
    bound_space: &(dyn Fn() -> Option<EmbeddingSpaceRef> + Sync),
    now_ms: u64,
) {
    let Some(space) = bound_space() else {
        return;
    };
    let service = Arc::clone(service);
    let outcome = tokio::task::spawn_blocking(move || {
        let Some(binding) = service.binding()? else {
            return Ok(None);
        };
        if binding.model_digest != space.digest {
            return Ok(None);
        }
        service.run_s6_stage(&space, now_ms)
    })
    .await;
    match outcome {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            tracing::warn!(%error, "the S6 stage worker did not activate the generation")
        }
        Err(_) => tracing::warn!("the S6 stage worker could not be scheduled"),
    }
}

/// Record the binding's graph text property, when it has one, in the registry the graph commit
/// chokepoint reads to decide which commits wake a semantic index, and in the owner table the
/// registry is rebuilt from at startup.
async fn register_graph_source(
    req_id: u64,
    state: &Arc<RwLock<ServerState>>,
    service: &Arc<SemanticIndexService>,
) -> Result<(), Response> {
    let service = Arc::clone(service);
    let Some(binding) = blocking(req_id, move || service.binding()).await? else {
        return Ok(());
    };
    let SemanticSourceSelector::GraphTextPropertyRef(selector) = &binding.source_selector else {
        return Ok(());
    };
    let recorded = {
        let guard = state.read().await;
        crate::semantic_index_runtime::record_graph_source_binding(
            &guard,
            &binding.tenant_id,
            &binding.binding_id,
            selector,
        )
    };
    recorded.map_err(|error| {
        Response::err(
            req_id,
            format!("ENGINE_UNAVAILABLE: graph source binding registry: {error}"),
        )
    })?;
    medulla_index_types::semantic_index::bind_graph_source(&binding.tenant_id, selector);
    Ok(())
}

/// The graph snapshot a source operation over a graph-sourced binding reads. Every other
/// operation, and every SQL-sourced binding, reads none.
async fn graph_source_for(
    req_id: u64,
    service: &Arc<SemanticIndexService>,
    op: &SemanticIndexOp,
    provider: &dyn GraphSourceProvider,
) -> Result<Option<Arc<GraphSourceSnapshot>>, Response> {
    if !matches!(
        op,
        SemanticIndexOp::AdmitSourceRecord { .. }
            | SemanticIndexOp::AdmitSourcePage { .. }
            | SemanticIndexOp::AdmitSourceReconcile { .. }
            | SemanticIndexOp::CompleteSqlSourceStage { .. }
    ) {
        return Ok(None);
    }
    let binding_service = Arc::clone(service);
    let Ok(Some(binding)) = blocking(req_id, move || binding_service.binding()).await else {
        return Ok(None);
    };
    let SemanticSourceSelector::GraphTextPropertyRef(selector) = &binding.source_selector else {
        return Ok(None);
    };
    provider
        .snapshot(selector)
        .await
        .map(|snapshot| Some(Arc::new(snapshot)))
        .map_err(|error| Response::err(req_id, error))
}

fn authorize(
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    op: &SemanticIndexOp,
) -> Result<CarrierAuthority, Box<Response>> {
    // Echo only the refused shape field and its static reason, never the
    // submitted value.
    match op.validate() {
        Ok(()) => {}
        Err(SemanticIndexError::InvalidField { field, reason }) => {
            return Err(Box::new(Response::err(
                req_id,
                format!("INVALID_ARGUMENT: semantic operation rejected: `{field}` {reason}"),
            )));
        }
        Err(_) => {
            return Err(Box::new(Response::err(
                req_id,
                "INVALID_ARGUMENT: semantic operation rejected",
            )));
        }
    }
    // Tenant isolation, checked ONCE rather than per arm, keeps a binding id
    // from becoming an execution grant across tenants.
    // Public request DTOs carry the opaque tenant scope and are bound to the
    // verified carrier by the adapter below. Internal worker ops carry the
    // raw tenant name, so their existing comparison stays here.
    if !matches!(op, SemanticIndexOp::GetBindingRequest { .. })
        && op.tenant_id() != verified.tenant()
    {
        return Err(Box::new(Response::err(
            req_id,
            "ACCESS_DENIED: semantic index tenant must match verified request tenant",
        )));
    }
    let authority = CarrierAuthority::from_verified(verified)
        .map_err(|error| Box::new(Response::err(req_id, error)))?;
    // The carrier's own capability is checked in addition to the ledger gate.
    if op.is_mutation() && !authority.can_write() {
        return Err(Box::new(Response::err(
            req_id,
            "ACCESS_DENIED: semantic index mutation requires kg:write",
        )));
    }
    if !op.is_mutation() && !authority.can_read() {
        return Err(Box::new(Response::err(
            req_id,
            "ACCESS_DENIED: semantic index read requires kg:read",
        )));
    }
    if let SemanticIndexOp::GetBindingRequest { request } = op {
        SemanticIndexServerAdapter::authorize_request(
            request,
            &authority,
            medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
        )
        .map_err(|error| Box::new(Response::err(req_id, error)))?;
    }
    Ok(authority)
}

async fn open_service(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    authority: &CarrierAuthority,
    op: &SemanticIndexOp,
) -> Result<(PathBuf, Arc<SemanticIndexService>), Response> {
    let persist_dir: PathBuf = {
        let guard = state.read().await;
        match guard.persist_dir.as_deref() {
            Some(dir) => PathBuf::from(dir),
            None => {
                return Err(Response::err(
                    req_id,
                    "ENGINE_UNAVAILABLE: semantic index storage is unavailable",
                ));
            }
        }
    };
    // The durable owner uses the carrier's opaque tenant scope, while the
    // wire tenant was checked above against the verified request tenant.
    let service = open_semantic_service(&persist_dir, authority.tenant_scope(), op.binding_id())
        .map_err(|_| {
            Response::err(
                req_id,
                "ENGINE_UNAVAILABLE: semantic index storage is unavailable",
            )
        })?;
    Ok((persist_dir, service))
}

/// Replace a draft's identity fields with the verified carrier's.
///
/// The same move `handle_agent_component` makes on a published component: the
/// caller SENDS these fields, so they are claims, and a claim that is used
/// unchecked is an assertion. Overwriting is stronger than comparing -- there is
/// no path on which a draft reaches admission carrying an identity the carrier
/// did not prove.
pub(super) fn stamp_draft_identity(
    draft: &mut medulla_index_types::semantic_index::SemanticBindingDraft,
    authority: &CarrierAuthority,
) {
    draft.tenant_id = authority.tenant_scope().to_string();
    draft.actor_scope = authority.actor_scope().to_string();
    draft.effective_actor_scope = authority.agent_id().to_string();
}

/// A lease may only be presented by the worker whose class consumer it was
/// issued to (`<worker>#<class>`).
pub(crate) fn own_lease(
    lease: &medulla_mutation_types::mutation_scope::MutationOutboxLease,
    authority: &CarrierAuthority,
) -> Result<(), String> {
    let owner = medulla_ann_codes::semantic_ann_codes::stage_consumer_parts(&lease.consumer)
        .map(|(worker, _)| worker);
    if owner != Some(authority.agent_id()) {
        return Err(
            "ACCESS_DENIED: semantic lease owner does not match verified carrier".to_string(),
        );
    }
    Ok(())
}

pub(super) fn read_port(
    ctx: &SemanticIndexContext<'_>,
) -> crate::semantic_index_runtime::AuthorizedSqlSourceReadPort {
    crate::semantic_index_runtime::AuthorizedSqlSourceReadPort::new(
        ctx.persist_dir.to_path_buf(),
        ctx.authority.clone(),
        semantic_cursor_secret(),
    )
    .with_graph_source(ctx.graph_source.clone())
}

/// Decode one opaque engine-minted cursor. Hex on the wire, bytes inside; the
/// bytes themselves are MAC-bound to the tenant that was issued them, so a
/// decoded cursor from another tenant fails its MAC rather than resuming.
pub(super) fn decode_cursor(cursor: Option<String>) -> Result<Option<Vec<u8>>, String> {
    match cursor {
        None => Ok(None),
        Some(cursor) => hex::decode(&cursor)
            .map(Some)
            .map_err(|_| "INVALID_ARGUMENT: semantic cursor is invalid".to_string()),
    }
}

pub(super) async fn current_binding(
    req_id: u64,
    service: &Arc<SemanticIndexService>,
) -> Result<SemanticBinding, Response> {
    let service = Arc::clone(service);
    match blocking(req_id, move || service.binding()).await {
        Ok(Some(binding)) => Ok(binding),
        Ok(None) => Err(Response::err(
            req_id,
            "CONFLICT: semantic binding is unavailable",
        )),
        Err(response) => Err(response),
    }
}

/// Marker for the one operation whose success carries no payload of its own.
pub(super) fn consumer_ack() -> bool {
    true
}

/// Run one synchronous owner operation off the async worker.
pub(super) async fn blocking<T, F>(req_id: u64, work: F) -> Result<T, Response>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, medulla_ann_codes::semantic_ann_codes::SemanticCodeError>
        + Send
        + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(semantic_failure_response(req_id, error)),
        Err(_) => Err(Response::err(
            req_id,
            "ENGINE_UNAVAILABLE: semantic index operation could not be scheduled",
        )),
    }
}

pub(crate) fn semantic_failure_response(req_id: u64, error: SemanticCodeError) -> Response {
    Response::err(req_id, semantic_failure_message(&error))
}

pub(super) fn reply<M, T>(req_id: u64, result: Result<T, Response>) -> Response
where
    M: medulla_protocol_types::result_contract::MethodResult<Body = T>,
    M::Encoding: medulla_protocol_types::result_contract::EncodeRef<T>,
{
    match result {
        Ok(value) => typed_payload::<M, _>(req_id, &value),
        Err(response) => response,
    }
}

pub(super) fn typed_payload<M, T>(req_id: u64, value: &T) -> Response
where
    M: medulla_protocol_types::result_contract::MethodResult<Body = T>,
    M::Encoding: medulla_protocol_types::result_contract::EncodeRef<T>,
{
    match ResultPayload::of_ref::<M>(value) {
        Ok(payload) => Response::ok(req_id, payload),
        Err(_) => Response::err(req_id, "INTERNAL: semantic result encoding failed"),
    }
}

#[cfg(test)]
mod error_contract_tests {
    use super::{decode_cursor, semantic_failure_response};
    use medulla_ann_codes::semantic_ann_codes::SemanticCodeError;

    #[test]
    fn worker_errors_use_declared_codes_without_backend_text() {
        for (error, expected) in [
            (
                SemanticCodeError::Refused("private lease".into()),
                "CONFLICT",
            ),
            (
                SemanticCodeError::Kernel("private path".into()),
                "ENGINE_UNAVAILABLE",
            ),
            (SemanticCodeError::Corrupt("private row".into()), "INTERNAL"),
            (
                SemanticCodeError::Io(std::io::Error::other("private io")),
                "ENGINE_UNAVAILABLE",
            ),
        ] {
            let response = semantic_failure_response(23, error);
            assert_eq!(response.error.as_deref(), Some(expected));
            assert!(!response
                .error_detail
                .as_deref()
                .unwrap_or_default()
                .contains("private"));
            assert!(medulla_capabilities::error_routing::method_allows_error(
                "SemanticIndex",
                expected
            ));
        }
        assert_eq!(
            decode_cursor(Some("not hex".into())).unwrap_err(),
            "INVALID_ARGUMENT: semantic cursor is invalid"
        );
    }
}
