//! `AgentComponent.content`: serve one component revision's verified bytes.

use medulla_server_blob::blob::BlobStateExt;
use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

/// Read one component revision through its tenant-scoped Agent Library holder,
/// then verify the engine-owned CAS bytes before returning them.
pub async fn handle_component_content(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: medulla_agent_types::agent_component::AgentComponentContentRequest,
) -> Response {
    // A committed decision record's body is engine-owned too, but it lives in
    // the Agent Library transaction that committed it, not in the Blob CAS.
    if request
        .component_id
        .starts_with(medulla_decision_types::decision::DECISION_COMPONENT_ID_PREFIX)
    {
        return serve_decision_record(store, req_id, verified, &request);
    }
    serve_component_content(state, store, req_id, verified, request).await
}

fn serve_decision_record(
    store: &AgentLibraryStore,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: &medulla_agent_types::agent_component::AgentComponentContentRequest,
) -> Response {
    if request.tenant_id != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: component content tenant must match verified request tenant",
        );
    }
    match store.decision_record_content(&request.tenant_id, &request.component_id) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentContent,
            >(&result),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// One snapshot read of the revision's record and body pointer, then the
/// engine-owned bytes, digest-verified by `read_engine_body`. Shared by
/// `Content` and `ExportBlueprint`, so there is no second read route.
#[cfg(feature = "blob")]
pub async fn read_verified_body(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    tenant_id: &str,
    component_id: &str,
    entry_revision: Option<u64>,
) -> Result<
    (
        medulla_persistence_records::connector_pack::ConnectorPackContentPointer,
        Vec<u8>,
    ),
    String,
> {
    let pointer = store.connector_pack_content_pointer(tenant_id, component_id, entry_revision)?;
    let blob = match state.read().await.blob_cursors().cloned() {
        Some(blob) => blob,
        None => return Err("AgentComponent.content Blob substrate disabled".to_string()),
    };
    let tenant_id = tenant_id.to_string();
    let read = tokio::task::spawn_blocking(move || {
        medulla_server_blob::blob::engine_bodies::read_engine_body(
            blob.store.as_ref(),
            &tenant_id,
            &pointer.engine_manifest_digest,
            pointer.body_sha256,
            pointer.length,
        )
        .map(|body| (pointer, body))
    })
    .await;
    match read {
        Ok(result) => result,
        Err(error) => Err(format!("component body read task failed: {error}")),
    }
}

async fn serve_component_content(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: medulla_agent_types::agent_component::AgentComponentContentRequest,
) -> Response {
    if request.tenant_id != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: component content tenant must match verified request tenant",
        );
    }
    #[cfg(not(feature = "blob"))]
    {
        let _ = (state, store, request);
        Response::err(req_id, "AgentComponent.content requires the blob feature")
    }
    #[cfg(feature = "blob")]
    {
        let (pointer, body) = match read_verified_body(
            state,
            store,
            &request.tenant_id,
            &request.component_id,
            request.entry_revision,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return Response::err(req_id, error),
        };
        let result = medulla_agent_types::agent_component::AgentComponentContentResult {
            schema_version: medulla_agent_types::agent_component::COMPONENT_CONTENT_SCHEMA_VERSION,
            component_id: pointer.component_id,
            entry_revision: pointer.entry_revision,
            definition_digest: pointer.definition_digest,
            content_digest: format!("sha256:{}", pointer.body_sha256),
            media_type: pointer.media_type,
            body,
        };
        Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentContent,
            >(&result),
        )
    }
}

/// `AgentComponent.export_blueprint`: the complete admitted record at one pinned
/// revision plus its verified body bytes, never parsed
/// (MEDULLA-TYPED-PACKS-R094.2).
pub async fn serve_blueprint_export(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: medulla_agent_types::agent_component::AgentComponentBlueprintExportRequest,
) -> Response {
    if request.tenant_id != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: blueprint export tenant must match verified request tenant",
        );
    }
    if request.entry_revision == 0 {
        return Response::err(
            req_id,
            "INVALID_ARGUMENT: a blueprint export is pinned to a revision of at least 1",
        );
    }
    #[cfg(not(feature = "blob"))]
    {
        let _ = (state, store);
        Response::err(
            req_id,
            "AgentComponent.export_blueprint requires the blob feature",
        )
    }
    #[cfg(feature = "blob")]
    {
        let (pointer, body) = match read_verified_body(
            state,
            store,
            &request.tenant_id,
            &request.component_id,
            Some(request.entry_revision),
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return Response::err(req_id, error),
        };
        let result = medulla_agent_types::agent_component::BlueprintExport {
            schema_version: medulla_agent_types::agent_component::BLUEPRINT_EXPORT_SCHEMA_VERSION,
            content_digest: format!("sha256:{}", pointer.body_sha256),
            media_type: pointer.media_type,
            entry: pointer.entry,
            body,
        };
        Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentBlueprintExport,
            >(&result),
        )
    }
}

/// The pure envelope checks of `AgentComponent.import_blueprint` (MEDULLA-TYPED-PACKS-R094.3).
/// Each refusal is `BLUEPRINT_MALFORMED` and names what disagrees.
pub fn check_blueprint_envelope(
    request: &medulla_agent_types::agent_component::BlueprintImportRequest,
) -> Result<(), String> {
    use medulla_agent_types::agent_component::{
        AgentComponentFacts, BLUEPRINT_EXPORT_SCHEMA_VERSION, COMPONENT_MEDIA_TYPE_ATTRIBUTE,
        DEFAULT_COMPONENT_MEDIA_TYPE, MAX_COMPONENT_BODY_BYTES,
    };
    let malformed = |why: &str| Err(format!("BLUEPRINT_MALFORMED: {why}"));
    let envelope = &request.envelope;
    let entry = &envelope.entry;
    if envelope.schema_version != BLUEPRINT_EXPORT_SCHEMA_VERSION {
        return malformed("schema_version is not the supported blueprint export version");
    }
    if envelope.body.len() > MAX_COMPONENT_BODY_BYTES {
        return malformed("the body is over the component body limit");
    }
    if std::str::from_utf8(&envelope.body).is_err() {
        return malformed("the body is not UTF-8");
    }
    let body_hash = medulla_types::contract::Digest256::sha256(&envelope.body).to_hex();
    if envelope.content_digest != format!("sha256:{body_hash}") {
        return malformed("content_digest is not the SHA-256 of the body");
    }
    if entry.content_digest != envelope.content_digest {
        return malformed("the entry's content_digest differs from the envelope's");
    }
    if entry.tenant_id != request.context.tenant_id {
        return malformed("the entry's tenant differs from the request's");
    }
    if Some(entry.entry_revision) != request.context.expected_revision {
        return malformed("the entry's revision is not the base revision of the request");
    }
    let media_type = entry
        .attributes
        .get(COMPONENT_MEDIA_TYPE_ATTRIBUTE)
        .map_or(DEFAULT_COMPONENT_MEDIA_TYPE, String::as_str);
    if envelope.media_type != media_type {
        return malformed("media_type differs from the entry's content media type");
    }
    match &entry.facts {
        AgentComponentFacts::SystemPrompt {
            body_sha256: Some(pin),
            ..
        } if *pin == body_hash => Ok(()),
        _ => malformed("the entry's facts are not a system prompt pinned to the body"),
    }
}

/// An import changes the content of a revision, never who the component is: its id, kind,
/// provenance and tenant stay those of the base revision (MEDULLA-TYPED-PACKS-R094.3).
pub fn check_blueprint_identity(
    imported: &medulla_agent_types::agent_component::AgentComponentEntry,
    base: &medulla_agent_types::agent_component::AgentComponentEntry,
) -> Result<(), String> {
    if imported.component_id != base.component_id
        || imported.kind != base.kind
        || imported.provenance != base.provenance
        || imported.tenant_id != base.tenant_id
    {
        return Err(
            "BLUEPRINT_IDENTITY_CHANGED: an import keeps the component id, kind, provenance and \
             tenant of its base revision"
                .to_string(),
        );
    }
    Ok(())
}

/// The draft that republishes `entry`'s fields: the server-owned entry fields are dropped and
/// restamped by publish. The one conversion the import and the skill edit share.
pub fn draft_of(
    entry: medulla_agent_types::agent_component::AgentComponentEntry,
) -> medulla_agent_types::agent_component::AgentComponentDraft {
    medulla_agent_types::agent_component::AgentComponentDraft {
        component_id: entry.component_id,
        kind: entry.kind,
        version: entry.version,
        content_digest: entry.content_digest,
        content_ref: None,
        facts: entry.facts,
        provenance: entry.provenance,
        summary: entry.summary,
        classification: entry.classification,
        requires: entry.requires,
        declared_capabilities: entry.declared_capabilities,
        required_capabilities: entry.required_capabilities,
        declared_required_capabilities: entry.declared_required_capabilities,
        attributes: entry.attributes,
        tenant_id: entry.tenant_id,
        actor_scope: entry.actor_scope,
        purpose_id: entry.purpose_id,
        policy_digest: entry.policy_digest,
        source_revision: entry.source_revision,
        source_revision_digest: entry.source_revision_digest,
    }
}

/// `AgentComponent.import_blueprint`: the edited envelope becomes the next revision of a
/// native prompt through the one publish path of R002.7 (MEDULLA-TYPED-PACKS-R094.3). The
/// server-owned entry fields are ignored and restamped by publish.
pub async fn serve_blueprint_import(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: medulla_agent_types::agent_component::BlueprintImportRequest,
) -> Response {
    use medulla_agent_types::agent_component::AgentComponentPublishRequest;

    if request.context.tenant_id != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: blueprint import tenant must match verified request tenant",
        );
    }
    if let Err(error) = check_blueprint_envelope(&request) {
        return Response::err(req_id, error);
    }
    let base = request.context.expected_revision.unwrap_or(0);
    let pointer = match store.connector_pack_content_pointer(
        &request.context.tenant_id,
        &request.envelope.entry.component_id,
        Some(base),
    ) {
        Ok(pointer) => pointer,
        Err(error) => return Response::err(req_id, error),
    };
    if let Err(error) = check_blueprint_identity(&request.envelope.entry, &pointer.entry) {
        return Response::err(req_id, error);
    }
    let medulla_agent_types::agent_component::BlueprintImportRequest { context, envelope } =
        request;
    let entry = envelope.entry;
    let prompt_blueprint = match String::from_utf8(envelope.body) {
        Ok(text) => text,
        Err(_) => return Response::err(req_id, "BLUEPRINT_MALFORMED: the body is not UTF-8"),
    };
    let publish = AgentComponentPublishRequest {
        context,
        component: draft_of(entry),
        evaluation_receipt_digest: None,
        body: Some(
            medulla_agent_types::agent_component::ComponentBody::PromptBlueprint(prompt_blueprint),
        ),
    };
    super::agent::component_publish(state, store, req_id, verified, Box::new(publish)).await
}

#[cfg(feature = "blob")]
/// A JSON value parsed with duplicate object keys refused. Non-finite numbers
/// cannot be written in JSON, so the parser refuses them already.
struct StrictJson(serde_json::Value);

#[cfg(feature = "blob")]
impl<'de> serde::Deserialize<'de> for StrictJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = serde_json::Value;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON value without duplicate keys")
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(serde_json::Value::Null)
            }
            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E> {
                Ok(serde_json::Value::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
                Ok(v.into())
            }
            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
                Ok(v.into())
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| E::custom("a non-finite number"))
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
                Ok(serde_json::Value::String(v.to_string()))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(StrictJson(item)) = seq.next_element()? {
                    items.push(item);
                }
                Ok(serde_json::Value::Array(items))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut object = serde_json::Map::new();
                while let Some((key, StrictJson(value))) = map.next_entry::<String, StrictJson>()? {
                    if object.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("a duplicate object key"));
                    }
                }
                Ok(serde_json::Value::Object(object))
            }
        }
        deserializer.deserialize_any(Visitor).map(StrictJson)
    }
}

/// The blueprint of one pinned prompt revision, from its verified body bytes
/// (MEDULLA-TYPED-PACKS-R002.9). The pin `body_sha256` is the SHA-256 of the
/// raw stored bytes, so a different digest refuses `PROMPT_PIN_UNRESOLVED`; a
/// body that is not JSON, repeats an object key or holds a nonfinite number
/// refuses `PROMPT_SCHEMA_INVALID`.
#[cfg(feature = "blob")]
pub fn read_prompt_blueprint(
    body: &[u8],
    identity: &medulla_agent_types::agent_component::PromptBlueprintIdentity,
) -> Result<serde_json::Value, &'static str> {
    if medulla_types::contract::Digest256::sha256(body).to_hex() != identity.body_sha256 {
        return Err(
            medulla_compute_assemble::assemble::prompt::RefusalCode::PromptPinUnresolved.as_str(),
        );
    }
    serde_json::from_slice::<StrictJson>(body)
        .map(|StrictJson(value)| value)
        .map_err(|_| medulla_agent_types::agent_component::facts::PROMPT_SCHEMA_INVALID)
}

/// The one pinned-prompt reader: the revision row, its pin and its engine body,
/// read through the same verified route `Content` and `ExportBlueprint` use.
/// A schema revision other than the revision's own is `PROMPT_SCHEMA_INVALID`;
/// an unavailable body store is `PROMPT_PRODUCER_UNAVAILABLE`.
#[cfg(feature = "blob")]
pub async fn read_pinned_prompt(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    request: &medulla_agent_types::agent_component::PromptRenderRequest,
) -> Result<
    (
        medulla_agent_types::agent_component::PromptBlueprintIdentity,
        Vec<u8>,
    ),
    String,
> {
    use medulla_agent_types::agent_component::facts::{
        PROMPT_PRODUCER_UNAVAILABLE, PROMPT_SCHEMA_INVALID,
    };
    use medulla_agent_types::agent_component::PromptBlueprintIdentity;

    let (pointer, body) = read_verified_body(
        state,
        store,
        &request.tenant_id,
        &request.component_id,
        Some(request.revision),
    )
    .await
    .map_err(|error| {
        if error.contains("Blob substrate disabled") {
            format!("{PROMPT_PRODUCER_UNAVAILABLE}: {error}")
        } else {
            error
        }
    })?;
    let identity = PromptBlueprintIdentity::of_entry(&pointer.entry).ok_or_else(|| {
        format!(
            "{}: the revision is not a pinned blueprint prompt",
            medulla_compute_assemble::assemble::prompt::RefusalCode::PromptPinUnresolved.as_str()
        )
    })?;
    if request.schema_revision != identity.schema_revision {
        return Err(format!(
            "{PROMPT_SCHEMA_INVALID}: schema revision {} is not the revision's own {}",
            request.schema_revision, identity.schema_revision
        ));
    }
    Ok((identity, body))
}

/// `AgentComponent.render`: render one pinned prompt revision with its
/// variables and return the bytes with a receipt over the pins, the variables
/// digest and the verified tenant scope (MEDULLA-TYPED-PACKS-R002.9). Writes
/// nothing.
pub async fn serve_prompt_render(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: medulla_agent_types::agent_component::PromptRenderRequest,
) -> Response {
    if request.tenant_id != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: prompt render tenant must match verified request tenant",
        );
    }
    #[cfg(not(feature = "blob"))]
    {
        let _ = (state, store);
        Response::err(req_id, "AgentComponent.render requires the blob feature")
    }
    #[cfg(feature = "blob")]
    {
        use medulla_agent_types::agent_component::facts::{
            validate_prompt_blueprint, PromptLimits,
        };
        use medulla_agent_types::agent_component::{
            receipt_digest, PromptRenderReceipt, PromptRenderResult, PROMPT_RENDER_SCHEMA_VERSION,
        };
        use medulla_decision_types::decision::knob_families::prompt_render;
        use medulla_decision_types::decision::knobs::KnobValue;
        use medulla_types::contract::Digest256;
        use {
            medulla_compute_assemble::assemble::prompt::canonical_bytes,
            medulla_compute_assemble::assemble::prompt::render_prompt,
            medulla_compute_assemble::assemble::prompt::RenderPolicy,
            medulla_compute_assemble::assemble::prompt::RENDERING_VERSION,
        };

        let (identity, body) = match read_pinned_prompt(state, store, &request).await {
            Ok(value) => value,
            Err(error) => return Response::err(req_id, error),
        };
        let blueprint = match read_prompt_blueprint(&body, &identity) {
            Ok(value) => value,
            Err(code) => return Response::err(req_id, code),
        };
        let validated =
            match validate_prompt_blueprint(&blueprint, &identity, &PromptLimits::default()) {
                Ok(value) => value,
                Err(code) => {
                    return Response::err(
                        req_id,
                        format!("{code}: the stored prompt blueprint is not admitted"),
                    )
                }
            };
        let spec = &prompt_render::KNOBS[prompt_render::MAX_RENDER_BYTES];
        let max_render_bytes = match spec.default {
            KnobValue::Integer(bytes) => u32::try_from(bytes).unwrap_or(u32::MAX),
            _ => medulla_compute_assemble::assemble::prompt::DEFAULT_MAX_RENDER_BYTES,
        };
        let policy = RenderPolicy {
            max_render_bytes,
            rendering_version: RENDERING_VERSION,
        };
        let rendered = match render_prompt(&validated, &request.variables, &policy) {
            Ok(value) => value,
            Err(refusal) => return Response::err(req_id, refusal.as_str()),
        };
        let variables = serde_json::Value::Object(
            request
                .variables
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        );
        let variables_digest = Digest256::sha256(&canonical_bytes(&variables)).to_hex();
        let scope = verified.tenant().to_string();
        let receipt = PromptRenderReceipt {
            receipt_digest: receipt_digest(&identity, &variables_digest, &scope),
            identity,
            variables_digest,
            scope,
        };
        let result = PromptRenderResult {
            schema_version: PROMPT_RENDER_SCHEMA_VERSION,
            render_digest: rendered.render_digest.to_hex(),
            bytes: rendered.bytes,
            receipt,
        };
        Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentRender,
            >(&result),
        )
    }
}
