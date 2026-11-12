//! `DecisionFit` carries caller-supplied feature importance
//! (MEDULLA-DECISION-ENGINE-R151.1): the model rides the served fit request,
//! a request without it is unchanged, and an out-of-range weight never
//! reaches the handler.

use super::*;
use medulla_decision_types::decision::importance_penalty::{
    FeatureImportance, PenaltyObjective, IMPORTANCE_SCHEMA_VERSION,
};
use medulla_decision_types::decision::numeric::{QuantScaleTag, QuantisedValue};

fn quantised(micros: i64) -> QuantisedValue {
    QuantisedValue {
        scale: QuantScaleTag::Pico,
        value: micros * 1_000_000,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R151.1
#[tokio::test]
async fn fit_with_importance_keeps_important_weights() {
    let h = Harness::new().await;
    let fixture = route_fixture(&h).await;
    let data = dataset(&fixture.schema_digest, &fixture.ids, &fixture.values, 400);
    let DecisionFitOp::Submit { request } = super::scorer_tests::scorer_fit(&fixture, &data) else {
        panic!("submit")
    };
    let mut request = *request;
    request.head_kind = HeadKind::ListwiseLogistic;
    request.idempotency_key = "fit-importance".into();
    request.penalty = Some(PenaltyObjective {
        schema_version: IMPORTANCE_SCHEMA_VERSION,
        loss_coefficient: quantised(1_000_000),
        importance_scale: quantised(500_000),
    });
    request.features = BoundedVec::new(vec![FeatureImportance::new(
        fixture.ids[0].clone(),
        quantised(900_000),
        "cd".repeat(32),
    )
    .unwrap()])
    .unwrap();

    // The request survives the wire the served method decodes.
    let wire = serde_json::to_value(&request).unwrap();
    let decoded: DecisionFitRequest = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(decoded, request);
    let mut bad = wire;
    bad["features"][0]["importance_weight"]["value"] = serde_json::json!(2_000_000_000_000_i64);
    assert!(serde_json::from_value::<DecisionFitRequest>(bad).is_err());

    let job: DecisionJobRecord = decode(
        super::super::jobs::handle_decision_fit(
            &h.state,
            2,
            &verified(),
            DecisionFitOp::Submit {
                request: Box::new(request),
            },
        )
        .await,
    )
    .unwrap();
    let DecisionJobOutput::Fit { draft, .. } = succeeded(&job) else {
        panic!("fit output")
    };
    assert_eq!(draft.kind, HeadKind::ListwiseLogistic);
}
