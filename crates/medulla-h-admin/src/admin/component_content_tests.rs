//! Served tests for the agent component blueprint export and prompt render.

use medulla_agent_types::agent_component::{
    AgentComponentBlueprintExportRequest, AgentComponentOp, BlueprintExport,
    BLUEPRINT_EXPORT_SCHEMA_VERSION,
};
use medulla_protocol_types::protocol::Method;

use super::connector_pack::tests::{
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
    assert!(
        unknown.starts_with("COMPONENT_REVISION_UNKNOWN"),
        "{unknown}"
    );

    // An unpinned (zero) revision is an invalid argument.
    let zero = refused(
        "zero revision",
        served.call("export:zero", export(TENANT, 0)).await,
    );
    assert!(zero.starts_with("INVALID_ARGUMENT"), "{zero}");

    // Another tenant is refused before any read.
    let denied = refused(
        "other tenant",
        served
            .call("export:denied", export("tenant-other", 1))
            .await,
    );
    assert!(denied.starts_with("ACCESS_DENIED"), "{denied}");
}

// spec: MEDULLA-TYPED-PACKS-R094.3
#[tokio::test]
async fn import_envelope_and_identity_checks_name_their_codes() {
    use medulla_agent_types::agent_component::{AgentComponentFacts, BlueprintImportRequest};
    use medulla_types::contract::Nonce;
    use {
        medulla_h_admin_schema::admin::component_content::check_blueprint_envelope,
        medulla_h_admin_schema::admin::component_content::check_blueprint_identity,
    };

    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    let pinned = tool(CONNECTOR, "pinned", "A tool pinned for import.");
    imported(
        &served,
        &build_pack(CONNECTOR, std::slice::from_ref(&pinned)),
        None,
    )
    .await;
    let exported: BlueprintExport = ok(
        "ExportBlueprint",
        served.call("import:export", export(TENANT, 1)).await,
    );
    let request = |envelope: BlueprintExport| BlueprintImportRequest {
        context: medulla_agent_types::agent_library::AgentLibraryMutationContext {
            request_id: 1,
            principal: "bound-by-engine".to_string(),
            caller_principal: "bound-by-engine".to_string(),
            attempt_nonce: Nonce::from_bytes([0; 32]),
            tenant_id: TENANT.to_string(),
            actor_scope: "bound-by-engine".to_string(),
            purpose_id: "bound-by-engine".to_string(),
            policy_revision: "bound-by-engine".to_string(),
            policy_digest: format!("sha256:{}", "0".repeat(64)),
            policy_decision_id: "bound-by-engine".to_string(),
            idempotency_key: "import-key".to_string(),
            expected_revision: Some(1),
            trace_id: None,
            created_at_ms: 1,
        },
        envelope,
    };
    let code = |result: Result<(), String>| result.expect_err("refused");

    // The exported tool is not a system prompt, so the facts clause names it.
    assert!(code(check_blueprint_envelope(&request(exported.clone())))
        .starts_with("BLUEPRINT_MALFORMED"));
    // Each other clause names BLUEPRINT_MALFORMED too.
    let mut bad_version = exported.clone();
    bad_version.schema_version += 1;
    let mut bad_digest = exported.clone();
    bad_digest.content_digest = format!("sha256:{}", "0".repeat(64));
    let mut bad_media = exported.clone();
    bad_media.media_type = "text/plain".to_string();
    let mut bad_tenant = exported.clone();
    bad_tenant.entry.tenant_id = "tenant-other".to_string();
    let mut bad_revision = exported.clone();
    bad_revision.entry.entry_revision = 2;
    let mut not_utf8 = exported.clone();
    not_utf8.body = vec![0xff, 0xfe];
    for envelope in [
        bad_version,
        bad_digest,
        bad_media,
        bad_tenant,
        bad_revision,
        not_utf8,
    ] {
        assert!(
            code(check_blueprint_envelope(&request(envelope))).starts_with("BLUEPRINT_MALFORMED")
        );
    }
    let mut opaque = exported.clone();
    opaque.entry.facts = AgentComponentFacts::Opaque;
    assert!(code(check_blueprint_envelope(&request(opaque))).starts_with("BLUEPRINT_MALFORMED"));

    // The identity of the base revision cannot change.
    assert_eq!(
        check_blueprint_identity(&exported.entry, &exported.entry),
        Ok(())
    );
    let mut other_id = exported.entry.clone();
    other_id.component_id = "prompt:other".to_string();
    let mut other_kind = exported.entry.clone();
    other_kind.kind = medulla_agent_types::agent_component::AgentComponentKind::SystemPrompt;
    let mut other_tenant = exported.entry.clone();
    other_tenant.tenant_id = "tenant-other".to_string();
    let mut other_provenance = exported.entry.clone();
    other_provenance.provenance = medulla_agent_types::agent_component::ComponentProvenance::Native;
    for changed in [other_id, other_kind, other_tenant, other_provenance] {
        assert!(code(check_blueprint_identity(&changed, &exported.entry))
            .starts_with("BLUEPRINT_IDENTITY_CHANGED"));
    }
}

// spec: MEDULLA-TYPED-PACKS-R002.9
#[test]
fn prompt_blueprint_reader_checks_pin_and_strict_json() {
    use medulla_agent_types::agent_component::{ComponentProvenance, PromptBlueprintIdentity};
    use medulla_h_admin_schema::admin::component_content::read_prompt_blueprint;

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
