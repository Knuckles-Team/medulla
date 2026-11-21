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
use medulla_types::contract::ServerErrorCode;

use crate::exec::PlanCtx;

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
    /// The fusion weight `w` of `score = (1 - w) * dense + w * topic`, in `[0, 1]`
    /// (the registry knob `retrieval.topic_weight`, resolved by the served planner;
    /// `None` until one is bound, and fusion refuses an unbound weight).
    pub weight: Option<f32>,
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
            weight: None,
        })
    }

    /// Bind the fusion weight `w`; refuses `TOPIC_WEIGHT_INVALID` outside `[0, 1]`.
    pub fn with_weight(mut self, weight: f32) -> Result<Self, String> {
        check_weight(weight).map_err(|refusal| refusal.to_string())?;
        self.weight = Some(weight);
        Ok(self)
    }
}

/// MEDULLA-FEDERATED-QUERY-R040.2.3: the topic evidence a served ranking step fuses in.
/// `backend` answers the topics of the query vector; `current_corpus_revision` is the graph
/// revision the query reads, which the topic result's corpus revision must equal, else the
/// fusion refuses `TOPIC_CORPUS_STALE` (`None`: the query binds no revision to compare with,
/// which refuses `TOPIC_BACKEND_UNAVAILABLE`).
#[derive(Clone)]
pub struct TopicFusion<'a> {
    pub backend: std::sync::Arc<dyn TopicBackend + 'a>,
    pub current_corpus_revision: Option<u64>,
}

/// MEDULLA-FEDERATED-QUERY-R040.2.3: the one fusion step every ranking leg takes after it
/// has scored its whole candidate set and before the result is truncated. With no topic
/// policy bound the dense ranking passes through unchanged. With one bound, the bound
/// backend weighs the topics against `query`, and [`fuse_topic_dense`] re-scores `dense`
/// against the store's declared space (the retriever). A policy with no backend, no
/// weight, or a store with no declared space refuses; the topic contribution is never
/// dropped silently.
pub(crate) fn fuse_ranked(
    ctx: &PlanCtx,
    query: &[f32],
    dense: Vec<(String, f32)>,
) -> Result<Vec<(String, f32)>, String> {
    let Some(policy) = ctx.topic_policy else {
        return Ok(dense);
    };
    let fusion = ctx.topic_fusion.as_ref().ok_or_else(|| {
        TopicRefusal::BackendUnavailable("no topic backend is bound on this query".to_string())
            .to_string()
    })?;
    let weight = policy.weight.unwrap_or(f32::NAN);
    let retriever = ctx.semantic.space().ok_or_else(|| {
        TopicRefusal::SpaceMismatch(
            "the store declares no embedding space to compare the topic centroids with".to_string(),
        )
        .to_string()
    })?;
    let current = fusion.current_corpus_revision.ok_or_else(|| {
        TopicRefusal::BackendUnavailable(
            "the query binds no graph revision to compare the topic corpus revision with"
                .to_string(),
        )
        .to_string()
    })?;
    let topics = fusion.backend.topics(query, &policy)?;
    let vector_of = |id: &str| ctx.semantic.get_embedding(id);
    fuse_topic_dense(
        &dense,
        &vector_of,
        &topics,
        weight,
        retriever,
        current,
    )
    .map_err(String::from)
}

/// `TOPIC_WEIGHT_INVALID` for a non-finite or out-of-range fusion weight.
fn check_weight(weight: f32) -> Result<(), TopicRefusal> {
    if weight.is_finite() && (0.0..=1.0).contains(&weight) {
        return Ok(());
    }
    Err(TopicRefusal::WeightInvalid { weight })
}

/// MEDULLA-FEDERATED-QUERY-R040.2.4: why topic fusion refused. Each refusal is raised
/// before any candidate is scored, and the requested topic contribution is never
/// dropped silently.
#[derive(Debug, Clone, PartialEq)]
pub enum TopicRefusal {
    /// The fusion weight is non-finite or outside `[0, 1]` (`TOPIC_WEIGHT_INVALID`).
    WeightInvalid {
        /// The refused weight.
        weight: f32,
    },
    /// The dimension, model revision or coordinate space of the topic result or of a
    /// candidate vector differs from the retriever's (`TOPIC_SPACE_MISMATCH`). Equal
    /// dimension alone is not compatible.
    SpaceMismatch(String),
    /// The centroids describe a corpus revision other than the current one
    /// (`TOPIC_CORPUS_STALE`).
    CorpusStale {
        /// The corpus revision the topic result describes.
        topics: u64,
        /// The corpus revision the retriever reads.
        current: u64,
    },
    /// The backend holds no topic evidence to fuse (`TOPIC_BACKEND_UNAVAILABLE`).
    BackendUnavailable(String),
}

impl TopicRefusal {
    /// The registered refusal code.
    pub fn code(&self) -> ServerErrorCode {
        match self {
            Self::WeightInvalid { .. } => ServerErrorCode::TopicWeightInvalid,
            Self::SpaceMismatch(_) => ServerErrorCode::TopicSpaceMismatch,
            Self::CorpusStale { .. } => ServerErrorCode::TopicCorpusStale,
            Self::BackendUnavailable(_) => ServerErrorCode::TopicBackendUnavailable,
        }
    }
}

impl std::fmt::Display for TopicRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = self.code();
        match self {
            Self::WeightInvalid { weight } => write!(
                f,
                "{code}: topic fusion weight {weight} must be finite and within [0, 1]"
            ),
            Self::SpaceMismatch(message) | Self::BackendUnavailable(message) => {
                write!(f, "{code}: {message}")
            }
            Self::CorpusStale { topics, current } => write!(
                f,
                "{code}: the topic centroids describe corpus revision {topics}, the corpus is at {current}"
            ),
        }
    }
}

impl std::error::Error for TopicRefusal {}

impl From<TopicRefusal> for String {
    fn from(refusal: TopicRefusal) -> Self {
        refusal.to_string()
    }
}

/// The identity a topic result and a candidate vector must share with the retriever:
/// the pinned tuple `(id, dimensions, revision)` and the whole space digest, which
/// folds in the model and preprocessing hashes.
fn same_space(a: &EmbeddingSpaceRef, b: &EmbeddingSpaceRef) -> bool {
    a.id == b.id && a.dimensions == b.dimensions && a.revision == b.revision && a.digest == b.digest
}

/// MEDULLA-FEDERATED-QUERY-R040.2.2, policy `weighted_topic_dense`:
/// `score = (1 - w) * dense + w * topic` for every candidate.
///
/// `dense` is each candidate's similarity, min-max normalized over the candidate set
/// (a set with no spread normalizes to `0`). `topic` is the sum over the backend's
/// topics of the topic's responsibility times the cosine similarity between the
/// candidate's vector and the topic's centroid. Ties break by candidate id ascending.
/// A pure function: hard constraints have already narrowed `dense`, and it reads no
/// state.
///
/// MEDULLA-FEDERATED-QUERY-R040.2.4: before any candidate is scored the weight, the
/// topic result's space against the `retriever`'s, and the topic corpus revision
/// against `current_corpus_revision` are checked; a candidate vector must have the
/// retriever's dimension and a candidate with no vector refuses too, so the topic
/// contribution is never dropped silently.
pub fn fuse_topic_dense(
    dense: &[(String, f32)],
    vector_of: &dyn Fn(&str) -> Option<Vec<f32>>,
    topics: &TopicResult,
    weight: f32,
    retriever: &EmbeddingSpaceRef,
    current_corpus_revision: u64,
) -> Result<Vec<(String, f32)>, TopicRefusal> {
    check_weight(weight)?;
    if !same_space(&topics.space, retriever) {
        return Err(TopicRefusal::SpaceMismatch(format!(
            "the topic centroids live in space `{}` ({} dimensions, revision `{}`), the retriever in `{}` ({} dimensions, revision `{}`)",
            topics.space.id,
            topics.space.dimensions,
            topics.space.revision,
            retriever.id,
            retriever.dimensions,
            retriever.revision
        )));
    }
    if topics.corpus_revision != current_corpus_revision {
        return Err(TopicRefusal::CorpusStale {
            topics: topics.corpus_revision,
            current: current_corpus_revision,
        });
    }
    let (low, high) = dense
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), (_, s)| {
            (low.min(*s), high.max(*s))
        });
    let spread = high - low;
    let mut fused = Vec::with_capacity(dense.len());
    for (id, score) in dense {
        let normalized = if spread > 0.0 { (score - low) / spread } else { 0.0 };
        let vector = vector_of(id).ok_or_else(|| {
            TopicRefusal::SpaceMismatch(format!(
                "candidate `{id}` has no stored embedding to compare with the topic centroids"
            ))
        })?;
        if vector.len() != retriever.dimensions {
            return Err(TopicRefusal::SpaceMismatch(format!(
                "candidate `{id}` has {} dimensions, the retriever's space has {}",
                vector.len(),
                retriever.dimensions
            )));
        }
        let vector_norm = norm(&vector);
        let topic: f32 = topics
            .topic_weights
            .iter()
            .zip(&topics.centroid_vectors)
            .map(|((_, responsibility), (_, centroid))| {
                let denominator = vector_norm * norm(centroid);
                let cosine = if denominator > 0.0 {
                    dot(&vector, centroid) / denominator
                } else {
                    0.0
                };
                responsibility * cosine
            })
            .sum();
        fused.push((id.clone(), (1.0 - weight) * normalized + weight * topic));
    }
    fused.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(fused)
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
    if centroids.is_empty() {
        return Err(TopicRefusal::BackendUnavailable(
            "the backend holds no topic centroids".to_string(),
        )
        .to_string());
    }
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

    // spec: MEDULLA-FEDERATED-QUERY-R040.2.2
    #[test]
    fn order_matches_oracle_w0_w1_mid() {
        let policy = TopicRankingPolicy::new(3, 0.0).unwrap();
        let memory = InMemoryTopicBackend {
            centroids: fixture(),
            corpus_revision: 7,
            space: space(),
        };
        let topics = memory.topics(&[1.0, 0.5], &policy).unwrap();
        // (id, dense similarity, candidate vector). `d` ties `c` on dense.
        let candidates: Vec<(&str, f32, Vec<f32>)> = vec![
            ("a", 0.9, vec![0.0, 1.0]),
            ("b", 0.5, vec![1.0, 0.0]),
            ("c", 0.2, vec![1.0, 1.0]),
            ("d", 0.2, vec![1.0, 0.2]),
        ];
        let dense: Vec<(String, f32)> = candidates
            .iter()
            .map(|(id, score, _)| (id.to_string(), *score))
            .collect();
        let lookup = |id: &str| {
            candidates
                .iter()
                .find(|(candidate, _, _)| *candidate == id)
                .map(|(_, _, vector)| vector.clone())
        };

        // The oracle recomputes every score from the definition in f64.
        let oracle = |weight: f64| -> Vec<(String, f64)> {
            let (low, high) = (0.2_f64, 0.9_f64);
            let mut scores: Vec<(String, f64)> = candidates
                .iter()
                .map(|(id, score, vector)| {
                    let normalized = (f64::from(*score) - low) / (high - low);
                    let mut topic = 0.0_f64;
                    for ((_, responsibility), (_, centroid)) in
                        topics.topic_weights.iter().zip(&topics.centroid_vectors)
                    {
                        let dot: f64 = vector
                            .iter()
                            .zip(centroid)
                            .map(|(x, y)| f64::from(*x) * f64::from(*y))
                            .sum();
                        let norms = vector.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt()
                            * centroid.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
                        topic += f64::from(*responsibility) * dot / norms;
                    }
                    (id.to_string(), (1.0 - weight) * normalized + weight * topic)
                })
                .collect();
            scores.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            scores
        };

        for weight in [0.0_f32, 1.0, 0.5] {
            let fused =
                fuse_topic_dense(&dense, &lookup, &topics, weight, &space(), 7).unwrap();
            let expected = oracle(f64::from(weight));
            let got: Vec<&str> = fused.iter().map(|(id, _)| id.as_str()).collect();
            let want: Vec<&str> = expected.iter().map(|(id, _)| id.as_str()).collect();
            assert_eq!(got, want, "order at w={weight}");
            for ((_, score), (_, want)) in fused.iter().zip(&expected) {
                assert!((f64::from(*score) - want).abs() < 1e-5, "score at w={weight}");
            }
        }
        // w = 0 is the dense order alone, ties by id ascending.
        let dense_only = fuse_topic_dense(&dense, &lookup, &topics, 0.0, &space(), 7).unwrap();
        let order: Vec<&str> = dense_only.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(order, ["a", "b", "c", "d"]);
        // A weight outside [0, 1] or non-finite refuses TOPIC_WEIGHT_INVALID.
        for bad in [-0.1_f32, 1.1, f32::NAN, f32::INFINITY] {
            let refusal =
                fuse_topic_dense(&dense, &lookup, &topics, bad, &space(), 7).unwrap_err();
            assert_eq!(refusal.code(), ServerErrorCode::TopicWeightInvalid);
            assert!(TopicRankingPolicy::new(1, 0.0).unwrap().with_weight(bad).is_err());
        }
        // A candidate with no vector is refused, never silently unweighted.
        let missing = |_: &str| None;
        let refusal =
            fuse_topic_dense(&dense, &missing, &topics, 0.5, &space(), 7).unwrap_err();
        assert_eq!(refusal.code(), ServerErrorCode::TopicSpaceMismatch);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R040.2.4
    #[test]
    fn incompatible_topics_refuse_before_fusion() {
        let policy = TopicRankingPolicy::new(3, 0.0).unwrap();
        let memory = InMemoryTopicBackend {
            centroids: fixture(),
            corpus_revision: 7,
            space: space(),
        };
        let topics = memory.topics(&[1.0, 0.5], &policy).unwrap();
        let dense = vec![("a".to_string(), 0.9_f32), ("b".to_string(), 0.5)];
        let lookup = |_: &str| Some(vec![1.0_f32, 0.0]);
        let run = |topics: &TopicResult, retriever: &EmbeddingSpaceRef, current: u64| {
            fuse_topic_dense(&dense, &lookup, topics, 0.5, retriever, current)
        };
        assert!(run(&topics, &space(), 7).is_ok());

        // Equal dimension alone is not compatible: another model revision, another
        // model id and another coordinate space (hashes) each refuse.
        let same_width = |id: &str, revision: &str, model_hash: &str| {
            EmbeddingSpaceRef::pinned(id, revision, model_hash, "b".repeat(64), 2, true).unwrap()
        };
        for other in [
            same_width("model", "2", &"a".repeat(64)),
            same_width("other", "1", &"a".repeat(64)),
            same_width("model", "1", &"c".repeat(64)),
        ] {
            assert_eq!(other.dimensions, space().dimensions);
            let refusal = run(&topics, &other, 7).unwrap_err();
            assert_eq!(refusal.code(), ServerErrorCode::TopicSpaceMismatch);
        }
        let wider = EmbeddingSpaceRef::pinned("model", "1", "a".repeat(64), "b".repeat(64), 3, true)
            .unwrap();
        assert_eq!(
            run(&topics, &wider, 7).unwrap_err().code(),
            ServerErrorCode::TopicSpaceMismatch
        );
        // A candidate vector of another width refuses.
        let narrow = |_: &str| Some(vec![1.0_f32]);
        assert_eq!(
            fuse_topic_dense(&dense, &narrow, &topics, 0.5, &space(), 7)
                .unwrap_err()
                .code(),
            ServerErrorCode::TopicSpaceMismatch
        );
        // A stale corpus refuses.
        let stale = run(&topics, &space(), 8).unwrap_err();
        assert_eq!(stale.code(), ServerErrorCode::TopicCorpusStale);
        assert!(stale.to_string().starts_with("TOPIC_CORPUS_STALE"));
        // A backend with no centroids is unavailable, never an empty fusion.
        let empty = InMemoryTopicBackend {
            centroids: Vec::new(),
            corpus_revision: 7,
            space: space(),
        };
        let unavailable = empty.topics(&[1.0, 0.5], &policy).unwrap_err();
        assert!(unavailable.starts_with("TOPIC_BACKEND_UNAVAILABLE"), "{unavailable}");
        let core = stored(&[]);
        let view = core.analysis_snapshot();
        let stored_backend = StoredCentroidTopicBackend::new(&view, 7, space());
        assert!(stored_backend
            .topics(&[1.0, 0.5], &policy)
            .unwrap_err()
            .starts_with("TOPIC_BACKEND_UNAVAILABLE"));
        assert_eq!(
            TopicRefusal::BackendUnavailable(String::new()).code(),
            ServerErrorCode::TopicBackendUnavailable
        );
    }
}
