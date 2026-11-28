//! MEDULLA-DECISION-ENGINE-R825: a served `ModelCapacity` question feeds the
//! caller's recorded observations to the admission controllers and answers the
//! limits they leave; it never exceeds the declared ceiling.

use super::*;
use medulla_decision_types::decision::record::AbstainReason;

fn observations(rows: &[(u64, u64, &str)]) -> String {
    let rows: Vec<serde_json::Value> = rows
        .iter()
        .map(|(at_ms, latency_ms, outcome)| {
            serde_json::json!({"at_ms": at_ms, "latency_ms": latency_ms, "outcome": outcome})
        })
        .collect();
    serde_json::to_string(&rows).unwrap()
}

fn bounds(initial: u32, ceiling: u32, cell_ceiling: u32, now_ms: u64) -> String {
    serde_json::json!({
        "group": "gpu-a", "initial": initial, "min": 1, "ceiling": ceiling,
        "cell_ceiling": cell_ceiling, "group_slots": 16, "now_ms": now_ms,
    })
    .to_string()
}

fn capacity_request(
    schema: &ComponentDependency,
    observations: &str,
    bounds: &str,
) -> DecideRequest {
    let text = |name: &str, value: &str| TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    };
    declared_request(
        schema,
        TENANT,
        QuestionKind::ModelCapacity,
        "capacity",
        DecisionPolicyRef::Default,
        vec![
            text("capacity_observations", observations),
            text("capacity_bounds", bounds),
        ],
    )
}

async fn answered(h: &Harness, request: DecideRequest) -> serde_json::Value {
    let batch = decide(h, request).await.unwrap();
    let StatisticalOutcome::Advisory { scores, .. } = &batch.records.as_slice()[0].outcome else {
        panic!("a capacity answer is advisory");
    };
    let option = &scores.as_slice()[0].option_id;
    serde_json::from_str(option.strip_prefix("capacity:").expect("capacity option")).unwrap()
}

// spec: MEDULLA-DECISION-ENGINE-R825
#[tokio::test]
async fn limit_narrows_on_latency_and_never_exceeds_ceiling() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let (schema, _) = publish_route_schema(&h);

    // Ten samples over the 2,000 ms target halve the limit each time down to the minimum.
    let slow: Vec<(u64, u64, &str)> = (1..=10).map(|at| (at, 5_000, "success")).collect();
    let narrowed = answered(
        &h,
        capacity_request(&schema, &observations(&slow), &bounds(1024, 2048, 2048, 10)),
    )
    .await;
    assert_eq!(narrowed["limit"], 1);

    // Three samples under it grow the limit by one each, and a ceiling holds it.
    let fast: Vec<(u64, u64, &str)> = (1..=3).map(|at| (at, 100, "success")).collect();
    let grown = answered(
        &h,
        capacity_request(&schema, &observations(&fast), &bounds(4, 16, 16, 3)),
    )
    .await;
    assert_eq!(grown["limit"], 7);
    let capped = answered(
        &h,
        capacity_request(&schema, &observations(&fast), &bounds(4, 5, 16, 3)),
    )
    .await;
    assert_eq!(capped["limit"], 5);

    // The default breaker opens after five consecutive failures.
    let failing: Vec<(u64, u64, &str)> = (1..=5).map(|at| (at, 100, "failure")).collect();
    let open = answered(
        &h,
        capacity_request(&schema, &observations(&failing), &bounds(4, 16, 16, 5)),
    )
    .await;
    assert_eq!(open["breaker"], "open");
    assert_eq!(open["effective_limit"], 0);
    assert_eq!(open["granted"], 0);

    // After the cooldown it half-opens and admits one.
    let half = answered(
        &h,
        capacity_request(
            &schema,
            &observations(&failing),
            &bounds(4, 16, 16, 5 + 30_000),
        ),
    )
    .await;
    assert_eq!(half["breaker"], "half_open");
    assert_eq!(half["effective_limit"], 1);

    // A ceiling above the cell ceiling abstains PolicyLoosening.
    let batch = decide(
        &h,
        capacity_request(&schema, &observations(&fast), &bounds(4, 32, 16, 3)),
    )
    .await
    .unwrap();
    let StatisticalOutcome::Abstained { reasons } = &batch.records.as_slice()[0].outcome else {
        panic!("a ceiling above the cell ceiling abstains");
    };
    assert!(matches!(
        &reasons.as_slice()[0],
        AbstainReason::PolicyLoosening { field } if field == "ceiling"
    ));

    // Time running backwards is an invalid parameter.
    let backwards = observations(&[(5, 100, "success"), (4, 100, "success")]);
    let error = decide(
        &h,
        capacity_request(&schema, &backwards, &bounds(4, 16, 16, 9)),
    )
    .await
    .unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
}
