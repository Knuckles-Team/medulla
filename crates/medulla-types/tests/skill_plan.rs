//! Skill compilation record round trip (MEDULLA-REPO-INGEST-R002.1.1).

use medulla_types::contract::Digest256;
use medulla_types::skill_plan::{SkillCompilation, SkillStepBinding, SKILL_GRAMMAR_REVISION};

fn compilation() -> SkillCompilation {
    SkillCompilation {
        directory_ref: "skills/deploy".to_string(),
        document_digest: Digest256::sha256(b"document"),
        grammar_revision: SKILL_GRAMMAR_REVISION.to_string(),
        plan_digest: Digest256::sha256(b"plan"),
        capability_digest: Digest256::sha256(b"capabilities"),
        type_digest: Digest256::sha256(b"types"),
        steps: vec![SkillStepBinding {
            step_id: "build".to_string(),
            span_start: 10,
            span_end: 42,
            instruction_digest: Digest256::sha256(b"build it"),
            capability_id: Some("cargo.build".to_string()),
        }],
    }
}

// spec: MEDULLA-REPO-INGEST-R002.1.1
#[test]
fn compilation_round_trips() {
    let value = compilation();
    assert_eq!(value.grammar_revision, "skill-steps-1");
    let json = serde_json::to_value(&value).expect("encode");
    let decoded: SkillCompilation = serde_json::from_value(json.clone()).expect("decode");
    assert_eq!(decoded, value);

    let mut unknown = json;
    unknown["document_text"] = serde_json::json!("copied prose");
    assert!(serde_json::from_value::<SkillCompilation>(unknown).is_err());

    let (doc, cap, ty) = (
        value.document_digest,
        value.capability_digest,
        value.type_digest,
    );
    assert!(!value.is_stale_against(&doc, &cap, &ty));
    assert!(value.is_stale_against(&Digest256::sha256(b"edited"), &cap, &ty));
}
