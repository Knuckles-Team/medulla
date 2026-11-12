//! Model-level proof for the rendered skill package contract.

use medulla_catalog_types::fleet_catalog::{
    RenderedSkillPackage, RENDERED_SKILL_PACKAGE_SCHEMA_VERSION,
};

// spec: MEDULLA-TYPED-PACKS-R100.1
#[test]
fn rendered_package_round_trip() {
    let expected = RenderedSkillPackage {
        schema_version: RENDERED_SKILL_PACKAGE_SCHEMA_VERSION,
        uri: "skill://triage".into(),
        digest: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
        ontology_refs: vec!["eg:ontology/task".into(), "eg:ontology/tool".into()],
        graph_revision: 42,
    };
    assert_eq!(expected.schema_version, 1);
    assert_eq!(expected.digest.len(), 64);
    assert!(expected
        .digest
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    let encoded = serde_json::to_vec(&expected).expect("encode");
    let decoded: RenderedSkillPackage = serde_json::from_slice(&encoded).expect("decode");
    assert_eq!(decoded, expected);

    // Unknown fields are refused and the version is required.
    let mut wire: serde_json::Value = serde_json::from_slice(&encoded).expect("json");
    wire["extra"] = serde_json::json!(1);
    assert!(serde_json::from_value::<RenderedSkillPackage>(wire).is_err());
    let mut wire: serde_json::Value = serde_json::from_slice(&encoded).expect("json");
    wire.as_object_mut().unwrap().remove("schema_version");
    assert!(serde_json::from_value::<RenderedSkillPackage>(wire).is_err());
}
