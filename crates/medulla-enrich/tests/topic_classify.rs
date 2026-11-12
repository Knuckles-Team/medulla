use medulla_enrich::semantic::{
    classify, classify_with_floor, EnrichmentKnobs, TopicCentroid, KNOB_OUT_OF_BOUNDS,
    TAXONOMY_EMPTY, VECTOR_DIMENSION_MISMATCH,
};
use medulla_enrich::Id;

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}

fn topic(name: &str, centroid: &[f32]) -> TopicCentroid {
    TopicCentroid {
        topic: id(name),
        centroid: centroid.to_vec(),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R614.2.3
#[test]
fn classify_picks_nearest_topic_and_respects_floor() {
    let doc = id("doc:1");
    let topics = [
        topic("topic:storage", &[1.0, 0.0]),
        topic("topic:network", &[0.0, 1.0]),
    ];

    let hit = classify(&doc, &[0.9, 0.1], &topics).unwrap().unwrap();
    assert_eq!(hit.topic.as_str(), "topic:storage");
    assert_eq!(hit.document, doc);
    // cos = 0.9 / sqrt(0.82) = 0.99388..., floor to 9938 bp.
    assert_eq!(hit.confidence.basis_points(), 9_938);

    // Exactly parallel: 10000, never above.
    let exact = classify(&doc, &[2.0, 0.0], &topics).unwrap().unwrap();
    assert_eq!(exact.confidence.basis_points(), 10_000);

    // Equidistant: ties go to the lowest topic id, whatever the order.
    let tie_a = classify(&doc, &[1.0, 1.0], &topics).unwrap().unwrap();
    let reversed = [topics[1].clone(), topics[0].clone()];
    let tie_b = classify(&doc, &[1.0, 1.0], &reversed).unwrap().unwrap();
    assert_eq!(tie_a.topic.as_str(), "topic:network");
    assert_eq!(tie_a, tie_b);

    // Below the default floor (3000 bp) nothing is assigned.
    assert_eq!(
        classify(&doc, &[0.1, 0.0], &[topic("topic:x", &[0.2, 1.0])]).unwrap(),
        None
    );
    // Negative cosine clamps to zero, which is below any non-zero floor and
    // passes a floor of zero.
    let opposite = [topic("topic:x", &[-1.0, 0.0])];
    assert_eq!(classify(&doc, &[1.0, 0.0], &opposite).unwrap(), None);
    let zero = classify_with_floor(&doc, &[1.0, 0.0], &opposite, 0)
        .unwrap()
        .unwrap();
    assert_eq!(zero.confidence.basis_points(), 0);

    // The floor is a knob: a stricter one drops the 9938 hit.
    assert_eq!(
        classify_with_floor(&doc, &[0.9, 0.1], &topics, 9_939).unwrap(),
        None
    );

    assert_eq!(
        classify(&doc, &[1.0, 0.0], &[]).unwrap_err().code,
        TAXONOMY_EMPTY
    );
    assert_eq!(
        classify(&doc, &[1.0], &topics).unwrap_err().code,
        VECTOR_DIMENSION_MISMATCH
    );
}

// spec: MEDULLA-DECISION-ENGINE-R614.2.3
#[test]
fn enrichment_knobs_refuse_values_outside_their_range() {
    assert!(EnrichmentKnobs::new(20, 3_000, 10_000).is_ok());
    assert!(EnrichmentKnobs::new(1, 0, 100).is_ok());
    assert!(EnrichmentKnobs::new(200, 10_000, 100_000).is_ok());
    for bad in [
        EnrichmentKnobs::new(0, 3_000, 10_000),
        EnrichmentKnobs::new(201, 3_000, 10_000),
        EnrichmentKnobs::new(20, 10_001, 10_000),
        EnrichmentKnobs::new(20, 3_000, 99),
        EnrichmentKnobs::new(20, 3_000, 100_001),
    ] {
        assert_eq!(bad.unwrap_err().code, KNOB_OUT_OF_BOUNDS);
    }
    assert_eq!(
        EnrichmentKnobs::default(),
        EnrichmentKnobs::new(20, 3_000, 10_000).unwrap()
    );
}
