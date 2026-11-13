//! MEDULLA-DECISION-ENGINE-R820: a served `ProcessRemediation` question judges
//! run-bound liveness evidence at the engine's clock: `zombie` after
//! `process.zombie_after` without progress or child output, `hung` after
//! `process.hung_after`, otherwise `progressing`. Stale evidence and evidence
//! for another run are coded refusals; a foreign tenant is refused.

use super::*;
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_decision_types::decision::process_remediation::KNOBS;

fn text(name: &str, value: &str) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

fn int(name: &str, value: u64) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Int(i64::try_from(value).unwrap()),
    }
}

fn liveness_request(
    schema: &ComponentDependency,
    policy: DecisionPolicyRef,
    idle_s: u64,
    child_output: bool,
    observed_age_ms: u64,
    evidence_run: &str,
) -> DecideRequest {
    let now = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    declared_request(
        schema,
        TENANT,
        QuestionKind::ProcessRemediation,
        "remediation",
        policy,
        vec![
            text("run_id", "run-1"),
            text("evidence_run_id", evidence_run),
            int("observed_at_ms", now - observed_age_ms),
            int("last_progress_ms", now - observed_age_ms - idle_s * 1_000),
            int("cpu_time_ms", 1_200),
            int("output_bytes", 4_096),
            TypedParam {
                name: "holds_child_output".to_string(),
                value: TypedValue::Bool(child_output),
            },
        ],
    )
}

fn answer(batch: &DecisionBatch) -> String {
    match &batch.records.as_slice()[0].outcome {
        StatisticalOutcome::Advisory { scores, .. } => scores.as_slice()[0].option_id.clone(),
        other => panic!("unexpected outcome {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R820
#[tokio::test]
async fn verdicts_follow_thresholds() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let (schema, _) = publish_route_schema(&h);
    let default = || DecisionPolicyRef::Default;

    for (idle_s, child, expected) in [
        (30, false, "progressing"),
        (900, false, "zombie"),
        (900, true, "progressing"),
        (2_400, true, "hung"),
        (2_400, false, "zombie"),
    ] {
        let request = liveness_request(&schema, default(), idle_s, child, 1_000, "run-1");
        assert_eq!(
            answer(&decide(&h, request).await.unwrap()),
            expected,
            "{idle_s} {child}"
        );
    }

    // The zombie threshold is the policy's knob.
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy
        .knobs
        .insert(KNOBS[0].knob_name(), KnobValue::Integer(1_200));
    let relaxed = DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-zombie-1200", &policy),
    };
    let request = liveness_request(&schema, relaxed, 900, false, 1_000, "run-1");
    assert_eq!(answer(&decide(&h, request).await.unwrap()), "progressing");

    // Stale evidence and evidence for another run refuse with their codes.
    let stale = liveness_request(&schema, default(), 30, false, 400_000, "run-1");
    let error = decide(&h, stale).await.unwrap_err();
    assert!(error.starts_with("LIVENESS_EVIDENCE_STALE"), "{error}");
    let other = liveness_request(&schema, default(), 30, false, 1_000, "run-2");
    let error = decide(&h, other).await.unwrap_err();
    assert!(error.starts_with("LIVENESS_EVIDENCE_MISMATCH"), "{error}");

    // A foreign tenant is refused before any read.
    let mut foreign = liveness_request(&schema, default(), 30, false, 1_000, "run-1");
    foreign.tenant_id = "tenant-other".to_string();
    let error = decide(&h, foreign).await.unwrap_err();
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");
}
