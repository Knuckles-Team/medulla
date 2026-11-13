//! MEDULLA-DECISION-ENGINE-R819: a served `ServingTechniqueActivation` question
//! activates a technique only when every benchmark receipt names it and the
//! candidate metric stays inside the policy's `serving.max_regression` and
//! gains `serving.min_gain`. Missing, mismatched and regressed receipts are
//! coded refusals; a foreign tenant is refused before any read.

use super::*;
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_decision_types::decision::serving_technique::KNOBS;

fn text(name: &str, value: &str) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

fn receipts(rows: &[(&str, u64, u64)]) -> String {
    let rows: Vec<serde_json::Value> = rows
        .iter()
        .map(|(model, baseline, candidate)| {
            serde_json::json!({
                "technique": "spec-decode",
                "model": model,
                "config_digest": "cfg-1",
                "baseline_ppm": baseline,
                "candidate_ppm": candidate,
            })
        })
        .collect();
    serde_json::to_string(&rows).unwrap()
}

fn serving_request(
    schema: &ComponentDependency,
    policy: DecisionPolicyRef,
    receipts: &str,
) -> DecideRequest {
    declared_request(
        schema,
        TENANT,
        QuestionKind::ServingTechniqueActivation,
        "activation",
        policy,
        vec![
            text("technique", "spec-decode"),
            text("model", "m-1"),
            text("config_digest", "cfg-1"),
            text("receipts", receipts),
        ],
    )
}

fn answer(batch: &DecisionBatch) -> String {
    match &batch.records.as_slice()[0].outcome {
        StatisticalOutcome::Advisory { scores, .. } => scores.as_slice()[0].option_id.clone(),
        other => panic!("unexpected outcome {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R819
#[tokio::test]
async fn verdict_follows_receipts_and_knobs() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let (schema, _) = publish_route_schema(&h);
    let default = || DecisionPolicyRef::Default;

    let batch = decide(
        &h,
        serving_request(&schema, default(), &receipts(&[("m-1", 1_000, 1_100)])),
    )
    .await
    .unwrap();
    assert_eq!(answer(&batch), "activate");
    assert!(batch.records.as_slice()[0]
        .premises
        .iter()
        .any(|p| p.fact == "receipts:1"));

    // Equal metrics clear the zero gain floor; a slight gain below a raised
    // floor holds.
    let pinned = |name: &str, value: i64| {
        let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
        let spec = KNOBS.iter().find(|k| k.name == name).unwrap();
        policy
            .knobs
            .insert(spec.knob_name(), KnobValue::DecimalMicros(value));
        DecisionPolicyRef::Pinned {
            component: h.publish_policy(&format!("policy-{name}-{value}"), &policy),
        }
    };
    let gain = serving_request(
        &schema,
        pinned("serving.min_gain", 100_000),
        &receipts(&[("m-1", 1_000, 1_050)]),
    );
    assert_eq!(answer(&decide(&h, gain).await.unwrap()), "hold");
    let tolerant = serving_request(
        &schema,
        pinned("serving.max_regression", 50_000),
        &receipts(&[("m-1", 1_000, 960)]),
    );
    assert_eq!(answer(&decide(&h, tolerant).await.unwrap()), "hold");

    // Each refusal names its code.
    for (rows, code) in [
        (receipts(&[("m-1", 1_000, 999)]), "TECHNIQUE_REGRESSED"),
        (
            receipts(&[("other", 1_000, 1_100)]),
            "TECHNIQUE_EVIDENCE_MISMATCH",
        ),
        ("[]".to_string(), "TECHNIQUE_EVIDENCE_MISSING"),
    ] {
        let error = decide(&h, serving_request(&schema, default(), &rows))
            .await
            .unwrap_err();
        assert!(error.starts_with(code), "{code}: {error}");
    }

    // Malformed receipts are an invalid parameter.
    let error = decide(&h, serving_request(&schema, default(), "{"))
        .await
        .unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");

    // A foreign tenant is refused before any read.
    let mut foreign = serving_request(&schema, default(), &receipts(&[("m-1", 1_000, 1_100)]));
    foreign.tenant_id = "tenant-other".to_string();
    let error = decide(&h, foreign).await.unwrap_err();
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");
}
