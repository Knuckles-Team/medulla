//! MEDULLA-DECISION-ENGINE-R824: a served `CapacityProposal` question reads the
//! cell's headroom and epoch from the capacity ledger and answers `allow` or
//! `refuse:<CODE>`: one case per refusal code. The observation is the engine's
//! own, so a caller that supplies it is refused, and a foreign tenant is too.

use std::sync::Arc;

use medulla_decision_types::decision::statistical::capacity::CapacityRefusalCode;
use medulla_mutation_types::capacity_lease::{CapacityCell, CapacityResourceClass};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::protocol::Method;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_work_types::native_control::{CapacityCellUpdateRequest, NativeControlSchemaVersion};

use super::*;

const GRAPH: &str = "capacity";
const CELL: &str = "cell-llm";

async fn capacity_harness() -> Harness {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let backend = Arc::new(
        RedbBackend::open_with_shards(h._dir.path().to_string_lossy().into_owned(), 16, 1).unwrap(),
    );
    backend
        .commit_capacity_lease(
            GRAPH,
            Method::UpdateCapacityCell {
                request: CapacityCellUpdateRequest {
                    schema_version: NativeControlSchemaVersion::V1,
                    cell: CapacityCell {
                        cell_id: CELL.to_string(),
                        parent_id: None,
                        resource_class: CapacityResourceClass::LlmGenerator,
                        capacity: 10,
                        reserved_floor: 0,
                        epoch: 1,
                        policy_digest: "d".repeat(64),
                        updated_at_ms: 0,
                        throttle: None,
                    },
                    expected_epoch: None,
                    now_ms: 1,
                },
            },
        )
        .await
        .unwrap();
    h.state.write().await.persistence = Some(backend);
    h
}

fn proposal() -> serde_json::Value {
    serde_json::json!({
        "proposal_id": "p-1", "tenant_id": TENANT, "resource_class_iri": CELL,
        "unit": "llm_generator", "current_capacity": 6, "proposed_capacity": 9,
        "min_capacity": 2, "max_capacity": 10, "measured_load": 5,
        "available_headroom": 4, "window_start_ms": 1, "window_end_ms": 2,
        "premise_revision": 1, "policy_revision": 1,
        "proposal_digest": "a".repeat(64),
    })
}

fn request(schema: &ComponentDependency, body: serde_json::Value) -> DecideRequest {
    declared_request(
        schema,
        TENANT,
        QuestionKind::CapacityProposal,
        "verdict",
        DecisionPolicyRef::Default,
        vec![
            TypedParam {
                name: "graph".to_string(),
                value: TypedValue::Text(GRAPH.to_string()),
            },
            TypedParam {
                name: "proposal".to_string(),
                value: TypedValue::Text(body.to_string()),
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

// spec: MEDULLA-DECISION-ENGINE-R824
#[tokio::test]
async fn capacity_proposal_verdicts() {
    let h = capacity_harness().await;
    let (schema, _) = publish_route_schema(&h);
    let with = |key: &str, value: serde_json::Value| {
        let mut body = proposal();
        body[key] = value;
        body
    };

    let batch = decide(&h, request(&schema, proposal())).await.unwrap();
    assert_eq!(answer(&batch), "allow");
    assert!(batch.records.as_slice()[0]
        .premises
        .iter()
        .any(|p| p.fact == "headroom:cell-llm:10:epoch=1"));

    let cases = [
        (
            with("proposal_digest", "ABC".into()),
            CapacityRefusalCode::DigestInvalid,
        ),
        (
            with("tenant_id", "tenant-other".into()),
            CapacityRefusalCode::TenantMismatch,
        ),
        (
            with("premise_revision", 0.into()),
            CapacityRefusalCode::PremiseStale,
        ),
        (
            with("measured_load", 7.into()),
            CapacityRefusalCode::MeasurementInvalid,
        ),
        (
            with("unit", "gpu".into()),
            CapacityRefusalCode::UnitMismatch,
        ),
        (
            with("window_end_ms", 1.into()),
            CapacityRefusalCode::WindowInvalid,
        ),
        (
            with("min_capacity", 8.into()),
            CapacityRefusalCode::LimitInvalid,
        ),
        (
            with("proposed_capacity", 11.into()),
            CapacityRefusalCode::LimitExceeded,
        ),
        (
            with("available_headroom", 2.into()),
            CapacityRefusalCode::HeadroomExceeded,
        ),
    ];
    for (body, code) in cases {
        let batch = decide(&h, request(&schema, body)).await.unwrap();
        assert_eq!(
            answer(&batch),
            format!("refuse:{}", code.code().as_str()),
            "{code:?}"
        );
    }

    // The ledger's observation is the engine's: a supplied one is refused.
    let mut supplied = request(&schema, proposal());
    let mut params = supplied.params.as_slice().to_vec();
    params.push(TypedParam {
        name: "observation".to_string(),
        value: TypedValue::Text("{}".to_string()),
    });
    params.sort_by(|a, b| a.name.cmp(&b.name));
    supplied.params = medulla_types::contract::BoundedVec::new(params).unwrap();
    let error = decide(&h, supplied).await.unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");

    // A foreign tenant is refused before any read.
    let mut foreign = request(&schema, proposal());
    foreign.tenant_id = "tenant-other".to_string();
    let error = decide(&h, foreign).await.unwrap_err();
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");
}
