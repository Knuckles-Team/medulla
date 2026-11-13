//! MEDULLA-DECISION-ENGINE-R826: a served `IngestShortfall` question funds the
//! goals that complete the most papers within the budget (ties by ascending goal
//! id) and names only the purge candidates that are terminal, unleased,
//! non-authoritative, receipted, not the last source copy and without active
//! work. A foreign tenant is refused.

use super::*;

fn json(name: &str, value: serde_json::Value) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

fn candidate(id: &str, leased: bool, last_copy: bool) -> serde_json::Value {
    serde_json::json!({
        "item_id": id, "terminal": true, "leased": leased, "authoritative": false,
        "source_receipt": true, "decision_receipt": true,
        "last_source_copy": last_copy, "active_work": false,
    })
}

fn shortfall_request(schema: &ComponentDependency, budget: i64) -> DecideRequest {
    declared_request(
        schema,
        TENANT,
        QuestionKind::IngestShortfall,
        "allocation",
        DecisionPolicyRef::Default,
        vec![
            json(
                "window",
                serde_json::json!({
                    "completed_distinct": 1500, "target": 2000, "shortfall": 500,
                    "bottleneck_stage": "embed", "state": "complete",
                }),
            ),
            json(
                "goals",
                serde_json::json!([
                    {"goal_id": "g-b", "cost": 10, "expected_papers": 100},
                    {"goal_id": "g-a", "cost": 10, "expected_papers": 100},
                    {"goal_id": "g-c", "cost": 40, "expected_papers": 100},
                ]),
            ),
            json(
                "purge_candidates",
                serde_json::json!([
                    candidate("p-2", false, false),
                    candidate("p-1", false, false),
                    candidate("p-leased", true, false),
                    candidate("p-last", false, true),
                ]),
            ),
            TypedParam {
                name: "budget".to_string(),
                value: TypedValue::Int(budget),
            },
        ],
    )
}

fn answer(batch: &DecisionBatch) -> Vec<String> {
    match &batch.records.as_slice()[0].outcome {
        StatisticalOutcome::Advisory { scores, .. } => scores
            .as_slice()
            .iter()
            .map(|s| s.option_id.clone())
            .collect(),
        other => panic!("unexpected outcome {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R826
#[tokio::test]
async fn allocation_ties_and_purge_ineligibility() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let (schema, _) = publish_route_schema(&h);

    // Equal cost per paper breaks by ascending goal id; the dearer goal does
    // not fit; only the two eligible purge candidates are named.
    let batch = decide(&h, shortfall_request(&schema, 25)).await.unwrap();
    assert_eq!(
        answer(&batch),
        ["goal:g-a", "goal:g-b", "purge:p-1", "purge:p-2"]
    );
    assert!(batch.records.as_slice()[0]
        .premises
        .iter()
        .any(|p| p.fact.starts_with("shortfall:500")));

    // A wider budget funds the third goal, after the cheaper two.
    let batch = decide(&h, shortfall_request(&schema, 60)).await.unwrap();
    assert_eq!(answer(&batch)[..3], ["goal:g-a", "goal:g-b", "goal:g-c"]);

    // A budget below every goal funds none but still names the purges.
    let batch = decide(&h, shortfall_request(&schema, 5)).await.unwrap();
    assert_eq!(answer(&batch), ["purge:p-1", "purge:p-2"]);

    // A negative budget is an invalid parameter.
    let error = decide(&h, shortfall_request(&schema, -1))
        .await
        .unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");

    // A foreign tenant is refused before any read.
    let mut foreign = shortfall_request(&schema, 25);
    foreign.tenant_id = "tenant-other".to_string();
    let error = decide(&h, foreign).await.unwrap_err();
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");
}
