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
) -> Result<(Vec<(String, f32)>, Option<medulla_types::retrieval::TopicReceipt>), String> {
    let Some(policy) = ctx.topic_policy else {
        return Ok((dense, None));
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
    // A topic result of another corpus revision refuses before any assignment is computed.
    if topics.corpus_revision != current {
        return Err(TopicRefusal::CorpusStale {
            topics: topics.corpus_revision,
            current,
        }
        .to_string());
    }
    let mut candidates = Vec::with_capacity(dense.len());
    let mut unembedded = None;
    for (id, _) in &dense {
        match ctx.semantic.get_embedding(id) {
            Some(embedding) => candidates.push((id.clone(), embedding)),
            None => {
                unembedded.get_or_insert_with(|| id.clone());
            }
        }
    }
    let assignments = fusion.backend.assignments(&candidates)?;
    let kept: Vec<String> = topics.topic_weights.iter().map(|(topic, _)| topic.clone()).collect();
    check_assignments(
        &candidates,
        &assignments,
        &kept,
        retriever,
        current,
        fusion.backend.id(),
        fusion.backend.revision(),
    )?;
    if let Some(id) = unembedded {
        return Err(TopicRefusal::AssignmentMissing(format!(
            "candidate `{id}` has no stored embedding to assign to the topics"
        ))
        .to_string());
    }
    let scored = fuse_topic_dense_scored(&dense, &assignments, &topics, weight, retriever, current)
        .map_err(String::from)?;
    let receipt = topic_receipt(
        fusion.backend.id(),
        fusion.backend.revision(),
        &topics,
        weight,
        retriever,
        &scored,
        &assignments,
        ctx.semantic.generation(),
    );
    Ok((
        scored.into_iter().map(|part| (part.id, part.fused)).collect(),
        Some(receipt),
    ))
}

/// A real number in millionths, rounded half away from zero.
fn micros(value: f32) -> i64 {
    (f64::from(value) * 1_000_000.0).round() as i64
}

/// The receipt of one fusion: a component entry for every fused candidate, in fused order.
fn topic_receipt(
    backend_id: &str,
    backend_revision: u64,
    topics: &TopicResult,
    weight: f32,
    retriever: &EmbeddingSpaceRef,
    scored: &[FusedScore],
    assignments: &[DocumentTopicAssignment],
    source_vector_generation: u64,
) -> medulla_types::retrieval::TopicReceipt {
    use medulla_types::retrieval::{
        TopicAssignmentTrace, TopicComponentScore, TopicFusionPolicy, TopicReceipt,
        TopicResponsibility,
    };
    // One entry per fused candidate and kept topic, in fused order, then topic id ascending.
    let mut traces = Vec::new();
    for part in scored {
        let mut own: Vec<&DocumentTopicAssignment> = assignments
            .iter()
            .filter(|a| {
                a.document_id == part.id
                    && topics.topic_weights.iter().any(|(topic, _)| *topic == a.topic_id)
            })
            .collect();
        own.sort_by(|a, b| a.topic_id.cmp(&b.topic_id));
        own.dedup_by(|a, b| a.topic_id == b.topic_id);
        traces.extend(own.into_iter().map(|a| TopicAssignmentTrace {
            candidate_id: a.document_id.clone(),
            topic_id: a.topic_id.clone(),
            content_revision: a.content_revision,
            responsibility_micros: micros(a.responsibility).clamp(0, 1_000_000) as u32,
        }));
    }
    let weight_micros = micros(weight).clamp(0, 1_000_000);
    TopicReceipt {
        backend_id: backend_id.to_string(),
        backend_revision,
        centroid_generation: topics.corpus_revision,
        space_tuple: format!(
            "{}|{}|{}",
            retriever.id, retriever.dimensions, retriever.revision
        ),
        responsibilities: topics
            .topic_weights
            .iter()
            .map(|(topic_id, responsibility)| TopicResponsibility {
                topic_id: topic_id.clone(),
                weight_micros: micros(*responsibility).clamp(0, 1_000_000) as u32,
            })
            .collect(),
        component_scores: scored
            .iter()
            .map(|part| {
                let dense_micros = micros(part.dense);
                let topic_micros = micros(part.topic);
                TopicComponentScore {
                    candidate_id: part.id.clone(),
                    dense_micros,
                    topic_micros,
                    final_micros: ((1_000_000 - weight_micros) * dense_micros
                        + weight_micros * topic_micros)
                        / 1_000_000,
                }
            })
            .collect(),
        fusion_policy: TopicFusionPolicy {
            name: FUSION_POLICY_NAME.to_string(),
            weight_micros: weight_micros as u32,
        },
        source_vector_generation,
        assignments: traces,
    }
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
    /// A candidate has no stored embedding, or no assignment for a kept topic
    /// (`TOPIC_ASSIGNMENT_MISSING`).
    AssignmentMissing(String),
    /// Two assignments share one document and topic (`TOPIC_ASSIGNMENT_DUPLICATE`).
    AssignmentDuplicate(String),
    /// A document's stored vector predates its current content
    /// (`TOPIC_ASSIGNMENT_STALE`).
    AssignmentStale {
        /// The document.
        document_id: String,
        /// The document's current content revision.
        content_revision: u64,
        /// The content revision its stored embedding was computed from.
        embedded_revision: u64,
    },
}

impl TopicRefusal {
    /// The registered refusal code.
    pub fn code(&self) -> ServerErrorCode {
        match self {
            Self::WeightInvalid { .. } => ServerErrorCode::TopicWeightInvalid,
            Self::SpaceMismatch(_) => ServerErrorCode::TopicSpaceMismatch,
            Self::CorpusStale { .. } => ServerErrorCode::TopicCorpusStale,
            Self::BackendUnavailable(_) => ServerErrorCode::TopicBackendUnavailable,
            Self::AssignmentMissing(_) => ServerErrorCode::TopicAssignmentMissing,
            Self::AssignmentDuplicate(_) => ServerErrorCode::TopicAssignmentDuplicate,
            Self::AssignmentStale { .. } => ServerErrorCode::TopicAssignmentStale,
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
            Self::SpaceMismatch(message)
            | Self::BackendUnavailable(message)
            | Self::AssignmentMissing(message)
            | Self::AssignmentDuplicate(message) => write!(f, "{code}: {message}"),
            Self::AssignmentStale {
                document_id,
                content_revision,
                embedded_revision,
            } => write!(
                f,
                "{code}: document `{document_id}` is at content revision {content_revision}, its stored embedding was computed from revision {embedded_revision}"
            ),
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
/// candidate's checked assignment for that topic. Ties break by candidate id ascending.
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
    assignments: &[DocumentTopicAssignment],
    topics: &TopicResult,
    weight: f32,
    retriever: &EmbeddingSpaceRef,
    current_corpus_revision: u64,
) -> Result<Vec<(String, f32)>, TopicRefusal> {
    Ok(fuse_topic_dense_scored(
        dense,
        assignments,
        topics,
        weight,
        retriever,
        current_corpus_revision,
    )?
    .into_iter()
    .map(|part| (part.id, part.fused))
    .collect())
}

/// One candidate's fused score with the two parts it was computed from
/// (MEDULLA-FEDERATED-QUERY-R040.2.5).
#[derive(Debug, Clone, PartialEq)]
pub struct FusedScore {
    /// The candidate id.
    pub id: String,
    /// The min-max normalized dense similarity.
    pub dense: f32,
    /// The summed responsibility-weighted centroid similarity.
    pub topic: f32,
    /// `(1 - w) * dense + w * topic`.
    pub fused: f32,
}

/// [`fuse_topic_dense`], keeping each candidate's dense and topic parts so a receipt reports
/// exactly the numbers the score was computed from. The one scorer both reach.
pub fn fuse_topic_dense_scored(
    dense: &[(String, f32)],
    assignments: &[DocumentTopicAssignment],
    topics: &TopicResult,
    weight: f32,
    retriever: &EmbeddingSpaceRef,
    current_corpus_revision: u64,
) -> Result<Vec<FusedScore>, TopicRefusal> {
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
        // The checked assignments (R040.2.7): the candidate's responsibility for each kept
        // topic, weighted by the query's responsibility for it.
        let mut topic = 0.0_f32;
        for (topic_id, responsibility) in &topics.topic_weights {
            let assigned = assignments
                .iter()
                .find(|a| a.document_id == *id && a.topic_id == *topic_id)
                .ok_or_else(|| {
                    TopicRefusal::AssignmentMissing(format!(
                        "candidate `{id}` has no assignment for topic `{topic_id}`"
                    ))
                })?;
            topic += responsibility * assigned.responsibility;
        }
        fused.push(FusedScore {
            id: id.clone(),
            dense: normalized,
            topic,
            fused: (1.0 - weight) * normalized + weight * topic,
        });
    }
    fused.sort_by(|a, b| b.fused.total_cmp(&a.fused).then_with(|| a.id.cmp(&b.id)));
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
    /// The backend's stable id, reported in the topic receipt.
    fn id(&self) -> &'static str;

    /// Weigh the backend's topics against `query` and cut the answer down with
    /// `policy`. Refuses a query whose width differs from the backend's space.
    fn topics(&self, query: &[f32], policy: &TopicRankingPolicy) -> Result<TopicResult, String>;

    /// The revision the backend reports for its topics: the graph revision of the read
    /// snapshot for the stored backend, the corpus revision for a fixture.
    fn revision(&self) -> u64;

    /// MEDULLA-FEDERATED-QUERY-R040.2.7: the assignment of every candidate (document id and
    /// stored embedding) to every stored topic. The backend only computes; the checks are
    /// made by [`check_assignments`].
    fn assignments(
        &self,
        candidates: &[(String, Vec<f32>)],
    ) -> Result<Vec<DocumentTopicAssignment>, String>;
}

/// MEDULLA-FEDERATED-QUERY-R040.2.7: one document's responsibility for one stored topic.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentTopicAssignment {
    pub document_id: String,
    /// The document's `content_revision` (absent means 0).
    pub content_revision: u64,
    /// The content revision its stored embedding was computed from.
    pub embedded_revision: u64,
    /// The backend's corpus revision.
    pub corpus_generation: u64,
    pub backend_id: String,
    pub backend_revision: u64,
    /// `<id>|<dimensions>|<revision>` of the backend's space.
    pub space_tuple: String,
    pub topic_id: String,
    /// `max(cosine, 0) / S` over the candidate's stored topics; all 0 when `S` is 0.
    pub responsibility: f32,
}

/// `<id>|<dimensions>|<revision>` of a space.
fn space_tuple(space: &EmbeddingSpaceRef) -> String {
    format!("{}|{}|{}", space.id, space.dimensions, space.revision)
}

/// What a backend stamps on every assignment it computes.
struct AssignmentStamp<'a> {
    backend_id: &'static str,
    backend_revision: u64,
    corpus_generation: u64,
    space: &'a EmbeddingSpaceRef,
}

/// Shared by both backends so they cannot disagree: for each candidate (by document id
/// ascending) and every stored topic (by topic id ascending, stored duplicates in stored
/// order) the clipped cosine over the candidate's sum of clipped cosines.
fn assign(
    centroids: &[(String, Vec<f32>)],
    candidates: &[(String, Vec<f32>)],
    revisions: &dyn Fn(&str) -> (u64, u64),
    stamp: &AssignmentStamp<'_>,
) -> Vec<DocumentTopicAssignment> {
    let mut ordered: Vec<&(String, Vec<f32>)> = candidates.iter().collect();
    ordered.sort_by(|a, b| a.0.cmp(&b.0));
    let mut topics: Vec<&(String, Vec<f32>)> = centroids.iter().collect();
    topics.sort_by(|a, b| a.0.cmp(&b.0));
    let tuple = space_tuple(stamp.space);
    let mut out = Vec::with_capacity(ordered.len() * topics.len());
    for (document, embedding) in ordered {
        let (content_revision, embedded_revision) = revisions(document);
        let embedding_norm = norm(embedding);
        let clipped: Vec<f32> = topics
            .iter()
            .map(|(_, centroid)| {
                let denominator = embedding_norm * norm(centroid);
                if denominator > 0.0 {
                    (dot(embedding, centroid) / denominator).max(0.0)
                } else {
                    0.0
                }
            })
            .collect();
        let total: f32 = clipped.iter().sum();
        for ((topic, _), value) in topics.iter().zip(clipped) {
            out.push(DocumentTopicAssignment {
                document_id: document.clone(),
                content_revision,
                embedded_revision,
                corpus_generation: stamp.corpus_generation,
                backend_id: stamp.backend_id.to_string(),
                backend_revision: stamp.backend_revision,
                space_tuple: tuple.clone(),
                topic_id: topic.clone(),
                responsibility: if total > 0.0 { value / total } else { 0.0 },
            });
        }
    }
    out
}

/// MEDULLA-FEDERATED-QUERY-R040.2.7: check a backend's assignments before any candidate is
/// scored, so every refusal precedes truncation and none falls back to dense-only ranking.
/// In order: (1) `TOPIC_SPACE_MISMATCH` for an assignment of another space tuple, backend
/// id or backend revision, or a candidate embedding whose width is not the retriever's;
/// (2) `TOPIC_ASSIGNMENT_MISSING` for a candidate with no assignment for a kept topic;
/// (3) `TOPIC_ASSIGNMENT_DUPLICATE` for two assignments of one document and topic;
/// (4) `TOPIC_ASSIGNMENT_STALE` for an embedding older than its document's content.
pub(crate) fn check_assignments(
    candidates: &[(String, Vec<f32>)],
    assignments: &[DocumentTopicAssignment],
    kept_topics: &[String],
    retriever: &EmbeddingSpaceRef,
    current_corpus_revision: u64,
    backend_id: &str,
    backend_revision: u64,
) -> Result<(), TopicRefusal> {
    let _ = current_corpus_revision;
    let tuple = space_tuple(retriever);
    for assignment in assignments {
        if assignment.space_tuple != tuple
            || assignment.backend_id != backend_id
            || assignment.backend_revision != backend_revision
        {
            return Err(TopicRefusal::SpaceMismatch(format!(
                "the assignment of document `{}` to topic `{}` was computed by backend `{}` at revision {} in space `{}`, the retriever reads backend `{backend_id}` at revision {backend_revision} in space `{tuple}`",
                assignment.document_id,
                assignment.topic_id,
                assignment.backend_id,
                assignment.backend_revision,
                assignment.space_tuple
            )));
        }
    }
    for (document, embedding) in candidates {
        if embedding.len() != retriever.dimensions {
            return Err(TopicRefusal::SpaceMismatch(format!(
                "candidate `{document}` has {} dimensions, the retriever's space has {}",
                embedding.len(),
                retriever.dimensions
            )));
        }
    }
    for (document, _) in candidates {
        for topic in kept_topics {
            if !assignments
                .iter()
                .any(|a| a.document_id == *document && a.topic_id == *topic)
            {
                return Err(TopicRefusal::AssignmentMissing(format!(
                    "candidate `{document}` has no assignment for topic `{topic}`"
                )));
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for assignment in assignments {
        if !seen.insert((assignment.document_id.as_str(), assignment.topic_id.as_str())) {
            return Err(TopicRefusal::AssignmentDuplicate(format!(
                "document `{}` has two assignments for topic `{}`",
                assignment.document_id, assignment.topic_id
            )));
        }
    }
    for assignment in assignments {
        if assignment.embedded_revision != assignment.content_revision {
            return Err(TopicRefusal::AssignmentStale {
                document_id: assignment.document_id.clone(),
                content_revision: assignment.content_revision,
                embedded_revision: assignment.embedded_revision,
            });
        }
    }
    Ok(())
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
    /// `(content_revision, embedded_revision)` by document id; absent means `(0, 0)`.
    pub revisions: std::collections::BTreeMap<String, (u64, u64)>,
}

#[cfg(test)]
impl TopicBackend for InMemoryTopicBackend {
    fn id(&self) -> &'static str {
        "in_memory"
    }

    fn topics(&self, query: &[f32], policy: &TopicRankingPolicy) -> Result<TopicResult, String> {
        weigh(
            &self.centroids,
            query,
            policy,
            self.corpus_revision,
            &self.space,
        )
    }

    fn revision(&self) -> u64 {
        self.corpus_revision
    }

    fn assignments(
        &self,
        candidates: &[(String, Vec<f32>)],
    ) -> Result<Vec<DocumentTopicAssignment>, String> {
        let revisions = |id: &str| self.revisions.get(id).copied().unwrap_or((0, 0));
        Ok(assign(
            &self.centroids,
            candidates,
            &revisions,
            &AssignmentStamp {
                backend_id: "in_memory",
                backend_revision: self.corpus_revision,
                corpus_generation: self.corpus_revision,
                space: &self.space,
            },
        ))
    }
}

/// The stable id of [`StoredCentroidTopicBackend`] in a topic receipt.
pub const STORED_CENTROID_BACKEND_ID: &str = "stored_centroid";

/// The fusion policy name in a topic receipt.
pub const FUSION_POLICY_NAME: &str = "weighted_topic_dense";

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
    fn id(&self) -> &'static str {
        STORED_CENTROID_BACKEND_ID
    }

    fn topics(&self, query: &[f32], policy: &TopicRankingPolicy) -> Result<TopicResult, String> {
        let centroids = self.stored_centroids()?;
        weigh(&centroids, query, policy, self.corpus_revision, &self.space)
    }

    fn revision(&self) -> u64 {
        self.corpus_revision
    }

    fn assignments(
        &self,
        candidates: &[(String, Vec<f32>)],
    ) -> Result<Vec<DocumentTopicAssignment>, String> {
        let centroids = self.stored_centroids()?;
        // The revisions of each candidate come from this backend's own graph view.
        let revisions = |id: &str| {
            let props = self.view.node_row_object(id);
            let read = |key: &str| {
                props
                    .as_ref()
                    .and_then(|props| props.get(key))
                    .and_then(|value| value.as_u64())
            };
            let content = read("content_revision").unwrap_or(0);
            (content, read("embedded_revision").unwrap_or(content))
        };
        Ok(assign(
            &centroids,
            candidates,
            &revisions,
            &AssignmentStamp {
                backend_id: STORED_CENTROID_BACKEND_ID,
                backend_revision: self.corpus_revision,
                corpus_generation: self.corpus_revision,
                space: &self.space,
            },
        ))
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
            revisions: Default::default(),
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
            revisions: Default::default(),
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
        let embedded: Vec<(String, Vec<f32>)> = candidates
            .iter()
            .map(|(id, _, vector)| (id.to_string(), vector.clone()))
            .collect();
        let assignments = memory.assignments(&embedded).unwrap();

        // The oracle recomputes every score from the definition in f64.
        let oracle = |weight: f64| -> Vec<(String, f64)> {
            let (low, high) = (0.2_f64, 0.9_f64);
            let mut scores: Vec<(String, f64)> = candidates
                .iter()
                .map(|(id, score, vector)| {
                    let normalized = (f64::from(*score) - low) / (high - low);
                    // The document's responsibility for every stored topic, then the
                    // query-weighted sum over the kept ones.
                    let cosine = |centroid: &Vec<f32>| -> f64 {
                        let dot: f64 = vector
                            .iter()
                            .zip(centroid)
                            .map(|(x, y)| f64::from(*x) * f64::from(*y))
                            .sum();
                        let norms = vector.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt()
                            * centroid.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
                        (dot / norms).max(0.0)
                    };
                    let total: f64 = fixture().iter().map(|(_, c)| cosine(c)).sum();
                    let mut topic = 0.0_f64;
                    for (kept, responsibility) in &topics.topic_weights {
                        let centroid = fixture().into_iter().find(|(t, _)| t == kept).unwrap().1;
                        let doc = if total > 0.0 { cosine(&centroid) / total } else { 0.0 };
                        topic += f64::from(*responsibility) * doc;
                    }
                    (id.to_string(), (1.0 - weight) * normalized + weight * topic)
                })
                .collect();
            scores.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            scores
        };

        for weight in [0.0_f32, 1.0, 0.5] {
            let fused =
                fuse_topic_dense(&dense, &assignments, &topics, weight, &space(), 7).unwrap();
            let expected = oracle(f64::from(weight));
            let got: Vec<&str> = fused.iter().map(|(id, _)| id.as_str()).collect();
            let want: Vec<&str> = expected.iter().map(|(id, _)| id.as_str()).collect();
            assert_eq!(got, want, "order at w={weight}");
            for ((_, score), (_, want)) in fused.iter().zip(&expected) {
                assert!((f64::from(*score) - want).abs() < 1e-5, "score at w={weight}");
            }
        }
        // w = 0 is the dense order alone, ties by id ascending.
        let dense_only = fuse_topic_dense(&dense, &assignments, &topics, 0.0, &space(), 7).unwrap();
        let order: Vec<&str> = dense_only.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(order, ["a", "b", "c", "d"]);
        // A weight outside [0, 1] or non-finite refuses TOPIC_WEIGHT_INVALID.
        for bad in [-0.1_f32, 1.1, f32::NAN, f32::INFINITY] {
            let refusal =
                fuse_topic_dense(&dense, &assignments, &topics, bad, &space(), 7).unwrap_err();
            assert_eq!(refusal.code(), ServerErrorCode::TopicWeightInvalid);
            assert!(TopicRankingPolicy::new(1, 0.0).unwrap().with_weight(bad).is_err());
        }
        // A candidate with no assignment is refused, never silently unweighted.
        let refusal = fuse_topic_dense(&dense, &[], &topics, 0.5, &space(), 7).unwrap_err();
        assert_eq!(refusal.code(), ServerErrorCode::TopicAssignmentMissing);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R040.2.4
    #[test]
    fn incompatible_topics_refuse_before_fusion() {
        let policy = TopicRankingPolicy::new(3, 0.0).unwrap();
        let memory = InMemoryTopicBackend {
            centroids: fixture(),
            corpus_revision: 7,
            space: space(),
            revisions: Default::default(),
        };
        let topics = memory.topics(&[1.0, 0.5], &policy).unwrap();
        let dense = vec![("a".to_string(), 0.9_f32), ("b".to_string(), 0.5)];
        let embedded: Vec<(String, Vec<f32>)> = ["a", "b"]
            .iter()
            .map(|id| (id.to_string(), vec![1.0_f32, 0.0]))
            .collect();
        let assignments = memory.assignments(&embedded).unwrap();
        let run = |topics: &TopicResult, retriever: &EmbeddingSpaceRef, current: u64| {
            fuse_topic_dense(&dense, &assignments, topics, 0.5, retriever, current)
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
        let narrow = vec![("a".to_string(), vec![1.0_f32])];
        let kept: Vec<String> = topics.topic_weights.iter().map(|(t, _)| t.clone()).collect();
        let narrow_assignments = memory.assignments(&narrow).unwrap();
        assert_eq!(
            check_assignments(&narrow, &narrow_assignments, &kept, &space(), 7, "in_memory", 7)
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
            revisions: Default::default(),
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

    // spec: MEDULLA-FEDERATED-QUERY-R040.2.5
    #[test]
    fn receipt_components_reproduce_the_final_score() {
        let topics = TopicResult {
            topic_weights: vec![("t1".into(), 0.75), ("t2".into(), 0.25)],
            centroid_vectors: vec![("t1".into(), vec![1.0, 0.0]), ("t2".into(), vec![0.0, 1.0])],
            corpus_revision: 7,
            space: space(),
        };
        let dense = vec![("a".to_string(), 0.9_f32), ("b".to_string(), 0.2)];
        let vectors = [("a", vec![1.0_f32, 0.0]), ("b", vec![0.6, 0.8])];
        let memory = InMemoryTopicBackend {
            centroids: vec![("t1".to_string(), vec![1.0, 0.0]), ("t2".to_string(), vec![0.0, 1.0])],
            corpus_revision: 7,
            space: space(),
            revisions: Default::default(),
        };
        let embedded: Vec<(String, Vec<f32>)> = vectors
            .iter()
            .map(|(id, vector)| (id.to_string(), vector.clone()))
            .collect();
        let assignments = memory.assignments(&embedded).unwrap();
        let weight = 0.5_f32;
        let scored =
            fuse_topic_dense_scored(&dense, &assignments, &topics, weight, &space(), 7).unwrap();
        let receipt = topic_receipt("stored_centroid", 7, &topics, weight, &space(), &scored, &assignments, 3);
        assert_eq!(receipt.backend_id, "stored_centroid");
        assert_eq!(receipt.centroid_generation, 7);
        assert_eq!(receipt.space_tuple, format!("model|2|1"));
        assert_eq!(receipt.fusion_policy.weight_micros, 500_000);
        assert_eq!(receipt.source_vector_generation, 3);
        // One assignment per fused candidate and kept topic, topic id ascending.
        let traced: Vec<(&str, &str)> = receipt
            .assignments
            .iter()
            .map(|a| (a.candidate_id.as_str(), a.topic_id.as_str()))
            .collect();
        assert_eq!(traced.len(), 4);
        assert!(traced.windows(2).all(|p| p[0].0 != p[1].0 || p[0].1 < p[1].1));
        let weights: Vec<u32> = receipt.responsibilities.iter().map(|r| r.weight_micros).collect();
        assert_eq!(weights, [750_000, 250_000]);
        let w = i64::from(receipt.fusion_policy.weight_micros);
        for part in &receipt.component_scores {
            let rebuilt = ((1_000_000 - w) * part.dense_micros + w * part.topic_micros) / 1_000_000;
            assert!((part.final_micros - rebuilt).abs() <= 1, "{part:?}");
        }
        let finals: Vec<i64> = receipt.component_scores.iter().map(|p| p.final_micros).collect();
        assert!(finals.windows(2).all(|pair| pair[0] >= pair[1]), "{finals:?}");
    }

    // spec: MEDULLA-FEDERATED-QUERY-R040.2.7
    #[test]
    fn assignments_match_the_clipped_cosine_oracle_and_sum_to_one() {
        let memory = InMemoryTopicBackend {
            centroids: fixture(),
            corpus_revision: 7,
            space: space(),
            revisions: Default::default(),
        };
        let candidates = vec![
            ("z".to_string(), vec![1.0_f32, 1.0]),
            ("a".to_string(), vec![-1.0_f32, 0.0]),
        ];
        let assignments = memory.assignments(&candidates).unwrap();
        // Ordered by document id, then topic id ascending.
        let keys: Vec<(&str, &str)> = assignments
            .iter()
            .map(|a| (a.document_id.as_str(), a.topic_id.as_str()))
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(keys.len(), 8);
        for document in ["a", "z"] {
            let own: Vec<&DocumentTopicAssignment> =
                assignments.iter().filter(|a| a.document_id == document).collect();
            let sum: f32 = own.iter().map(|a| a.responsibility).sum();
            assert!((sum - 1.0).abs() < 1e-5, "{document}: {sum}");
        }
        // `a` points opposite `sports`: it is clipped to zero there, and `opposite` takes it all.
        let opposite = assignments
            .iter()
            .find(|a| a.document_id == "a" && a.topic_id == "opposite")
            .unwrap();
        assert!((opposite.responsibility - 1.0).abs() < 1e-5);
        assert_eq!(opposite.space_tuple, "model|2|1");
        assert_eq!(opposite.backend_id, "in_memory");
    }

    // spec: MEDULLA-FEDERATED-QUERY-R040.2.7
    #[test]
    fn missing_duplicate_stale_and_mismatched_assignments_refuse_in_order() {
        let memory = InMemoryTopicBackend {
            centroids: fixture(),
            corpus_revision: 7,
            space: space(),
            revisions: Default::default(),
        };
        let candidates = vec![("d".to_string(), vec![1.0_f32, 0.0])];
        let kept = vec!["sports".to_string(), "finance".to_string()];
        let good = memory.assignments(&candidates).unwrap();
        let check = |assignments: &[DocumentTopicAssignment]| {
            check_assignments(&candidates, assignments, &kept, &space(), 7, "in_memory", 7)
        };
        assert_eq!(check(&good), Ok(()));

        let mut stale = good.clone();
        stale[0].embedded_revision = 1;
        stale[0].content_revision = 2;
        let mut duplicate = stale.clone();
        duplicate.push(duplicate[0].clone());
        let mut missing = duplicate.clone();
        missing.retain(|a| a.topic_id != "finance");
        let mut mismatched = missing.clone();
        mismatched[0].space_tuple = "other|2|1".to_string();

        // Each fault shadows the one after it, in the specified order.
        assert_eq!(
            check(&mismatched).unwrap_err().code(),
            ServerErrorCode::TopicSpaceMismatch
        );
        assert_eq!(
            check(&missing).unwrap_err().code(),
            ServerErrorCode::TopicAssignmentMissing
        );
        assert_eq!(
            check(&duplicate).unwrap_err().code(),
            ServerErrorCode::TopicAssignmentDuplicate
        );
        let refusal = check(&stale).unwrap_err();
        assert_eq!(refusal.code(), ServerErrorCode::TopicAssignmentStale);
        assert!(refusal.to_string().starts_with("TOPIC_ASSIGNMENT_STALE"));
        // A backend id or revision other than the topics result's is a space mismatch.
        assert_eq!(
            check_assignments(&candidates, &good, &kept, &space(), 7, "stored_centroid", 7)
                .unwrap_err()
                .code(),
            ServerErrorCode::TopicSpaceMismatch
        );
    }
}
