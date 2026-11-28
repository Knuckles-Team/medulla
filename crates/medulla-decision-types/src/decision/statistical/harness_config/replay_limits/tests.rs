use super::*;
use medulla_decision_statistical_types::decision::knobs::KnobName;

fn limits(premises: i64, candidates: i64, bytes: i64) -> HarnessReplayLimits {
    let mut policy = DecisionPolicy::engine_default();
    for (name, value) in [
        ("harness.replay_max_premises", premises),
        ("harness.replay_max_candidates", candidates),
        ("harness.replay_max_bytes", bytes),
    ] {
        policy
            .knobs
            .insert(KnobName::new(name), KnobValue::Integer(value));
    }
    HarnessReplayLimits::from_policy(&policy)
}

fn evidence(options: usize) -> HarnessConfigEvaluationEvidence {
    let decided =
        |value: serde_json::Value| serde_json::json!({"decision": "decided", "value": value});
    let q32 = |n: u64| serde_json::json!({"numerator": n, "denominator": 4294967296u64});
    let declared: Vec<serde_json::Value> = (0..options)
        .map(|i| serde_json::json!({"option_id": format!("tool-{i:04}")}))
        .collect();
    serde_json::from_value(serde_json::json!({
        "model_profile": {"component_id": "profile", "kind": "model_profile",
            "definition_digest": "sha256:profile"},
        "request": {"model_profile_ref": "profile", "task_class": "task",
            "usable_context_budget_tokens": 10, "priority": "interactive"},
        "answer": {
            "context_policy": decided(serde_json::json!("none")),
            "soft_threshold": decided(q32(1)),
            "hard_threshold": decided(q32(2)),
            "recent_window_fraction": decided(q32(1)),
            "plan_scaffold": decided(serde_json::json!("on")),
            "action_space": decided(serde_json::json!("tool_set")),
            "tool_subset": decided(serde_json::json!([]))
        },
        "declared_options": declared
    }))
    .unwrap()
}

// spec: MEDULLA-DECISION-ENGINE-R128.13.2.2
#[test]
fn defaults_come_from_the_knob_table() {
    let defaults = HarnessReplayLimits::from_policy(&DecisionPolicy::engine_default());
    assert_eq!(defaults.max_premises, 1024);
    assert_eq!(defaults.max_candidates, 256);
    assert_eq!(defaults.max_bytes, 8 * 1024 * 1024);
}

// spec: MEDULLA-DECISION-ENGINE-R128.13.2.2
#[test]
fn each_limit_refuses_with_the_typed_code() {
    let within = evidence(8);
    assert!(limits(16, 8, 64 * 1024).check(16, &within, 1024).is_ok());
    for (premises, candidates, bytes) in [(17, 8, 1024), (16, 9, 1024), (16, 8, 64 * 1024 + 1)] {
        let evidence = evidence(candidates);
        let refused = limits(16, 8, 64 * 1024)
            .check(premises, &evidence, bytes)
            .unwrap_err();
        assert!(refused.starts_with(HARNESS_REPLAY_LIMIT), "{refused}");
    }
}
