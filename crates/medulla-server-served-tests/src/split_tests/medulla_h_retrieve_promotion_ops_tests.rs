//! Served proof of the two promotion-gate ops (MEDULLA-UNIFIED-DATA-PLANE-R040.6): both
//! go through the real request boundary, as a connector sends them.
use super::medulla_h_query_dispatch_pipeline_tests::{
    dispatch_state, refused, signed, worker_authority, TENANT, WORKER,
};
use super::medulla_h_query_sql_source_read_tests::{binding_draft, dispatch_table_fixture};
use medulla_index_types::semantic_index::{
    SemanticBindingState, SemanticIndexOp, SemanticQueueClass, SemanticStageLeasePage,
};
use medulla_server_api::test_support::decode_raw_response as ok;
use std::collections::BTreeMap;
use {
    medulla_catalog_source_types::embedding_promotion::ProbeSet,
    medulla_catalog_source_types::embedding_promotion::PromotionModality,
    medulla_catalog_source_types::embedding_promotion::PromotionProbePoint,
    medulla_catalog_source_types::embedding_promotion::SemanticModelConfig,
};

fn text_config() -> SemanticModelConfig {
    let probe = ProbeSet {
        k: 2,
        queries: vec![vec![1.0, 0.0]],
        ground_truth: vec![vec![1, 2]],
        recall_floor: 0.5,
    };
    SemanticModelConfig {
        model_id: "promotion-ops-model".to_string(),
        model_version: 1,
        modalities: vec![PromotionModality::Text],
        probe_set_digests: BTreeMap::from([(PromotionModality::Text, probe.digest())]),
        probe_set: BTreeMap::from([(PromotionModality::Text, probe)]),
    }
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R040.6
#[tokio::test]
async fn model_config_and_probe_measurement_are_served() {
    let worker = worker_authority();
    let fixture = dispatch_table_fixture(&worker, TENANT);
    let draft = binding_draft(&worker, &fixture.snapshot);
    let binding_id = draft.binding_id.clone();
    let state = dispatch_state(&fixture.persist_dir);
    let dispatch = medulla_server_router::dispatch::request_boundary::dispatch;

    let _: serde_json::Value = ok(
        "AdmitBinding",
        dispatch(
            &state,
            signed(
                940,
                SemanticIndexOp::AdmitBinding {
                    tenant_id: TENANT.to_string(),
                    binding_id: binding_id.clone(),
                    draft: Box::new(draft.clone()),
                    idempotency_key: "promotion-ops-admit".to_string(),
                },
            ),
        )
        .await,
    );

    // The config is installed through the served op and acknowledged.
    let installed: bool = ok(
        "InstallModelConfig",
        dispatch(
            &state,
            signed(
                941,
                SemanticIndexOp::InstallModelConfig {
                    tenant_id: TENANT.to_string(),
                    binding_id: binding_id.clone(),
                    config: Box::new(text_config()),
                    idempotency_key: "promotion-ops-config".to_string(),
                },
            ),
        )
        .await,
    );
    assert!(installed);

    // A config whose probe-set digest is not the pinned one refuses PROMOTION_BLOCKED.
    let mut bad = text_config();
    bad.probe_set_digests
        .insert(PromotionModality::Text, "0".repeat(64));
    let error = refused(
        "a config with a wrong probe-set digest",
        dispatch(
            &state,
            signed(
                942,
                SemanticIndexOp::InstallModelConfig {
                    tenant_id: TENANT.to_string(),
                    binding_id: binding_id.clone(),
                    config: Box::new(bad),
                    idempotency_key: "promotion-ops-bad-config".to_string(),
                },
            ),
        )
        .await,
    );
    assert!(error.contains("PROMOTION_BLOCKED"), "{error}");

    // Lease the admitted S1 and present it for an S4 or S5 measurement: the
    // lease names another stage, so both are refused.
    let _: serde_json::Value = ok(
        "AdmitSourceRecord",
        dispatch(
            &state,
            signed(
                943,
                SemanticIndexOp::AdmitSourceRecord {
                    tenant_id: TENANT.to_string(),
                    binding_id: binding_id.clone(),
                    record: Box::new(fixture.dirty.clone()),
                },
            ),
        )
        .await,
    );
    let _: serde_json::Value = ok(
        "TransitionBinding",
        dispatch(
            &state,
            signed(
                944,
                SemanticIndexOp::TransitionBinding {
                    tenant_id: TENANT.to_string(),
                    binding_id: binding_id.clone(),
                    expected_generation: draft.generation,
                    next_state: SemanticBindingState::Building,
                    idempotency_key: "promotion-ops-build".to_string(),
                },
            ),
        )
        .await,
    );
    let _: serde_json::Value = ok(
        "SubscribeStageConsumer",
        dispatch(
            &state,
            signed(
                945,
                SemanticIndexOp::SubscribeStageConsumer {
                    tenant_id: TENANT.to_string(),
                    binding_id: binding_id.clone(),
                    consumer: WORKER.to_string(),
                },
            ),
        )
        .await,
    );
    let page: SemanticStageLeasePage = ok(
        "ClaimStageLeases(Fast)",
        dispatch(
            &state,
            signed(
                946,
                SemanticIndexOp::ClaimStageLeases {
                    tenant_id: TENANT.to_string(),
                    binding_id: binding_id.clone(),
                    consumer: WORKER.to_string(),
                    queue_class: SemanticQueueClass::Fast,
                    limit: 1,
                    lease_ms: 60_000,
                },
            ),
        )
        .await,
    );
    let s1_lease = page.entries[0].lease.clone();
    for (id, stage) in [
        (947, PromotionProbePoint::Vector),
        (948, PromotionProbePoint::AnnIndex),
    ] {
        let error = refused(
            "a probe measurement under an S1 lease",
            dispatch(
                &state,
                signed(
                    id,
                    SemanticIndexOp::RecordProbeMeasurement {
                        tenant_id: TENANT.to_string(),
                        binding_id: binding_id.clone(),
                        lease: Box::new(s1_lease.clone()),
                        stage,
                        generation: draft.generation,
                        modality: PromotionModality::Text,
                        retrieved: vec![vec![1, 2]],
                    },
                ),
            )
            .await,
        );
        assert!(!error.is_empty());
    }
}
