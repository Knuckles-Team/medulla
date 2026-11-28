//! Fitted topics persist as `Topic` nodes and `classify` reads their stored floor
//! (MEDULLA-DECISION-ENGINE-R614.2.6). The write set is the one `FitTopics` commits through the
//! ChangeEnvelope route; the test applies its methods to a graph core the way the commit does.

use medulla_enrich::semantic::{
    classify_with_floor, fit_topics, LabelledVector, HOLDOUT_BP_DEFAULT,
};
use medulla_enrich::Id;
use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::Method;
use {
    medulla_h_ingest_repository::semantic_enrichment::lower_fit,
    medulla_h_ingest_repository::semantic_enrichment::stored_topics,
};

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}

fn examples(spread: f32) -> Vec<LabelledVector> {
    let mut out = Vec::new();
    for n in 0..6u32 {
        let wobble = spread * n as f32;
        out.push(LabelledVector {
            topic: id("topic:storage"),
            vector: vec![1.0, wobble * 0.1],
            example: id(&format!("storage:{n}")),
        });
        out.push(LabelledVector {
            topic: id("topic:network"),
            vector: vec![wobble * 0.1, 1.0],
            example: id(&format!("network:{n}")),
        });
    }
    out
}

/// Commit a write set to the core: nodes by id (a repeat replaces the properties) and the
/// centroid as the node's embedding.
fn apply(core: &GraphCore, methods: &[Method]) {
    for method in methods {
        match method {
            Method::AddNode {
                node_id,
                properties_msgpack,
            } => core.add_node(node_id.clone(), properties_msgpack.clone()),
            Method::AddEmbedding { node_id, embedding } => core
                .semantic_store
                .write()
                .add_embedding(node_id.clone(), embedding.clone())
                .unwrap(),
            other => panic!("unexpected method {other:?}"),
        }
    }
}

fn stored(core: &GraphCore, taxonomy: &str) -> Vec<medulla_enrich::semantic::TopicCentroid> {
    let store = core.semantic_store.read();
    let rows = core
        .get_nodes_by_label("Topic", 0)
        .into_iter()
        .filter_map(|(topic_id, bytes)| {
            let value = medulla_types::msgpack::decode_property_value(&bytes).ok()?;
            Some((topic_id, value))
        });
    stored_topics(rows, |topic_id| store.get_embedding(topic_id), taxonomy).unwrap()
}

// spec: MEDULLA-DECISION-ENGINE-R614.2.6
#[test]
fn fit_writes_topics_and_classify_reads_floor() {
    let core = GraphCore::new();
    let first = fit_topics(&examples(1.0), HOLDOUT_BP_DEFAULT).unwrap();
    let write = lower_fit(&first, "subjects", "record:1").unwrap().unwrap();
    apply(&core, write.methods());

    // One Topic node per topic with its floor and hold-out precision, and the centroid as the
    // node's embedding.
    let topics = stored(&core, "subjects");
    assert_eq!(
        topics.iter().map(|t| t.topic.as_str()).collect::<Vec<_>>(),
        ["topic:network", "topic:storage"]
    );
    for (stored, fitted) in topics.iter().zip(&first.topics) {
        assert_eq!(stored.centroid, fitted.centroid);
        assert_eq!(stored.floor_bp, Some(fitted.floor_bp));
    }
    let node = core.get_node_properties("topic:storage").unwrap();
    let node = medulla_types::msgpack::decode_property_value(&node).unwrap();
    assert_eq!(node["type"], "Topic");
    assert_eq!(node["taxonomy"], "subjects");
    assert_eq!(
        node["holdout_precision_bp"],
        first.topics[1].holdout_precision_bp
    );
    assert!(stored(&core, "another-taxonomy").is_empty());

    // A document on the storage centroid classifies, and its confidence reaches the stored floor.
    let doc = id("doc:1");
    let centroid = first.topics[1].centroid.clone();
    let hit = classify_with_floor(&doc, &centroid, &topics, 0)
        .unwrap()
        .unwrap();
    assert_eq!(hit.topic.as_str(), "topic:storage");
    assert!(hit.confidence.basis_points() >= first.topics[1].floor_bp);

    // A refit of the taxonomy replaces the nodes by id, and classify uses the new floor.
    let second = fit_topics(&examples(0.0), HOLDOUT_BP_DEFAULT).unwrap();
    assert_ne!(first.topics[1].centroid, second.topics[1].centroid);
    let refit = lower_fit(&second, "subjects", "record:2").unwrap().unwrap();
    apply(&core, refit.methods());
    let after = stored(&core, "subjects");
    assert_eq!(
        after.len(),
        2,
        "the refit replaced the nodes, it added none"
    );
    for (stored, fitted) in after.iter().zip(&second.topics) {
        assert_eq!(stored.centroid, fitted.centroid);
        assert_eq!(stored.floor_bp, Some(fitted.floor_bp));
    }
    // The refit measured parallel hold-out examples, so its floor is 10000 and a document short of
    // it is dropped; the same topics without their stored floors keep it under a zero cut.
    assert!(after.iter().all(|t| t.floor_bp == Some(10_000)));
    let near = [0.9, 0.1];
    assert!(classify_with_floor(&doc, &near, &after, 0)
        .unwrap()
        .is_none());
    let mut unfloored = after.clone();
    for topic in &mut unfloored {
        topic.floor_bp = None;
    }
    assert!(classify_with_floor(&doc, &near, &unfloored, 0)
        .unwrap()
        .is_some());
}
