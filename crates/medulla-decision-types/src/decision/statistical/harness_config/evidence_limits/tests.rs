use super::*;
use crate::decision::knobs::KnobName;

fn limits(items: i64, bytes: i64, string_bytes: i64) -> HarnessEvidenceLimits {
    let mut policy = DecisionPolicy::engine_default();
    for (name, value) in [
        ("harness.evidence_max_items", items),
        ("harness.evidence_max_bytes", bytes),
        ("harness.evidence_max_string_bytes", string_bytes),
    ] {
        policy
            .knobs
            .insert(KnobName::new(name), KnobValue::Integer(value));
    }
    HarnessEvidenceLimits::from_policy(&policy)
}

fn evidence(profile: &str, tools: usize) -> HarnessConfigEvaluationEvidence {
    let decided =
        |value: serde_json::Value| serde_json::json!({"decision": "decided", "value": value});
    let q32 = |n: u64| serde_json::json!({"numerator": n, "denominator": 4294967296u64});
    let tool_ids: Vec<String> = (0..tools).map(|i| format!("tool-{i:04}")).collect();
    serde_json::from_value(serde_json::json!({
        "model_profile": {"component_id": profile, "kind": "model_profile",
            "definition_digest": "sha256:profile"},
        "request": {"model_profile_ref": profile, "task_class": "task",
            "usable_context_budget_tokens": 10, "priority": "interactive"},
        "answer": {
            "context_policy": decided(serde_json::json!("none")),
            "soft_threshold": decided(q32(1)),
            "hard_threshold": decided(q32(2)),
            "recent_window_fraction": decided(q32(1)),
            "plan_scaffold": decided(serde_json::json!("on")),
            "action_space": decided(serde_json::json!("tool_set")),
            "tool_subset": decided(serde_json::json!(tool_ids))
        }
    }))
    .unwrap()
}

// spec: MEDULLA-DECISION-ENGINE-R128.13.1.2
#[test]
fn defaults_come_from_the_knob_table() {
    let defaults = HarnessEvidenceLimits::from_policy(&DecisionPolicy::engine_default());
    assert_eq!(defaults.max_items, 4096);
    assert_eq!(defaults.max_bytes, 16 * 1024 * 1024);
    assert_eq!(defaults.max_string_bytes, 64 * 1024);
}

// spec: MEDULLA-DECISION-ENGINE-R128.13.1.2
#[test]
fn each_limit_refuses_with_the_typed_code() {
    let within = evidence("profile", 3);
    let frame = serde_json::to_vec(&within).unwrap();
    assert_eq!(limits(64, 1 << 24, 1024).decode(&frame).unwrap(), within);
    // Frame bytes: refused before the frame is decoded.
    let garbage = vec![b'{'; 70_000];
    let refused = limits(64, 65_536, 1024).decode(&garbage).unwrap_err();
    assert!(
        refused.starts_with(HARNESS_EVIDENCE_OVER_LIMIT),
        "{refused}"
    );
    // Items.
    let many = evidence("profile", 65);
    let refused = limits(64, 1 << 24, 1024).check_evidence(&many).unwrap_err();
    assert!(
        refused.starts_with(HARNESS_EVIDENCE_OVER_LIMIT),
        "{refused}"
    );
    // One string.
    let long = evidence(&"p".repeat(2048), 1);
    let refused = limits(64, 1 << 24, 1024).check_evidence(&long).unwrap_err();
    assert!(
        refused.starts_with(HARNESS_EVIDENCE_OVER_LIMIT),
        "{refused}"
    );
}
