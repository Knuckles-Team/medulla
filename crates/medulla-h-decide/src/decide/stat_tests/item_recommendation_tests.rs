//! Item recommendation by residual quantization (MEDULLA-DECISION-ENGINE-R175.2):
//! pause steps refine the query toward the history, so the ranking over a
//! seeded catalog of four clusters never loses NDCG.

use medulla_decision_stats::decision::recommend::{
    encode_catalog, fit_codebooks, refine_and_rank, CatalogItem,
};
use medulla_decision_types::decision::knob_families::recommend::RecommendKnobs;

const TOP_K: u16 = 6;

fn catalog() -> Vec<CatalogItem> {
    let mut items = Vec::new();
    for cluster in 0..4usize {
        for member in 0..6usize {
            let mut vector = vec![0.05f32 * member as f32; 4];
            vector[cluster] = 1.0 + 0.01 * member as f32;
            items.push(CatalogItem {
                item_id: format!("c{cluster}-i{member}"),
                time_bucket: 0,
                vector,
            });
        }
    }
    items
}

fn ndcg(ranked: &[String], relevant_prefix: &str) -> f64 {
    let gain = |position: usize| 1.0 / ((position as f64) + 2.0).log2();
    let dcg: f64 = ranked
        .iter()
        .enumerate()
        .filter(|(_, id)| id.starts_with(relevant_prefix))
        .map(|(position, _)| gain(position))
        .sum();
    let ideal: f64 = (0..usize::from(TOP_K)).map(gain).sum();
    dcg / ideal
}

fn ranked_ids(pause_steps: u8) -> Vec<String> {
    let knobs = RecommendKnobs {
        levels: 2,
        codes_per_level: 4,
        kmeans_iters: 10,
        pause_alpha: 0.5,
        nearest_pull: 0.25,
    };
    let items = catalog();
    let mut digest = [0u8; 32];
    digest[..8].copy_from_slice(&0x0123_4567_89ab_cdefu64.to_be_bytes());
    let encoder = fit_codebooks(&items, &digest, &knobs).expect("fit");
    let encoded = encode_catalog(&encoder, &items).expect("encode");
    refine_and_rank(
        &encoder,
        &encoded,
        &[0.7, 0.6, 0.0, 0.0],
        &["c0-i0".to_string(), "c0-i1".to_string()],
        pause_steps,
        TOP_K,
        &knobs,
    )
    .expect("rank")
    .into_iter()
    .map(|item| item.item_id)
    .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R175.2, MEDULLA-DECISION-ENGINE-R175.6
#[test]
fn pause_steps_lift_ndcg() {
    let without = ndcg(&ranked_ids(0), "c0-");
    let with = ndcg(&ranked_ids(2), "c0-");
    assert!(with >= without, "{with} < {without}");
}

mod served {
    //! The served journey: a catalog graph, the encoder fitted through `Learn`
    //! and the question decided against it (MEDULLA-DECISION-ENGINE-R175.3,
    //! R175.4, R175.5, R175.8).

    use super::super::kernel_family_support::{decide_as_other_tenant, kernel_request};
    use super::super::learning_tests::graph_with;
    use super::super::path_preference_tests::pins;
    use super::super::retrieval_tests::learn;
    use super::super::*;
    use medulla_decision_types::decision::statistical::retrieval::{
        LearningRecorded, LearningWrite,
    };
    use medulla_decision_types::decision::statistical::{TypedParam, TypedValue};
    use medulla_decision_types::decision::PremiseClass;
    use medulla_types::evidence_class::EvidenceClass;

    const CATALOG: &str = "catalog-items";

    /// Four clusters of six items in four dimensions.
    fn rows() -> Vec<(String, Vec<f32>)> {
        let mut rows = Vec::new();
        for cluster in 0..4usize {
            for member in 0..6usize {
                let mut vector = vec![0.05f32 * member as f32; 4];
                vector[cluster] = 1.0 + 0.01 * member as f32;
                rows.push((format!("c{cluster}-i{member}"), vector));
            }
        }
        rows
    }

    fn fit(catalog: &str) -> LearningWrite {
        LearningWrite::FitCatalogEncoder {
            catalog_ref: catalog.to_string(),
        }
    }

    fn recommend_request(
        schema: &ComponentDependency,
        query: &[f32],
        pause_steps: u8,
        catalog: &str,
    ) -> DecideRequest {
        let body = serde_json::json!({
            "schema_version": 1,
            "catalog_ref": catalog,
            "query_embedding": query,
            "history_item_ids": ["c0-i0", "c0-i1"],
            "pause_steps": pause_steps,
            "top_k": 6,
            "as_of": 0,
        });
        let mut request = kernel_request(
            schema,
            QuestionKind::ItemRecommendation,
            "recommend",
            TypedValue::Text(body.to_string()),
            &["catalog"],
        );
        request.params = BoundedVec::new(vec![TypedParam {
            name: "recommend".to_string(),
            value: TypedValue::Text(body.to_string()),
        }])
        .unwrap();
        request
    }

    fn abstain_code(batch: &DecisionBatch) -> String {
        format!("{:?}", batch.records.as_slice()[0].outcome)
    }

    // spec: MEDULLA-DECISION-ENGINE-R175.8
    #[tokio::test]
    async fn fit_stores_an_encoder_keyed_by_catalog_digest() {
        let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
        graph_with(&h, CATALOG, "1", 4, rows()).await;
        let recorded = learn(&h, "decider", fit(CATALOG)).await.unwrap();
        let LearningRecorded::CatalogEncoder(fitted) = recorded else {
            panic!("a catalog encoder receipt");
        };
        assert_eq!((fitted.items, fitted.dimension), (24, 4));
        assert_eq!(fitted.catalog_digest.len(), 64);
        let key = medulla_decision_types::decision::item_recommendation::encoder_key(
            CATALOG,
            &fitted.catalog_digest,
        );
        assert!(h.store.decision_artifact(TENANT, &key).unwrap().is_some());
        // The same catalog fits to the same bytes: a replay, not a conflict.
        let again = learn(&h, "decider", fit(CATALOG)).await.unwrap();
        assert!(matches!(again, LearningRecorded::CatalogEncoder(_)));
        // Another tenant's store holds no such encoder.
        assert!(h
            .store
            .decision_artifact("tenant-other", &key)
            .unwrap()
            .is_none());
    }

    // spec: MEDULLA-DECISION-ENGINE-R175.3
    #[tokio::test]
    async fn decide_routes_item_recommendation_to_the_stored_encoder() {
        let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
        let pins = pins(&h, TENANT);
        graph_with(&h, CATALOG, "1", 4, rows()).await;
        learn(&h, "decider", fit(CATALOG)).await.unwrap();
        let request = recommend_request(&pins.0, &[0.7, 0.6, 0.0, 0.0], 2, CATALOG);
        let batch = decide(&h, request.clone()).await.unwrap();
        match &batch.records.as_slice()[0].outcome {
            StatisticalOutcome::Advisory { scores, .. } => {
                assert_eq!(scores.len(), 6);
                assert!(scores
                    .iter()
                    .zip(scores.iter().skip(1))
                    .all(|(a, b)| a.score.value >= b.score.value));
            }
            other => panic!("an advisory ranking, got {other:?}"),
        }
        // Another tenant's caller reads no ranking.
        let refused = decide_as_other_tenant(&h, request).await;
        assert!(!refused.is_empty());
    }

    // spec: MEDULLA-DECISION-ENGINE-R175.4
    #[tokio::test]
    async fn item_recommendation_refusals() {
        let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
        let pins = pins(&h, TENANT);
        let query = [0.7f32, 0.6, 0.0, 0.0];
        // An empty catalog.
        graph_with(&h, "catalog-empty", "1", 4, Vec::new()).await;
        let batch = decide(&h, recommend_request(&pins.0, &query, 0, "catalog-empty"))
            .await
            .unwrap();
        assert!(abstain_code(&batch).contains("EMPTY_CATALOG"));
        // A query of another width.
        graph_with(&h, CATALOG, "1", 4, rows()).await;
        let batch = decide(&h, recommend_request(&pins.0, &[1.0, 0.0], 0, CATALOG))
            .await
            .unwrap();
        assert!(abstain_code(&batch).contains("DIMENSION_MISMATCH"));
        // No encoder was fitted.
        let batch = decide(&h, recommend_request(&pins.0, &query, 0, CATALOG))
            .await
            .unwrap();
        assert!(abstain_code(&batch).contains("ENCODER_MISSING"));
        // An encoder fitted on a catalog that has since changed is stale.
        learn(&h, "decider", fit(CATALOG)).await.unwrap();
        {
            let guard = h.state.read().await;
            let core = guard.registry.get(CATALOG).unwrap().core.clone();
            let props = serde_json::json!({"type": "Doc"});
            core.add_node("late".into(), rmp_serde::to_vec_named(&props).unwrap());
            core.semantic_store
                .write()
                .add_embedding("late".into(), vec![0.3, 0.3, 0.3, 0.3])
                .unwrap();
        }
        let batch = decide(&h, recommend_request(&pins.0, &query, 0, CATALOG))
            .await
            .unwrap();
        assert!(abstain_code(&batch).contains("ENCODER_STALE"));
        for record in &batch.records {
            assert!(!matches!(
                record.outcome,
                StatisticalOutcome::Advisory { .. }
            ));
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R175.5
    #[tokio::test]
    async fn record_lists_premises_and_claim() {
        let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
        let pins = pins(&h, TENANT);
        graph_with(&h, CATALOG, "1", 4, rows()).await;
        learn(&h, "decider", fit(CATALOG)).await.unwrap();
        let batch = decide(
            &h,
            recommend_request(&pins.0, &[0.7, 0.6, 0.0, 0.0], 2, CATALOG),
        )
        .await
        .unwrap();
        let record = &batch.records.as_slice()[0];
        let facts: Vec<&str> = record.premises.iter().map(|p| p.fact.as_str()).collect();
        assert!(facts.iter().any(|f| f.starts_with("encoder:")), "{facts:?}");
        assert!(facts.iter().any(|f| f.starts_with("catalog:")), "{facts:?}");
        assert!(facts.contains(&"pause_steps:2"), "{facts:?}");
        assert_eq!(facts.iter().filter(|f| f.starts_with("item:")).count(), 6);
        assert!(record
            .premises
            .iter()
            .filter(|p| p.fact.starts_with("item:"))
            .all(|p| p.class == PremiseClass::Observation));
        assert_eq!(record.evidence_class, EvidenceClass::Observation);

        // The NDCG reproduction persists as a benchmark claim: the ranking with
        // pause steps is at least as good as the one without them.
        let without = super::ndcg(&super::ranked_ids(0), "c0-");
        let with = super::ndcg(&super::ranked_ids(2), "c0-");
        let claim = medulla_mutation_types::benchmark_claim::BenchmarkClaim {
            comparison:
                medulla_mutation_types::benchmark_claim::BenchmarkComparison::NdcgItemRecommendation,
            configuration:
                "4 clusters of 6 items, top 6, 2 levels of 4 codes, 0 versus 2 pause steps"
                    .to_string(),
            measured_result: format!("ndcg@6 with pause steps {with:.6}, without {without:.6}"),
            passed: with >= without,
        };
        assert_eq!(claim.accept_as_superiority_claim(), Ok(()));
    }
}
