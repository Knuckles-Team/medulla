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

fn penalised_request(
    fixture: &RouteFixture,
    data: &LabelledDataset,
    key: &str,
    importance_on_first: Option<i64>,
) -> DecisionFitRequest {
    let DecisionFitOp::Submit { request } = super::scorer_tests::scorer_fit(fixture, data) else {
        panic!("submit")
    };
    let mut request = *request;
    request.head_kind = HeadKind::ListwiseLogistic;
    request.idempotency_key = key.into();
    if let Some(weight) = importance_on_first {
        request.penalty = Some(PenaltyObjective {
            schema_version: IMPORTANCE_SCHEMA_VERSION,
            loss_coefficient: quantised(1_000_000),
            importance_scale: quantised(5_000_000),
        });
        request.features = BoundedVec::new(vec![FeatureImportance::new(
            "text".into(),
            quantised(weight),
            "cd".repeat(32),
        )
        .unwrap()])
        .unwrap();
    }
    request
}

async fn submit_fit(h: &Harness, id: u64, request: DecisionFitRequest) -> Response {
    super::super::jobs::handle_decision_fit(
        &h.state,
        id,
        &verified(),
        DecisionFitOp::Submit {
            request: Box::new(request),
        },
    )
    .await
}

fn first_weight(
    draft: &medulla_decision_types::decision::statistical::head::DecisionHeadBody,
) -> f64 {
    medulla_decision_stats::decision::quant::value_of(draft.weights.as_slice()[0]).abs()
}

// spec: MEDULLA-DECISION-ENGINE-R151.3
#[tokio::test]
async fn fit_applies_the_penalty_from_the_request() {
    let h = Harness::new().await;
    let fixture = route_fixture(&h).await;
    let data = dataset(&fixture.schema_digest, &fixture.ids, &fixture.values, 400);

    let free: DecisionJobRecord =
        decode(submit_fit(&h, 2, penalised_request(&fixture, &data, "fit-free", None)).await)
            .unwrap();
    let DecisionJobOutput::Fit {
        draft: free_draft,
        penalty: free_penalty,
        ..
    } = succeeded(&free)
    else {
        panic!("fit output")
    };
    assert!(
        free_penalty.is_none(),
        "a plain fit carries no penalty receipt"
    );

    let pulled: DecisionJobRecord = decode(
        submit_fit(
            &h,
            3,
            penalised_request(&fixture, &data, "fit-pulled", Some(900_000)),
        )
        .await,
    )
    .unwrap();
    let DecisionJobOutput::Fit {
        draft: pulled_draft,
        penalty,
        ..
    } = succeeded(&pulled)
    else {
        panic!("fit output")
    };
    assert!(
        first_weight(pulled_draft) < first_weight(free_draft),
        "the high-importance feature moves less than in the free fit"
    );
    assert!(penalty.is_some(), "the draft records the penalty digest");

    // A request in another tenant's name never reaches the fit.
    let mut foreign = penalised_request(&fixture, &data, "fit-foreign", Some(900_000));
    foreign.tenant_id = "foreign".into();
    let refused: Result<DecisionJobRecord, String> = decode(submit_fit(&h, 4, foreign).await);
    assert!(refused.unwrap_err().starts_with("ACCESS_DENIED"));
}

// spec: MEDULLA-DECISION-ENGINE-R151.5
#[tokio::test]
async fn penalty_evidence() {
    let h = Harness::new().await;
    let fixture = route_fixture(&h).await;
    let data = dataset(&fixture.schema_digest, &fixture.ids, &fixture.values, 400);
    let request = penalised_request(&fixture, &data, "fit-evidence", Some(900_000));
    let expected_features = request.features.as_slice().to_vec();
    let job: DecisionJobRecord = decode(submit_fit(&h, 2, request).await).unwrap();
    let DecisionJobOutput::Fit {
        draft_sha256,
        draft_length,
        penalty,
        ..
    } = succeeded(&job).clone()
    else {
        panic!("fit output")
    };
    let receipt = penalty.expect("a penalised fit leaves a receipt");

    // Read the receipt back through a separately authorized status query.
    let status: DecisionJobRecord = decode(
        super::super::jobs::handle_decision_fit(
            &h.state,
            3,
            &verified(),
            DecisionFitOp::Status {
                request: medulla_decision_types::decision::DecisionJobStatusRequest {
                    tenant_id: TENANT.to_string(),
                    job_id: job.job_id.clone(),
                },
            },
        )
        .await,
    )
    .unwrap();
    let DecisionJobOutput::Fit {
        penalty: stored, ..
    } = succeeded(&status)
    else {
        panic!("stored fit output")
    };
    let stored = stored
        .as_ref()
        .expect("the stored output keeps the receipt");
    assert_eq!(**stored, *receipt);

    // Every field, and the digest over them.
    assert_eq!(receipt.features.as_slice(), expected_features.as_slice());
    assert_eq!(receipt.loss_coefficient, quantised(1_000_000));
    assert_eq!(receipt.importance_scale, quantised(5_000_000));
    assert_eq!(
        receipt.evidence_class,
        medulla_decision_types::decision::EvidenceClass::Observation
    );
    let sealed = medulla_decision_types::decision::importance_penalty::PenaltyReceipt::seal(
        &PenaltyObjective {
            schema_version: IMPORTANCE_SCHEMA_VERSION,
            loss_coefficient: quantised(1_000_000),
            importance_scale: quantised(5_000_000),
        },
        &expected_features,
    )
    .unwrap();
    assert_eq!(receipt.penalty_digest, sealed.penalty_digest);
    assert!(receipt.penalty_digest.starts_with("sha256:"));

    // The evaluation of the draft names the penalty as a premise; an
    // evaluation of an unpenalised draft names none.
    let eval =
        super::scorer_tests::evaluated(&h, 4, (draft_sha256, draft_length), data.clone()).await;
    assert_eq!(
        eval.premises.as_slice(),
        [format!("penalty:{}", receipt.penalty_digest)]
    );
    let plain: DecisionJobRecord =
        decode(submit_fit(&h, 5, penalised_request(&fixture, &data, "fit-plain", None)).await)
            .unwrap();
    let DecisionJobOutput::Fit {
        draft_sha256,
        draft_length,
        ..
    } = succeeded(&plain).clone()
    else {
        panic!("fit output")
    };
    let plain_eval =
        super::scorer_tests::evaluated(&h, 6, (draft_sha256, draft_length), data).await;
    assert!(plain_eval.premises.is_empty());
}
