//! Served scale intents (MEDULLA-DECISION-ENGINE-R833): a replica-count intent is
//! judged against the `Scale*` authority nodes stored in the graph the question
//! names, and answers `allow` or `refuse:<CODE>`.

use medulla_decision_types::decision::statistical::{TypedParam, TypedValue};
use medulla_h_decide_stat_tests::decide::stat_tests::kernel_family_support::kernel_request;
use medulla_h_decide_stat_tests::decide::stat_tests::path_preference_tests::pins;
use medulla_h_decide_stat_tests::decide::stat_tests::*;

const GRAPH: &str = "scale-graph";

fn node(label: &str, fields: serde_json::Value) -> Vec<u8> {
    let mut object = fields.as_object().cloned().unwrap_or_default();
    object.insert("type".into(), label.into());
    rmp_serde::to_vec_named(&serde_json::Value::Object(object)).unwrap()
}

async fn seed_authority(h: &Harness, domain_status: &str, revisions: usize) {
    let mut guard = h.state.write().await;
    guard
        .registry
        .create_graph(
            GRAPH,
            medulla_protocol_types::protocol::GraphType::Team,
            None,
        )
        .unwrap();
    let core = guard.registry.get(GRAPH).unwrap().core.clone();
    let fence = serde_json::json!({"lease_id": "l1", "lease_epoch": 2, "fence_token": 9});
    let rows = vec![
        (
            "u1".to_string(),
            node(
                "ScaleUnit",
                serde_json::json!({
                    "unit_id": "u1", "revision": 3, "resource_pool_id": "p1",
                    "min_replicas": 0, "max_replicas": 10, "controller_mode": "native",
                    "replica_writer_id": "w1", "failure_domain_id": "d1",
                }),
            ),
        ),
        (
            "p1".to_string(),
            node(
                "ScaleResourcePool",
                serde_json::json!({
                    "pool_id": "p1", "capacity_units": 10, "reserved_headroom_units": 2,
                }),
            ),
        ),
        (
            "d1".to_string(),
            node(
                "ScaleFailureDomain",
                serde_json::json!({"domain_id": "d1", "status": domain_status}),
            ),
        ),
        (
            "reg1".to_string(),
            node(
                "ScaleControllerRegistration",
                serde_json::json!({"unit_id": "u1", "lease_fence": fence}),
            ),
        ),
    ];
    for (id, blob) in rows {
        core.add_node(id, blob);
    }
    for n in 0..revisions {
        core.add_node(
            format!("rev{n}"),
            node(
                "ScaleAuthorityRevision",
                serde_json::json!({"authority_revision": 5}),
            ),
        );
    }
}

fn intent(desired: u64) -> serde_json::Value {
    serde_json::json!({
        "intent_id": "i1", "intent_revision": 1, "expected_unit_revision": 3,
        "desired_replicas": desired, "min_replicas": 1, "max_replicas": 10,
        "controller_mode": "native", "replica_writer_id": "w1", "execution_key": "x1",
        "fence": {"lease_id": "l1", "lease_epoch": 2, "fence_token": 9},
        "target": {
            "cluster_id": "c", "runtime_id": "r", "workload_id": "u1",
            "target_uid": "uid", "target_resource_version": "7",
        },
    })
}

fn request(schema: &ComponentDependency, body: serde_json::Value) -> DecideRequest {
    let mut request = kernel_request(
        schema,
        QuestionKind::ScaleIntent,
        "intent",
        TypedValue::Text(body.to_string()),
        &["verdict"],
    );
    request.params = BoundedVec::new(vec![
        TypedParam {
            name: "graph".into(),
            value: TypedValue::Text(GRAPH.into()),
        },
        TypedParam {
            name: "intent".into(),
            value: TypedValue::Text(body.to_string()),
        },
    ])
    .unwrap();
    request
}

fn verdict(record: &StatisticalDecisionRecord) -> String {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => scores.as_slice()[0].option_id.clone(),
        other => panic!("an advisory answer, got {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R833
#[tokio::test]
async fn a_scale_intent_is_judged_against_the_stored_authority() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let pins = pins(&h, TENANT);
    seed_authority(&h, "healthy", 1).await;

    let allowed = decide(&h, request(&pins.0, intent(4))).await.unwrap();
    assert_eq!(verdict(&allowed.records.as_slice()[0]), "allow");

    let over_pool = decide(&h, request(&pins.0, intent(9))).await.unwrap();
    assert_eq!(
        verdict(&over_pool.records.as_slice()[0]),
        "refuse:SCALE_POOL_HEADROOM_EXCEEDED"
    );

    let mut stale = intent(4);
    stale["expected_unit_revision"] = 2.into();
    let refused = decide(&h, request(&pins.0, stale)).await.unwrap();
    assert_eq!(
        verdict(&refused.records.as_slice()[0]),
        "refuse:SCALE_UNIT_REVISION_STALE"
    );

    let mut unknown = intent(4);
    unknown["target"]["workload_id"] = "nope".into();
    let refused = decide(&h, request(&pins.0, unknown)).await.unwrap();
    assert_eq!(
        verdict(&refused.records.as_slice()[0]),
        "refuse:SCALE_UNIT_UNKNOWN"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R833
#[tokio::test]
async fn an_unhealthy_domain_and_a_missing_authority_refuse() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let pins = pins(&h, TENANT);
    seed_authority(&h, "offline", 0).await;
    let none = decide(&h, request(&pins.0, intent(4))).await.unwrap();
    assert_eq!(
        verdict(&none.records.as_slice()[0]),
        "refuse:SCALE_AUTHORITY_MISSING"
    );
    h.state
        .write()
        .await
        .registry
        .get(GRAPH)
        .unwrap()
        .core
        .add_node(
            "rev0".to_string(),
            node(
                "ScaleAuthorityRevision",
                serde_json::json!({"authority_revision": 5}),
            ),
        );
    let offline = decide(&h, request(&pins.0, intent(4))).await.unwrap();
    assert_eq!(
        verdict(&offline.records.as_slice()[0]),
        "refuse:SCALE_FAILURE_DOMAIN_UNHEALTHY"
    );
}
