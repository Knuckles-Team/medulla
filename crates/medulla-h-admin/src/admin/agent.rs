//! Authenticated Agent Library operation handlers.

use std::sync::Arc;

use tokio::sync::RwLock;

#[cfg(feature = "redb")]
use medulla_persistence_records::agent_library::context::bind_agent_library_context;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::state::ServerState;

/// Bind the context of a read-only Agent Library status query. Status never
/// reaches the mutation kernel, so no attempt nonce is required; a binding
/// failure is answered as this request's error response.
#[cfg(feature = "redb")]
fn bind_status_context(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    context: medulla_agent_types::agent_library::AgentLibraryMutationContext,
    purpose_id: &str,
) -> Result<medulla_agent_types::agent_library::AgentLibraryMutationContext, Response> {
    bind_agent_library_context(store, req_id, verified, context, purpose_id, false)
        .map_err(|error| Response::err(req_id, error))
}

#[cfg(feature = "redb")]
pub fn bind_agent_library_draft(
    draft: medulla_agent_types::agent_library::AgentLibraryEntryDraft,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    _action_context: &medulla_agent_types::agent_library::AgentLibraryMutationContext,
) -> Result<medulla_agent_types::agent_library::AgentLibraryEntryDraft, String> {
    if draft.tenant_id != verified.tenant() {
        return Err(
            "ACCESS_DENIED: Agent Library definition tenant must match verified request tenant"
                .to_string(),
        );
    }
    // `draft.agent_id` names the selected built definition. Its actor scope,
    // purpose, and policy digest are immutable definition provenance and must
    // survive a later action by another caller under a newer policy. The
    // authenticated request's action authority is carried separately in the
    // mutation context and outbox event; body attribution never authorizes the
    // action or replaces historical definition fields.
    Ok(draft)
}

#[cfg(feature = "redb")]
async fn ensure_agent_library(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
) -> Result<Arc<medulla_persistence_records::agent_library::AgentLibraryStore>, Response> {
    medulla_persistence_records::agent_library::shared_agent_library(state)
        .await
        .map_err(|error| Response::err(req_id, error))
}

#[cfg(feature = "redb")]
fn validate_component_op(
    op: &medulla_agent_types::agent_component::AgentComponentOp,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
) -> Result<(), String> {
    op.validate()?;
    if op.tenant_id() != verified.tenant() {
        return Err(
            "ACCESS_DENIED: agent component tenant must match verified request tenant".to_string(),
        );
    }
    Ok(())
}

#[cfg(feature = "redb")]
fn validate_template_op(
    op: &medulla_agent_types::agent_template::AgentTemplateOp,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
) -> Result<(), String> {
    op.validate()?;
    if op.tenant_id() != verified.tenant() {
        return Err(
            "ACCESS_DENIED: agent template tenant must match verified request tenant".to_string(),
        );
    }
    Ok(())
}

/// Serve the typed RF-020 Agent Library operations from the same authenticated
/// dispatch boundary as every other self-routing control-plane method.
#[cfg(feature = "redb")]
/// Publish, retire, inspect or SEARCH one durable agent component
/// (RF-ADR-008 layer 1).
///
/// Same store as the two layers above it. `Search` is the capability query the
/// layer exists for -- it resolves a task through EG's native ontology and
/// matches components by subsumption, so the answer comes from the graph rather
/// than from a model.
pub async fn handle_agent_component(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    op: medulla_agent_types::agent_component::AgentComponentOp,
) -> Response {
    use medulla_agent_types::agent_component::AgentComponentOp;

    if let Err(error) = validate_component_op(&op, verified) {
        return Response::err(req_id, error);
    }
    let store = match ensure_agent_library(state, req_id).await {
        Ok(store) => store,
        Err(response) => return response,
    };
    match op {
        AgentComponentOp::Publish { request } => {
            component_publish(state, &store, req_id, verified, request).await
        }
        AgentComponentOp::Retire { request } => component_retire(&store, req_id, verified, request),
        AgentComponentOp::Current {
            tenant_id,
            component_id,
        } => component_current(&store, req_id, tenant_id, component_id),
        AgentComponentOp::History {
            tenant_id,
            component_id,
        } => component_history(&store, req_id, tenant_id, component_id),
        AgentComponentOp::Status { request } => component_status(&store, req_id, verified, request),
        AgentComponentOp::Search { request } => component_search(&store, req_id, request),
        AgentComponentOp::Content { request } => {
            super::component_content::handle_component_content(
                state, &store, req_id, verified, request,
            )
            .await
        }
        record @ AgentComponentOp::RecordSourceEffect { .. } => {
            component_record_source_effect(&store, req_id, &record)
        }
        AgentComponentOp::ExportBlueprint { request } => {
            super::component_content::serve_blueprint_export(
                state, &store, req_id, verified, request,
            )
            .await
        }
        AgentComponentOp::ImportBlueprint { request } => {
            super::component_content::serve_blueprint_import(
                state, &store, req_id, verified, *request,
            )
            .await
        }
        AgentComponentOp::EditSkillSource { request } => {
            super::skill_source_edit::serve_skill_source_edit(
                state, &store, req_id, verified, *request,
            )
            .await
        }
        AgentComponentOp::StoreProposal { request } => {
            match store.store_proposal(
                &request.context.tenant_id,
                &request.component_id,
                &request.proposal,
            ) {
                Ok(result) => Response::ok(
                    req_id,
                    ResultPayload::of_ref::<
                        medulla_protocol_types::result_contract::storage::AgentComponentStoreProposal,
                    >(&result),
                ),
                Err(error) => Response::err(req_id, error),
            }
        }
        AgentComponentOp::Render { request } => {
            super::component_content::serve_prompt_render(state, &store, req_id, verified, request)
                .await
        }
        AgentComponentOp::VerifyRender { receipt } => {
            super::prompt_receipt::serve_verify_render(&store, req_id, verified, &receipt)
        }
        AgentComponentOp::AdmitSkillUpgrade { request } => {
            super::skill_admission::handle_admit_skill_upgrade(&store, req_id, request)
        }
    }
}

/// `AgentComponent.record_source_effect`: keep the receipt of an approved
/// repository-source effect on the revision it exported (MEDULLA-TYPED-PACKS-R094.8).
#[cfg(feature = "redb")]
fn component_record_source_effect(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    op: &medulla_agent_types::agent_component::AgentComponentOp,
) -> Response {
    let Some(receipt) = op.source_effect_receipt() else {
        return Response::err(req_id, "INVALID_ARGUMENT: not a source-effect receipt");
    };
    match store.record_source_effect(&receipt) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentRecordSourceEffect,
            >(&result),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// `AgentComponent.publish`: a pinned system prompt (or a skill source) stages its body in the engine
/// body store, then the owner write commits revision, head, body holder and receipt
/// together (MEDULLA-TYPED-PACKS-R002.7).
#[cfg(feature = "redb")]
pub(crate) async fn component_publish(
    state: &Arc<RwLock<ServerState>>,
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    mut request: Box<medulla_agent_types::agent_component::AgentComponentPublishRequest>,
) -> Response {
    let context = match bind_agent_library_context(
        store,
        req_id,
        verified,
        request.context,
        "agent-component:publish",
        true,
    ) {
        Ok(context) => context,
        Err(error) => return Response::err(req_id, error),
    };
    request.component.tenant_id = context.tenant_id.clone();
    request.component.actor_scope = context.actor_scope.clone();
    request.component.purpose_id = context.purpose_id.clone();
    request.component.policy_digest = context.policy_digest.clone();
    request.context = context;
    let staged = match stage_component_body(state, &mut request).await {
        Ok(staged) => staged,
        Err(error) => return Response::err(req_id, error),
    };
    match store.publish_component(*request, staged) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentPublish,
            >(&result.result),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// Check the body a publish carries against its pins, then stage its bytes in the engine
/// body store. Returns the holder the owner write commits with the revision, or `None` for a
/// publish that carries no body. No refusal before the staging call stages a body.
///
/// One mechanism for both body kinds (MEDULLA-TYPED-PACKS-R100.100): a pinned system prompt
/// stages its blueprint, a skill stages its SKILL.md source.
#[cfg(feature = "redb")]
async fn stage_component_body(
    state: &Arc<RwLock<ServerState>>,
    request: &mut medulla_agent_types::agent_component::AgentComponentPublishRequest,
) -> Result<Option<medulla_persistence_records::connector_pack::commit::ConnectorPackHolderCommit>, String>
{
    use medulla_agent_types::agent_component::facts::PROMPT_SCHEMA_INVALID;
    use medulla_agent_types::agent_component::{
        AgentComponentFacts, AgentComponentKind, ComponentBody, MAX_COMPONENT_BODY_BYTES,
    };

    let pin = match &request.component.facts {
        AgentComponentFacts::SystemPrompt {
            body_sha256: Some(hex),
            ..
        } => Some(hex.clone()),
        _ => None,
    };
    let expected_revision = request
        .context
        .expected_revision
        .ok_or_else(|| "agent component writes require an explicit expected_revision".to_string())?;
    let entry_revision = expected_revision + 1;
    match (pin, request.body.clone()) {
        (None, None) => Ok(None),
        (Some(_), None) => Err(format!(
            "{PROMPT_SCHEMA_INVALID}: a pinned system prompt publish carries its blueprint"
        )),
        (None, Some(ComponentBody::PromptBlueprint(_))) => Err(format!(
            "{PROMPT_SCHEMA_INVALID}: only a pinned system prompt carries a blueprint"
        )),
        (Some(_), Some(ComponentBody::SkillSource(_))) => Err(format!(
            "{PROMPT_SCHEMA_INVALID}: a pinned system prompt carries a blueprint, not a skill source"
        )),
        (Some(pin), Some(ComponentBody::PromptBlueprint(blueprint))) => {
            stage_prompt_body(state, request, entry_revision, &pin, blueprint).await
        }
        (None, Some(ComponentBody::SkillSource(source))) => {
            if request.component.kind != AgentComponentKind::Skill {
                return Err(
                    "INVALID_ARGUMENT: only a skill component carries a skill source".to_string(),
                );
            }
            if source.len() > MAX_COMPONENT_BODY_BYTES {
                return Err(
                    "SKILL_SPAN_UNKNOWN: the skill source is over the component body limit"
                        .to_string(),
                );
            }
            let sha256 = medulla_types::contract::Digest256::sha256(source.as_bytes());
            let hex = sha256.to_hex();
            if request.component.content_digest != format!("sha256:{hex}") {
                return Err(
                    "BODY_DIGEST_MISMATCH: the skill source bytes differ from the content digest"
                        .to_string(),
                );
            }
            request.component.content_ref = Some(format!("eg-body:sha256:{hex}"));
            stage_engine_body(
                state,
                &request.context.tenant_id,
                &request.component.component_id,
                entry_revision,
                sha256,
                source.into_bytes(),
                request.context.created_at_ms,
            )
            .await
            .map(Some)
        }
    }
}

/// The blueprint of a pinned system prompt, checked against its pin and the bounded profile.
#[cfg(feature = "redb")]
async fn stage_prompt_body(
    state: &Arc<RwLock<ServerState>>,
    request: &mut medulla_agent_types::agent_component::AgentComponentPublishRequest,
    entry_revision: u64,
    pin: &str,
    blueprint: String,
) -> Result<Option<medulla_persistence_records::connector_pack::commit::ConnectorPackHolderCommit>, String>
{
    use medulla_agent_types::agent_component::facts::{
        validate_prompt_blueprint, PromptLimits, PROMPT_SCHEMA_INVALID,
    };
    use medulla_agent_types::agent_component::PromptBlueprintIdentity;

    let sha256 = medulla_types::contract::Digest256::sha256(blueprint.as_bytes());
    let hex = sha256.to_hex();
    if hex != pin || request.component.content_digest != format!("sha256:{hex}") {
        return Err(
            "BODY_DIGEST_MISMATCH: the prompt blueprint bytes differ from the pinned digest"
                .to_string(),
        );
    }
    request.component.content_ref = Some(format!("eg-body:sha256:{hex}"));
    let value: serde_json::Value = serde_json::from_str(&blueprint)
        .map_err(|error| format!("{PROMPT_SCHEMA_INVALID}: blueprint is not JSON: {error}"))?;
    let identity = PromptBlueprintIdentity::derive(
        &request.context.tenant_id,
        &request.component.component_id,
        entry_revision,
        &request.component.facts,
        &request.component.provenance,
    )?;
    validate_prompt_blueprint(&value, &identity, &PromptLimits::default())
        .map_err(|code| format!("{code}: the prompt blueprint is not admitted"))?;
    stage_engine_body(
        state,
        &request.context.tenant_id,
        &request.component.component_id,
        entry_revision,
        sha256,
        blueprint.into_bytes(),
        request.context.created_at_ms,
    )
    .await
    .map(Some)
}

#[cfg(all(feature = "redb", feature = "blob"))]
async fn stage_engine_body(
    state: &Arc<RwLock<ServerState>>,
    tenant_id: &str,
    component_id: &str,
    entry_revision: u64,
    sha256: medulla_types::contract::Digest256,
    body: Vec<u8>,
    created_at_ms: u64,
) -> Result<medulla_persistence_records::connector_pack::commit::ConnectorPackHolderCommit, String>
{
    use medulla_server_blob::blob::BlobStateExt;

    let blob = state.read().await.blob_cursors().cloned().ok_or_else(|| {
        "COMPONENT_BODY_UNAVAILABLE: the engine body store is disabled".to_string()
    })?;
    let tenant = tenant_id.to_string();
    let staged = tokio::task::spawn_blocking(move || {
        blob.store.put_engine_bodies(
            &tenant,
            &[medulla_server_blob::blob::engine_bodies::EngineBody { sha256, body }],
            created_at_ms,
        )
    })
    .await
    .map_err(|error| format!("component body staging task failed: {error}"))??;
    let stored = staged.into_iter().next().ok_or_else(|| {
        "COMPONENT_BODY_UNAVAILABLE: the engine body store staged no body".to_string()
    })?;
    Ok(
        medulla_persistence_records::connector_pack::commit::ConnectorPackHolderCommit {
            body_sha256: sha256,
            component_id: component_id.to_string(),
            entry_revision,
            row: medulla_persistence_records::connector_pack::ConnectorPackBodyHolderRow {
                engine_manifest_digest: stored.manifest_digest,
                length: stored.length,
            },
        },
    )
}

#[cfg(all(feature = "redb", not(feature = "blob")))]
async fn stage_engine_body(
    _state: &Arc<RwLock<ServerState>>,
    _tenant_id: &str,
    _component_id: &str,
    _entry_revision: u64,
    _sha256: medulla_types::contract::Digest256,
    _body: Vec<u8>,
    _created_at_ms: u64,
) -> Result<medulla_persistence_records::connector_pack::commit::ConnectorPackHolderCommit, String>
{
    Err("COMPONENT_BODY_UNAVAILABLE: the engine body store is disabled".to_string())
}

#[cfg(feature = "redb")]
fn component_retire(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    mut request: medulla_agent_types::agent_component::AgentComponentRetireRequest,
) -> Response {
    let context = match bind_agent_library_context(
        store,
        req_id,
        verified,
        request.context,
        "agent-component:retire",
        true,
    ) {
        Ok(context) => context,
        Err(error) => return Response::err(req_id, error),
    };
    request.context = context;
    match store.retire_component(request) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentRetire,
            >(&result.result),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn component_current(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    tenant_id: String,
    component_id: String,
) -> Response {
    match store.current_component(&tenant_id, &component_id) {
        Ok(entry) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentCurrent,
            >(&entry),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn component_history(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    tenant_id: String,
    component_id: String,
) -> Response {
    match store.component_revisions(&tenant_id, &component_id) {
        Ok(entries) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentHistory,
            >(&entries),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// The `bind_agent_library_context` purpose for a status read, named by the
/// mutation kind it is reporting on. A pack member's withdrawal and its
/// return are importer actions, not caller ones; the status read still has
/// to name them so a new kind cannot inherit an unrelated purpose.
///
/// A free function rather than a match inline in `component_status`: the
/// kind enum is exhaustive by design (RF-ADR-010), and every variant this
/// module adds would otherwise read as `component_status` itself getting
/// more complex, when the dispatch is the whole story.
#[cfg(feature = "redb")]
fn component_status_purpose(
    kind: medulla_agent_types::agent_component::AgentComponentMutationKind,
) -> &'static str {
    match kind {
        medulla_agent_types::agent_component::AgentComponentMutationKind::Publish => {
            "agent-component:publish"
        }
        medulla_agent_types::agent_component::AgentComponentMutationKind::Retire => {
            "agent-component:retire"
        }
        medulla_agent_types::agent_component::AgentComponentMutationKind::Withdraw => {
            "agent-component:withdraw"
        }
        medulla_agent_types::agent_component::AgentComponentMutationKind::Republish => {
            "agent-component:republish"
        }
    }
}

#[cfg(feature = "redb")]
fn component_status(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    mut request: medulla_agent_types::agent_component::AgentComponentStatusRequest,
) -> Response {
    let purpose = component_status_purpose(request.kind);
    request.context = match bind_status_context(store, req_id, verified, request.context, purpose) {
        Ok(context) => context,
        Err(response) => return response,
    };
    match store.component_status(request) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentStatus,
            >(&result.map(|result| result.result)),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn component_search(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    request: medulla_agent_types::agent_component::AgentComponentSearchRequest,
) -> Response {
    match store.search_components(&request) {
        Ok(page) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentSearch,
            >(&page),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// Publish, retire, inspect or INSTANTIATE one durable agent template
/// (RF-ADR-008 item C).
///
/// Same store as the three layers beside it. `Instantiate` is the operation
/// the layer exists for, and it is a READ: it binds parameters and returns an
/// ordinary `AgentLibraryEntryDraft`, which the caller then publishes through
/// `Method::AgentLibrary`. Nothing here commits an instance, so the library
/// write privilege is still what admits one.
#[cfg(feature = "redb")]
pub async fn handle_agent_template(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    op: medulla_agent_types::agent_template::AgentTemplateOp,
) -> Response {
    use medulla_agent_types::agent_template::AgentTemplateOp;

    if let Err(error) = validate_template_op(&op, verified) {
        return Response::err(req_id, error);
    }
    let store = match ensure_agent_library(state, req_id).await {
        Ok(store) => store,
        Err(response) => return response,
    };
    match op {
        AgentTemplateOp::Publish { request } => template_publish(&store, req_id, verified, request),
        AgentTemplateOp::Retire { request } => template_retire(&store, req_id, verified, request),
        AgentTemplateOp::Current {
            tenant_id,
            template_id,
        } => template_current(&store, req_id, tenant_id, template_id),
        AgentTemplateOp::History {
            tenant_id,
            template_id,
        } => template_history(&store, req_id, tenant_id, template_id),
        AgentTemplateOp::Status { request } => template_status(&store, req_id, verified, request),
        AgentTemplateOp::Instantiate { request } => template_instantiate(&store, req_id, request),
        AgentTemplateOp::List { request } => template_list(&store, req_id, request),
    }
}

#[cfg(feature = "redb")]
fn template_publish(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    mut request: Box<medulla_agent_types::agent_template::AgentTemplatePublishRequest>,
) -> Response {
    let context = match bind_agent_library_context(
        store,
        req_id,
        verified,
        request.context,
        "agent-template:publish",
        true,
    ) {
        Ok(context) => context,
        Err(error) => return Response::err(req_id, error),
    };
    // Definition provenance is bound to the AUTHENTICATED caller, not
    // to whatever the body claimed -- the same rule the three layers
    // beside it apply. The base's own tenant has to move with it or
    // `AgentTemplateDraft::validate` refuses the pair.
    request.template.tenant_id = context.tenant_id.clone();
    request.template.base.tenant_id = context.tenant_id.clone();
    request.template.actor_scope = context.actor_scope.clone();
    request.template.purpose_id = context.purpose_id.clone();
    request.template.policy_digest = context.policy_digest.clone();
    request.context = context;
    match store.publish_template(*request) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentTemplatePublish,
            >(&result.result),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn template_retire(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    mut request: medulla_agent_types::agent_template::AgentTemplateRetireRequest,
) -> Response {
    let context = match bind_agent_library_context(
        store,
        req_id,
        verified,
        request.context,
        "agent-template:retire",
        true,
    ) {
        Ok(context) => context,
        Err(error) => return Response::err(req_id, error),
    };
    request.context = context;
    match store.retire_template(request) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentTemplateRetire,
            >(&result.result),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn template_current(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    tenant_id: String,
    template_id: String,
) -> Response {
    match store.current_template(&tenant_id, &template_id) {
        Ok(entry) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentTemplateCurrent,
            >(&entry),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn template_history(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    tenant_id: String,
    template_id: String,
) -> Response {
    match store.template_revisions(&tenant_id, &template_id) {
        Ok(entries) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentTemplateHistory,
            >(&entries),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn template_status(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    mut request: medulla_agent_types::agent_template::AgentTemplateStatusRequest,
) -> Response {
    let purpose = match request.kind {
        medulla_agent_types::agent_template::AgentTemplateMutationKind::Publish => {
            "agent-template:publish"
        }
        medulla_agent_types::agent_template::AgentTemplateMutationKind::Retire => {
            "agent-template:retire"
        }
    };
    request.context = match bind_status_context(store, req_id, verified, request.context, purpose) {
        Ok(context) => context,
        Err(response) => return response,
    };
    match store.template_status(request) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentTemplateStatus,
            >(&result.map(|result| result.result)),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn template_list(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    request: medulla_agent_types::agent_template::AgentTemplateListRequest,
) -> Response {
    match store.list_templates(&request) {
        Ok(page) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentTemplateList,
            >(&page),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn template_instantiate(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    request: medulla_agent_types::agent_template::AgentTemplateInstantiateRequest,
) -> Response {
    match store.instantiate_template(&request) {
        Ok(draft) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentTemplateInstantiate,
            >(&draft),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// Publish, retire, or inspect one durable agent GRAPH (RF-ADR-008).
///
/// Routed to the SAME store as [`handle_agent_library`]: a graph is published
/// into the agent-library owner alongside the entries it composes, so there is
/// one `ensure_agent_library` and one physical authority (RF-RULING-004).
pub async fn handle_agent_graph(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    op: medulla_agent_types::agent_graph::AgentGraphOp,
) -> Response {
    use medulla_agent_types::agent_graph::AgentGraphOp;

    if let Err(error) = op.validate() {
        return Response::err(req_id, error);
    }
    let store = match ensure_agent_library(state, req_id).await {
        Ok(store) => store,
        Err(response) => return response,
    };
    match op {
        AgentGraphOp::Publish { request } => graph_publish(&store, req_id, verified, request),
        AgentGraphOp::Retire { request } => graph_retire(&store, req_id, verified, request),
        AgentGraphOp::Current {
            tenant_id,
            graph_id,
        } => graph_current(&store, req_id, verified, tenant_id, graph_id),
        AgentGraphOp::History {
            tenant_id,
            graph_id,
        } => graph_history(&store, req_id, verified, tenant_id, graph_id),
        AgentGraphOp::Status { request } => graph_status(&store, req_id, verified, request),
    }
}

#[cfg(feature = "redb")]
fn graph_publish(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    mut request: Box<medulla_agent_types::agent_graph::AgentGraphPublishRequest>,
) -> Response {
    let context = match bind_agent_library_context(
        store,
        req_id,
        verified,
        request.context,
        "agent-graph:publish",
        true,
    ) {
        Ok(context) => context,
        Err(error) => return Response::err(req_id, error),
    };
    // The verified caller owns the tenant/actor/purpose on the record,
    // not the request body: a caller must not be able to publish a
    // graph attributed to another tenant.
    request.graph.tenant_id = context.tenant_id.clone();
    request.graph.actor_scope = context.actor_scope.clone();
    request.graph.purpose_id = context.purpose_id.clone();
    request.graph.policy_digest = context.policy_digest.clone();
    request.context = context;
    if let Some(refusal) = medulla_server_ops::api::admin::topology_shape::refusal(&request.graph) {
        return Response::err(req_id, refusal);
    }
    match store.publish_graph(*request) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentGraphPublish,
            >(&result.result),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn graph_retire(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    mut request: medulla_agent_types::agent_graph::AgentGraphRetireRequest,
) -> Response {
    let context = match bind_agent_library_context(
        store,
        req_id,
        verified,
        request.context,
        "agent-graph:retire",
        true,
    ) {
        Ok(context) => context,
        Err(error) => return Response::err(req_id, error),
    };
    request.context = context;
    match store.retire_graph(request) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentGraphRetire,
            >(&result.result),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn graph_current(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    tenant_id: String,
    graph_id: String,
) -> Response {
    if tenant_id != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: agent graph tenant must match verified request tenant",
        );
    }
    match store.current_graph(&tenant_id, &graph_id) {
        Ok(entry) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentGraphCurrent,
            >(&entry),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn graph_history(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    tenant_id: String,
    graph_id: String,
) -> Response {
    if tenant_id != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: agent graph tenant must match verified request tenant",
        );
    }
    match store.graph_revisions(&tenant_id, &graph_id) {
        Ok(entries) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentGraphHistory,
            >(&entries),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn graph_status(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    mut request: medulla_agent_types::agent_graph::AgentGraphStatusRequest,
) -> Response {
    let purpose = match request.kind {
        medulla_agent_types::agent_graph::AgentGraphMutationKind::Publish => "agent-graph:publish",
        medulla_agent_types::agent_graph::AgentGraphMutationKind::Retire => "agent-graph:retire",
    };
    request.context = match bind_status_context(store, req_id, verified, request.context, purpose) {
        Ok(context) => context,
        Err(response) => return response,
    };
    match store.graph_status(request) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentGraphStatus,
            >(&result.map(|result| result.result)),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// The served response, and the entry a `Publish` committed: the router emits that entry's
/// catalog nodes through the one emitter (MEDULLA-TYPED-PACKS-R108.3).
pub struct AgentLibraryOutcome {
    pub response: Response,
    pub published: Option<medulla_agent_types::agent_library::AgentLibraryEntry>,
}

impl From<Response> for AgentLibraryOutcome {
    fn from(response: Response) -> Self {
        Self {
            response,
            published: None,
        }
    }
}

pub async fn handle_agent_library(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    op: medulla_agent_types::agent_library::AgentLibraryOp,
) -> AgentLibraryOutcome {
    let store = match ensure_agent_library(state, req_id).await {
        Ok(store) => store,
        Err(response) => return response.into(),
    };
    match op {
        medulla_agent_types::agent_library::AgentLibraryOp::Publish { request } => {
            library_publish(&store, req_id, verified, request)
        }
        medulla_agent_types::agent_library::AgentLibraryOp::Retire { request } => {
            library_retire(&store, req_id, verified, request).into()
        }
        medulla_agent_types::agent_library::AgentLibraryOp::Current {
            tenant_id,
            agent_id,
        } => library_current(&store, req_id, verified, tenant_id, agent_id).into(),
        medulla_agent_types::agent_library::AgentLibraryOp::History {
            tenant_id,
            agent_id,
        } => library_history(&store, req_id, verified, tenant_id, agent_id).into(),
        medulla_agent_types::agent_library::AgentLibraryOp::Status { request } => {
            library_status(&store, req_id, verified, request).into()
        }
        medulla_agent_types::agent_library::AgentLibraryOp::List {
            tenant_id,
            kind,
            lifecycle,
            limit,
            cursor,
        } => library_list(
            &store,
            req_id,
            verified,
            &tenant_id,
            kind.unwrap_or_default(),
            lifecycle,
            limit,
            cursor.as_deref(),
        )
        .into(),
        medulla_agent_types::agent_library::AgentLibraryOp::Get {
            tenant_id,
            kind,
            id,
        } => library_get(&store, req_id, verified, &tenant_id, kind, &id).into(),
    }
}

#[cfg(feature = "redb")]
fn library_publish(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    mut request: Box<medulla_agent_types::agent_library::AgentLibraryPublishRequest>,
) -> AgentLibraryOutcome {
    let context = match bind_agent_library_context(
        store,
        req_id,
        verified,
        request.context,
        "agent-library:publish",
        true,
    ) {
        Ok(context) => context,
        Err(error) => return Response::err(req_id, error).into(),
    };
    request.context = context.clone();
    request.entry = match bind_agent_library_draft(request.entry, verified, &context) {
        Ok(entry) => entry,
        Err(error) => return Response::err(req_id, error).into(),
    };
    match store.publish(*request) {
        Ok(result) => AgentLibraryOutcome {
            response: Response::ok(
                req_id,
                ResultPayload::of_ref::<
                    medulla_protocol_types::result_contract::storage::AgentLibraryPublish,
                >(&result),
            ),
            published: Some(result.entry.clone()),
        },
        Err(error) => Response::err(req_id, error).into(),
    }
}

#[cfg(feature = "redb")]
fn library_retire(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    mut request: medulla_agent_types::agent_library::AgentLibraryRetireRequest,
) -> Response {
    let context = match bind_agent_library_context(
        store,
        req_id,
        verified,
        request.context,
        "agent-library:retire",
        true,
    ) {
        Ok(context) => context,
        Err(error) => return Response::err(req_id, error),
    };
    request.context = context;
    match store.retire(request) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentLibraryRetire,
            >(&result),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn library_current(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    tenant_id: String,
    agent_id: String,
) -> Response {
    if tenant_id != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: Agent Library tenant must match verified request tenant",
        );
    }
    match store.current(&tenant_id, &agent_id) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentLibraryCurrent,
            >(&result),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn library_history(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    tenant_id: String,
    agent_id: String,
) -> Response {
    if tenant_id != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: Agent Library tenant must match verified request tenant",
        );
    }
    match store.revisions(&tenant_id, &agent_id) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentLibraryHistory,
            >(&result),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// `AgentLibrary` op `list` (MEDULLA-TYPED-PACKS-R011.1): one cursor-paged page of the
/// verified tenant's library heads. A tenant other than the verified one refuses exactly as the
/// other reads do.
#[cfg(feature = "redb")]
fn library_list(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    tenant_id: &str,
    kind: medulla_agent_types::agent_library::AgentLibraryKind,
    lifecycle: Option<medulla_agent_types::agent_library::AgentLibraryLifecycle>,
    limit: Option<u32>,
    cursor: Option<&str>,
) -> Response {
    use medulla_agent_types::agent_library::{
        unsupported_kind_refusal, AgentLibraryKind, AgentLibraryListEntry, AgentLibraryPage,
    };
    if tenant_id != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: Agent Library tenant must match verified request tenant",
        );
    }
    let page = match kind {
        AgentLibraryKind::Agent => store.list(tenant_id, lifecycle, limit, cursor),
        // The tool library is the component library's tool heads, paged by the one component
        // scan (MEDULLA-TYPED-PACKS-R011.2).
        AgentLibraryKind::Tool => store
            .search_components(&medulla_agent_types::agent_component::AgentComponentSearchRequest {
                tenant_id: tenant_id.to_string(),
                task: None,
                capabilities: Vec::new(),
                kinds: vec![medulla_agent_types::agent_component::AgentComponentKind::Tool],
                read_only: false,
                limit,
                cursor: cursor.map(str::to_string),
            })
            .map(|found| AgentLibraryPage {
                entries: found
                    .entries
                    .iter()
                    .filter(|entry| lifecycle.is_none_or(|wanted| entry.lifecycle == wanted))
                    .map(AgentLibraryListEntry::of_tool)
                    .collect(),
                next_cursor: found.next_cursor,
            }),
        AgentLibraryKind::Suggestion => Err(unsupported_kind_refusal(kind)),
    };
    match page {
        Ok(page) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentLibraryList,
            >(&page),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// `AgentLibrary` op `get` (MEDULLA-TYPED-PACKS-R011.2): the head entry of one id of one kind,
/// or null. A tenant other than the verified one refuses exactly as the other reads do.
#[cfg(feature = "redb")]
fn library_get(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    tenant_id: &str,
    kind: medulla_agent_types::agent_library::AgentLibraryKind,
    id: &str,
) -> Response {
    use medulla_agent_types::agent_library::{
        unsupported_kind_refusal, AgentLibraryKind, AgentLibraryListEntry,
    };
    if tenant_id != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: Agent Library tenant must match verified request tenant",
        );
    }
    let head: Result<Option<AgentLibraryListEntry>, String> = match kind {
        AgentLibraryKind::Agent => store
            .current(tenant_id, id)
            .map(|entry| entry.as_ref().map(AgentLibraryListEntry::from)),
        AgentLibraryKind::Tool => store.current_component(tenant_id, id).map(|entry| {
            entry
                .filter(|entry| {
                    entry.kind == medulla_agent_types::agent_component::AgentComponentKind::Tool
                })
                .as_ref()
                .map(AgentLibraryListEntry::of_tool)
        }),
        AgentLibraryKind::Suggestion => Err(unsupported_kind_refusal(kind)),
    };
    match head {
        Ok(head) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentLibraryGet,
            >(&head),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(feature = "redb")]
fn library_status(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    mut request: medulla_agent_types::agent_library::AgentLibraryStatusRequest,
) -> Response {
    let kind = request.kind;
    let purpose = match kind {
        medulla_agent_types::agent_library::AgentLibraryMutationKind::Publish => {
            "agent-library:publish"
        }
        medulla_agent_types::agent_library::AgentLibraryMutationKind::Retire => {
            "agent-library:retire"
        }
    };
    request.context = match bind_status_context(store, req_id, verified, request.context, purpose) {
        Ok(context) => context,
        Err(response) => return response,
    };
    match store.status(request) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentLibraryStatus,
            >(&result),
        ),
        Err(error) => Response::err(req_id, error),
    }
}
