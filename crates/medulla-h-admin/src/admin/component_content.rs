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
pub(crate) async fn handle_component_content(
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
async fn read_verified_body(
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
pub(crate) async fn serve_blueprint_export(
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
        Response::err(req_id, "AgentComponent.export_blueprint requires the blob feature")
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
            schema_version:
                medulla_agent_types::agent_component::BLUEPRINT_EXPORT_SCHEMA_VERSION,
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
pub(crate) fn read_prompt_blueprint(
    body: &[u8],
    identity: &medulla_agent_types::agent_component::PromptBlueprintIdentity,
) -> Result<serde_json::Value, &'static str> {
    if medulla_types::contract::Digest256::sha256(body).to_hex() != identity.body_sha256 {
        return Err(medulla_compute::assemble::prompt::RefusalCode::PromptPinUnresolved.as_str());
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
pub(crate) async fn read_pinned_prompt(
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
            medulla_compute::assemble::prompt::RefusalCode::PromptPinUnresolved.as_str()
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
pub(crate) async fn serve_prompt_render(
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
        use medulla_compute::assemble::prompt::{
            canonical_bytes, render_prompt, RenderPolicy, RENDERING_VERSION,
        };
        use medulla_decision_types::decision::knob_families::prompt_render;
        use medulla_decision_types::decision::knobs::KnobValue;
        use medulla_types::contract::Digest256;

        let (identity, body) = match read_pinned_prompt(state, store, &request).await {
            Ok(value) => value,
            Err(error) => return Response::err(req_id, error),
        };
        let blueprint = match read_prompt_blueprint(&body, &identity) {
            Ok(value) => value,
            Err(code) => return Response::err(req_id, code),
        };
        let validated = match validate_prompt_blueprint(&blueprint, &identity, &PromptLimits::default())
        {
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
            _ => medulla_compute::assemble::prompt::DEFAULT_MAX_RENDER_BYTES,
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

#[cfg(all(test, feature = "redb", feature = "blob"))]
mod tests {
    use medulla_agent_types::agent_component::{
        AgentComponentBlueprintExportRequest, AgentComponentOp, BlueprintExport,
        BLUEPRINT_EXPORT_SCHEMA_VERSION,
    };
    use medulla_protocol_types::protocol::Method;

    use crate::admin::connector_pack::tests::{
        bind, build_pack, imported, ok, refused, tool, Served, ADMIN, TENANT,
    };

    const CONNECTOR: &str = "blueprint-export";

    fn export(tenant: &str, revision: u64) -> Method {
        Method::AgentComponent {
            op: AgentComponentOp::ExportBlueprint {
                request: AgentComponentBlueprintExportRequest {
                    tenant_id: tenant.to_string(),
                    component_id: format!("mcp:{CONNECTOR}/tool/pinned"),
                    entry_revision: revision,
                },
            },
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R094.2
    #[tokio::test]
    async fn export_pins_revision_and_refuses_unknown() {
        let served = Served::new();
        bind(&served, CONNECTOR, ADMIN).await;
        let pinned = tool(CONNECTOR, "pinned", "A tool pinned for export.");
        imported(
            &served,
            &build_pack(CONNECTOR, std::slice::from_ref(&pinned)),
            None,
        )
        .await;

        // The pinned revision exports its complete record and its exact bytes.
        let exported: BlueprintExport = ok(
            "ExportBlueprint",
            served.call("export:pinned", export(TENANT, 1)).await,
        );
        assert_eq!(exported.schema_version, BLUEPRINT_EXPORT_SCHEMA_VERSION);
        assert_eq!(exported.body, pinned.body);
        assert_eq!(exported.entry.entry_revision, 1);
        assert_eq!(exported.entry.tenant_id, TENANT);
        assert_eq!(
            exported.entry.component_id,
            format!("mcp:{CONNECTOR}/tool/pinned")
        );
        assert_eq!(exported.content_digest, exported.entry.content_digest);
        assert_eq!(exported.media_type, "application/json");

        // A revision the tenant never published is refused by name.
        let unknown = refused(
            "unknown revision",
            served.call("export:unknown", export(TENANT, 99)).await,
        );
        assert!(unknown.starts_with("COMPONENT_REVISION_UNKNOWN"), "{unknown}");

        // An unpinned (zero) revision is an invalid argument.
        let zero = refused(
            "zero revision",
            served.call("export:zero", export(TENANT, 0)).await,
        );
        assert!(zero.starts_with("INVALID_ARGUMENT"), "{zero}");

        // Another tenant is refused before any read.
        let denied = refused(
            "other tenant",
            served.call("export:denied", export("tenant-other", 1)).await,
        );
        assert!(denied.starts_with("ACCESS_DENIED"), "{denied}");
    }

    // spec: MEDULLA-TYPED-PACKS-R002.9
    #[test]
    fn prompt_blueprint_reader_checks_pin_and_strict_json() {
        use super::read_prompt_blueprint;
        use medulla_agent_types::agent_component::{ComponentProvenance, PromptBlueprintIdentity};

        let identity_for = |body: &[u8]| PromptBlueprintIdentity {
            tenant_id: TENANT.to_string(),
            component_id: "prompt:a".to_string(),
            entry_revision: 1,
            schema_revision: 1,
            body_sha256: medulla_types::contract::Digest256::sha256(body).to_hex(),
            provenance: ComponentProvenance::Native,
        };
        let body = br#"{"task":"t","schema_revision":1}"#;
        let value = read_prompt_blueprint(body, &identity_for(body)).expect("reads");
        assert_eq!(value["task"], "t");
        assert_eq!(
            read_prompt_blueprint(body, &identity_for(b"other")),
            Err("PROMPT_PIN_UNRESOLVED")
        );
        let duplicate = br#"{"task":"a","task":"b"}"#;
        assert_eq!(
            read_prompt_blueprint(duplicate, &identity_for(duplicate)),
            Err("PROMPT_SCHEMA_INVALID")
        );
        let not_json = b"not json";
        assert_eq!(
            read_prompt_blueprint(not_json, &identity_for(not_json)),
            Err("PROMPT_SCHEMA_INVALID")
        );
    }
}
