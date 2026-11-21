//! MEDULLA-FEDERATED-QUERY-R040.2.1: topic fusion backends.
//!
//! One result contract, two interchangeable sources of topic evidence: a
//! fixture ([`InMemoryTopicBackend`], test-only) and the stored centroid nodes
//! of the caller's graph snapshot ([`StoredCentroidTopicBackend`]). Both answer
//! with a [`TopicResult`], so a ranking step never learns which one produced
//! the weights. Each result names the retriever's [`EmbeddingSpaceRef`] and the
//! corpus revision it was computed at, so a later fusion step can refuse a
//! mismatched space or a stale corpus.

use medulla_graph_core::graph::GraphView;
use medulla_index_types::embedding::EmbeddingSpaceRef;

/// The label stored centroid nodes carry.
pub const TOPIC_CENTROID_LABEL: &str = "TopicCentroid";
/// The node property holding the topic id (the node id when absent).
pub const TOPIC_ID_KEY: &str = "topic";
/// The node property holding the centroid vector.
pub const TOPIC_CENTROID_KEY: &str = "centroid";

/// How a topic result is cut down before fusion. Both bounds come from the
/// caller's resolved settings; there is no built-in default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TopicRankingPolicy {
    /// The most topics a result keeps, heaviest first.
    pub top_topics: usize,
    /// Topics lighter than this (a weight in `[0, 1]`) are dropped.
    pub min_weight: f32,
}

impl TopicRankingPolicy {
    /// A policy; `top_topics` must be positive and `min_weight` in `[0, 1]`.
    pub fn new(top_topics: usize, min_weight: f32) -> Result<Self, String> {
        if top_topics == 0 {
            return Err("topic ranking policy top_topics must be positive".to_string());
        }
        if !(0.0..=1.0).contains(&min_weight) {
            return Err("topic ranking policy min_weight must be within [0, 1]".to_string());
        }
        Ok(Self {
            top_topics,
            min_weight,
        })
    }
}

/// What any [`TopicBackend`] answers.
#[derive(Debug, Clone, PartialEq)]
pub struct TopicResult {
    /// `(topic id, weight)`, heaviest first, ties by topic id; weights sum to 1
    /// over the kept topics.
    pub topic_weights: Vec<(String, f32)>,
    /// `(topic id, centroid)` for every kept topic, in the same order.
    pub centroid_vectors: Vec<(String, Vec<f32>)>,
    /// The corpus revision the centroids describe.
    pub corpus_revision: u64,
    /// The embedding space the centroids and the query vector live in.
    pub space: EmbeddingSpaceRef,
}

/// A source of topic evidence for one query vector.
pub trait TopicBackend: Send + Sync {
    /// Weigh the backend's topics against `query` and cut the answer down with
    /// `policy`. Refuses a query whose width differs from the backend's space.
    fn topics(&self, query: &[f32], policy: &TopicRankingPolicy) -> Result<TopicResult, String>;
}

/// Shared by both backends so they cannot disagree: cosine similarity clipped
/// at zero, normalized to sum 1, cut by the policy.
fn weigh(
    centroids: &[(String, Vec<f32>)],
    query: &[f32],
    policy: &TopicRankingPolicy,
    corpus_revision: u64,
    space: &EmbeddingSpaceRef,
) -> Result<TopicResult, String> {
    if query.len() != space.dimensions {
        return Err(format!(
            "topic query has {} dimensions but space `{}` has {}",
            query.len(),
            space.digest,
            space.dimensions
        ));
    }
    let query_norm = norm(query);
    let mut scored: Vec<(String, f32, &Vec<f32>)> = Vec::with_capacity(centroids.len());
    for (topic, centroid) in centroids {
        if centroid.len() != space.dimensions {
            return Err(format!(
                "topic `{topic}` centroid has {} dimensions but space `{}` has {}",
                centroid.len(),
                space.digest,
                space.dimensions
            ));
        }
        let denominator = query_norm * norm(centroid);
        let cosine = if denominator > 0.0 {
            dot(query, centroid) / denominator
        } else {
            0.0
        };
        scored.push((topic.clone(), cosine.max(0.0), centroid));
    }
    let total: f32 = scored.iter().map(|(_, w, _)| w).sum();
    if total > 0.0 {
        for entry in &mut scored {
            entry.1 /= total;
        }
    }
    scored.retain(|(_, weight, _)| *weight > 0.0 && *weight >= policy.min_weight);
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored.truncate(policy.top_topics);
    let kept: f32 = scored.iter().map(|(_, w, _)| w).sum();
    if kept > 0.0 {
        for entry in &mut scored {
            entry.1 /= kept;
        }
    }
    Ok(TopicResult {
        topic_weights: scored.iter().map(|(t, w, _)| (t.clone(), *w)).collect(),
        centroid_vectors: scored
            .iter()
            .map(|(t, _, c)| (t.clone(), (*c).clone()))
            .collect(),
        corpus_revision,
        space: space.clone(),
    })
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn norm(a: &[f32]) -> f32 {
    dot(a, a).sqrt()
}

/// A fixed set of centroids. Compiled only for tests.
#[cfg(test)]
pub struct InMemoryTopicBackend {
    pub centroids: Vec<(String, Vec<f32>)>,
    pub corpus_revision: u64,
    pub space: EmbeddingSpaceRef,
}

#[cfg(test)]
impl TopicBackend for InMemoryTopicBackend {
    fn topics(&self, query: &[f32], policy: &TopicRankingPolicy) -> Result<TopicResult, String> {
        weigh(
            &self.centroids,
            query,
            policy,
            self.corpus_revision,
            &self.space,
        )
    }
}

/// Reads `TopicCentroid` nodes from one graph snapshot.
pub struct StoredCentroidTopicBackend<'a> {
    view: &'a GraphView,
    corpus_revision: u64,
    space: EmbeddingSpaceRef,
}

impl<'a> StoredCentroidTopicBackend<'a> {
    /// A backend over `view`. `corpus_revision` is the revision of the snapshot
    /// the caller read it at; `space` is the space the stored centroids were
    /// embedded in.
    pub fn new(view: &'a GraphView, corpus_revision: u64, space: EmbeddingSpaceRef) -> Self {
        Self {
            view,
            corpus_revision,
            space,
        }
    }

    fn stored_centroids(&self) -> Result<Vec<(String, Vec<f32>)>, String> {
        let mut ids: Vec<&String> = self.view.node_properties.keys().collect();
        ids.sort();
        let mut centroids = Vec::new();
        for id in ids {
            let Some(props) = self.view.node_row_object(id) else {
                continue;
            };
            if !medulla_graph_core::graph::props_carry_label(&props, TOPIC_CENTROID_LABEL) {
                continue;
            }
            let topic = props
                .get(TOPIC_ID_KEY)
                .and_then(|v| v.as_str())
                .unwrap_or(id)
                .to_string();
            let Some(values) = props.get(TOPIC_CENTROID_KEY).and_then(|v| v.as_array()) else {
                return Err(format!("topic centroid node `{id}` has no centroid vector"));
            };
            let mut centroid = Vec::with_capacity(values.len());
            for value in values {
                let Some(component) = value.as_f64().filter(|c| c.is_finite()) else {
                    return Err(format!(
                        "topic centroid node `{id}` holds a non-numeric component"
                    ));
                };
                centroid.push(component as f32);
            }
            centroids.push((topic, centroid));
        }
        Ok(centroids)
    }
}

impl TopicBackend for StoredCentroidTopicBackend<'_> {
    fn topics(&self, query: &[f32], policy: &TopicRankingPolicy) -> Result<TopicResult, String> {
        let centroids = self.stored_centroids()?;
        weigh(&centroids, query, policy, self.corpus_revision, &self.space)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_graph_core::graph::GraphCore;
    use serde_json::json;

    fn space() -> EmbeddingSpaceRef {
        EmbeddingSpaceRef::pinned("model", "1", "a".repeat(64), "b".repeat(64), 2, true).unwrap()
    }

    fn fixture() -> Vec<(String, Vec<f32>)> {
        vec![
            ("sports".to_string(), vec![1.0, 0.0]),
            ("finance".to_string(), vec![0.0, 1.0]),
            ("opposite".to_string(), vec![-1.0, 0.0]),
            ("mixed".to_string(), vec![1.0, 1.0]),
        ]
    }

    fn stored(centroids: &[(String, Vec<f32>)]) -> GraphCore {
        let core = GraphCore::new();
        for (topic, centroid) in centroids {
            let blob = rmp_serde::to_vec_named(&json!({
                "type": TOPIC_CENTROID_LABEL,
                "topic": topic,
                "centroid": centroid,
            }))
            .unwrap();
            core.add_node(format!("node-{topic}"), blob);
        }
        // A node of another label must never read as a topic.
        let other =
            rmp_serde::to_vec_named(&json!({"type": "Doc", "centroid": [9.0, 9.0]})).unwrap();
        core.add_node("doc".into(), other);
        core
    }

    // spec: MEDULLA-FEDERATED-QUERY-R040.2.1
    #[test]
    fn two_backends_one_result_contract() {
        let policy = TopicRankingPolicy::new(3, 0.0).unwrap();
        let memory = InMemoryTopicBackend {
            centroids: fixture(),
            corpus_revision: 7,
            space: space(),
        };
        let core = stored(&fixture());
        let view = core.analysis_snapshot();
        let backend = StoredCentroidTopicBackend::new(&view, 7, space());
        let backends: [&dyn TopicBackend; 2] = [&memory, &backend];
        let query = [1.0_f32, 0.5];
        let mut results = Vec::new();
        for backend in backends {
            let result = backend.topics(&query, &policy).unwrap();
            assert_eq!(result.corpus_revision, 7);
            assert_eq!(result.space, space());
            assert_eq!(result.topic_weights.len(), result.centroid_vectors.len());
            // Heaviest first, weights sum to one, the opposite topic is clipped.
            let weights: Vec<f32> = result.topic_weights.iter().map(|(_, w)| *w).collect();
            assert!(weights.windows(2).all(|pair| pair[0] >= pair[1]));
            assert!((weights.iter().sum::<f32>() - 1.0).abs() < 1e-5);
            assert!(result.topic_weights.iter().all(|(t, _)| t != "opposite"));
            for (index, (topic, centroid)) in result.centroid_vectors.iter().enumerate() {
                assert_eq!(*topic, result.topic_weights[index].0);
                let expected = fixture().into_iter().find(|(t, _)| t == topic).unwrap().1;
                assert_eq!(*centroid, expected);
            }
            results.push(result);
        }
        // One contract: both backends answer identically.
        assert_eq!(results[0], results[1]);
        assert_eq!(results[0].topic_weights[0].0, "mixed");
        // The policy cuts: top one keeps only the heaviest at weight 1.
        let top_one = TopicRankingPolicy::new(1, 0.0).unwrap();
        let cut = memory.topics(&query, &top_one).unwrap();
        assert_eq!(cut.topic_weights, vec![("mixed".to_string(), 1.0)]);
        // A query in a different width refuses; so does an invalid policy.
        assert!(memory.topics(&[1.0, 0.0, 0.0], &policy).is_err());
        assert!(backend.topics(&[1.0, 0.0, 0.0], &policy).is_err());
        assert!(TopicRankingPolicy::new(0, 0.0).is_err());
        assert!(TopicRankingPolicy::new(1, 1.5).is_err());
    }
}
