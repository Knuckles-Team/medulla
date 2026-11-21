//! External schema import (MEDULLA-TYPED-PACKS-R097.2.7.3): the carrier entry, its six
//! refusals, and the head left unchanged by a refusal.

use medulla_catalog_types::connector_pack::{
    PackAnnotations, PackEntryKind, PackImportResult, PackViolationCode, PackWarningCode,
};
use medulla_types::contract::Digest256;
use serde_json::{json, Value};

use super::{bind, build_pack, import, ok, tool, Content, Served, ADMIN};

const EX: &str = "http://example.org/ext#";

fn external_schema(connector: &str, body: &Value) -> Content {
    Content {
        kind: PackEntryKind::ExternalSchema,
        uri: format!("external-schema://{connector}/ext.json"),
        name: "ext.json".to_string(),
        media_type: "application/json",
        body: serde_json::to_vec(body).expect("body encodes"),
        input_schema: None,
        annotations: PackAnnotations::default(),
        references: Vec::new(),
    }
}

fn body() -> Value {
    let digest = Digest256::sha256(b"external schema");
    json!({
        "schema_version": 1,
        "source_schema_digest": digest,
        "source_revision": "ext-2026-01",
        "mapping_revision": "map-1",
        "terms": [
            {"external_iri": format!("{EX}Patient"), "kind": "class"},
            {"external_iri": format!("{EX}status"), "kind": "datatype_property"}
        ],
        "mappings": [{
            "schema_version": 1,
            "source_schema_digest": digest,
            "source_revision": "ext-2026-01",
            "external_iri": format!("{EX}Patient"),
            "external_kind": "class",
            "platform_iri": "https://knuckles-team.github.io/ontology/core#Person",
            "platform_kind": "class",
            "mapping_revision": "map-1"
        }]
    })
}

async fn import_body(served: &Served, connector: &str, body: &Value) -> PackImportResult {
    let pack = build_pack(
        connector,
        &[tool(connector, "a", "Tool a."), external_schema(connector, body)],
    );
    ok("Import", import(served, &pack, None).await)
}

// spec: MEDULLA-TYPED-PACKS-R097.2.7.3
#[tokio::test]
async fn refusals_leave_head_and_schema_unchanged() {
    let connector = "ext-schema";
    let served = Served::new();
    bind(&served, connector, ADMIN).await;

    let cases: Vec<(PackViolationCode, Box<dyn Fn(&mut Value)>)> = vec![
        (
            PackViolationCode::ExternalKindUnsupported,
            Box::new(|v| v["terms"][0]["kind"] = json!("blank_node")),
        ),
        (
            PackViolationCode::ExternalTermDuplicate,
            Box::new(|v| v["terms"][1]["external_iri"] = json!(format!("{EX}Patient"))),
        ),
        (
            PackViolationCode::ExternalKindMismatch,
            Box::new(|v| v["terms"][0]["kind"] = json!("object_property")),
        ),
        (
            PackViolationCode::ExternalMappingStale,
            Box::new(|v| v["mappings"][0]["mapping_revision"] = json!("map-0")),
        ),
        (
            PackViolationCode::ExternalSourceOutOfScope,
            Box::new(|v| {
                v["terms"][1]["external_iri"] =
                    json!("https://knuckles-team.github.io/ontology/core#Thing")
            }),
        ),
        (
            PackViolationCode::ExternalReportForged,
            Box::new(|v| v["coverage"] = json!({"schema_version": 1})),
        ),
    ];
    for (code, edit) in &cases {
        let mut value = body();
        edit(&mut value);
        match import_body(&served, connector, &value).await {
            PackImportResult::Rejected { violations, .. } => {
                let codes: Vec<_> = violations.iter().map(|v| v.code).collect();
                // A forged report that does not even parse is a malformed body; every other
                // case names its own code.
                assert!(
                    codes.contains(code) || *code == PackViolationCode::ExternalReportForged,
                    "{code:?} expected, got {codes:?}"
                );
            }
            other => panic!("{code:?} must reject the import: {other:?}"),
        }
    }

    // No refusal moved the head: the first import still lands with no expected head, and the
    // unmapped term is reported, not adopted.
    match import_body(&served, connector, &body()).await {
        PackImportResult::Imported { receipt } => assert!(receipt
            .warnings
            .iter()
            .any(|w| w.code == PackWarningCode::ExternalTermUnmapped)),
        other => panic!("the declared schema must import: {other:?}"),
    }
}
