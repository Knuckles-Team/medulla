use medulla_enrich::semantic::{
    classify_with_floor, fit_topics, LabelledVector, TopicCentroid, HOLDOUT_BP_DEFAULT,
    KNOB_OUT_OF_BOUNDS, TOPIC_EXAMPLES_TOO_FEW, VECTOR_DIMENSION_MISMATCH,
};
use medulla_enrich::Id;

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}

fn example(topic: &str, n: u32, vector: [f32; 2]) -> LabelledVector {
    LabelledVector {
        topic: id(topic),
        vector: vector.to_vec(),
        example: id(&format!("{topic}:ex{n}")),
    }
}

/// Two well-separated topics of six examples each.
fn fixed_examples() -> Vec<LabelledVector> {
    let storage = [[1.0, 0.0], [1.0, 0.1], [0.9, 0.05], [1.0, -0.1], [0.95, 0.0], [1.0, 0.2]];
    let network = [[0.0, 1.0], [0.1, 1.0], [0.05, 0.9], [-0.1, 1.0], [0.0, 0.95], [0.2, 1.0]];
    let mut examples = Vec::new();
    for (n, vector) in storage.into_iter().enumerate() {
        examples.push(example("topic:storage", n as u32, vector));
    }
    for (n, vector) in network.into_iter().enumerate() {
        examples.push(example("topic:network", n as u32, vector));
    }
    examples
}

// spec: MEDULLA-DECISION-ENGINE-R614.2.5
#[test]
fn centroids_floor_and_refusals_on_fixed_examples() {
    let examples = fixed_examples();
    let fit = fit_topics(&examples, HOLDOUT_BP_DEFAULT).unwrap();
    assert_eq!(fit.holdout_bp, HOLDOUT_BP_DEFAULT);
    assert_eq!(
        fit.topics.iter().map(|t| t.topic.as_str()).collect::<Vec<_>>(),
        ["topic:network", "topic:storage"]
    );

    // Deterministic for one seed, whatever the input order.
    assert_eq!(fit, fit_topics(&examples, HOLDOUT_BP_DEFAULT).unwrap());
    let mut reversed = examples.clone();
    reversed.reverse();
    assert_eq!(fit, fit_topics(&reversed, HOLDOUT_BP_DEFAULT).unwrap());

    // 20 percent of six examples is two (rounded up) set aside, four train.
    for topic in &fit.topics {
        assert_eq!(topic.holdout_examples.len(), 2);
        assert_eq!(topic.train_examples, 4);
        let norm: f32 = topic.centroid.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "unit length, got {norm}");
    }

    // The clusters separate, so every hold-out example is classified right: precision is 10000,
    // and the floor is the lowest confidence among the hold-out examples of the topic.
    let centroids: Vec<TopicCentroid> = fit
        .topics
        .iter()
        .map(|t| TopicCentroid {
            topic: t.topic.clone(),
            centroid: t.centroid.clone(),
            floor_bp: None,
        })
        .collect();
    for topic in &fit.topics {
        assert_eq!(topic.holdout_precision_bp, 10_000);
        let lowest = topic
            .holdout_examples
            .iter()
            .map(|held| {
                let vector = &examples.iter().find(|e| &e.example == held).unwrap().vector;
                let hit = classify_with_floor(held, vector, &centroids, 0).unwrap().unwrap();
                assert_eq!(hit.topic, topic.topic);
                hit.confidence.basis_points()
            })
            .min()
            .unwrap();
        assert_eq!(topic.floor_bp, lowest);
    }

    // Refusals: a topic with 4 examples, a vector of another width and a hold-out past 5000.
    let mut thin = fixed_examples();
    thin.retain(|e| !(e.topic.as_str() == "topic:network" && e.example.as_str().ends_with("ex5")));
    thin.retain(|e| !(e.topic.as_str() == "topic:network" && e.example.as_str().ends_with("ex4")));
    assert_eq!(
        fit_topics(&thin, HOLDOUT_BP_DEFAULT).unwrap_err().code,
        TOPIC_EXAMPLES_TOO_FEW
    );
    let mut wide = fixed_examples();
    wide[3].vector = vec![1.0, 0.0, 0.0];
    assert_eq!(
        fit_topics(&wide, HOLDOUT_BP_DEFAULT).unwrap_err().code,
        VECTOR_DIMENSION_MISMATCH
    );
    assert_eq!(fit_topics(&examples, 5_001).unwrap_err().code, KNOB_OUT_OF_BOUNDS);
    assert!(fit_topics(&examples, 0).is_ok(), "no hold-out leaves every example to train");
}

// spec: MEDULLA-DECISION-ENGINE-R614.2.5
#[test]
fn a_topic_the_holdout_cannot_separate_gets_the_highest_floor() {
    // Both topics sit on one direction, so the hold-out of one is assigned to whichever topic
    // wins the tie; no suffix of the predictions reaches 90 percent for both topics.
    let mut examples = Vec::new();
    for n in 0..6 {
        examples.push(example("topic:a", n, [1.0, 0.0]));
        examples.push(example("topic:b", n, [1.0, 0.0]));
    }
    let fit = fit_topics(&examples, HOLDOUT_BP_DEFAULT).unwrap();
    let worst = fit.topics.iter().map(|t| t.holdout_precision_bp).min().unwrap();
    assert!(worst < 9_000, "precision {worst}");
    for topic in fit.topics.iter().filter(|t| t.holdout_precision_bp < 9_000) {
        assert_eq!(topic.floor_bp, 10_000);
    }
}
