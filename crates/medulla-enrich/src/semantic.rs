//! Semantic enrichment records: capability cards, enhancement candidates,
//! spec drafts, topic assignments and projection rules
//! (MEDULLA-DECISION-ENGINE-R614.1).
//!
//! The model layer of the enrichment family: typed, identifier-validated
//! records the behaviour (R614.2), the expected-value scheduling (R614.3),
//! the `CARD_UNGROUNDED` refusal (R614.4) and the per-card evidence (R614.5)
//! build on. Every identifier is an [`Id`], validated at construction and
//! on the wire; nothing here calls a model or schedules work.

use serde::{Deserialize, Serialize};

use crate::{EnrichError, Id};

/// The stable code of a refused classification confidence.
pub const INVALID_CONFIDENCE: &str = "INVALID_CONFIDENCE";

/// The largest confidence any topic assignment accepts.
pub const MAX_CONFIDENCE_BASIS_POINTS: u32 = 10_000;

/// A classification confidence in basis points of 1.0 (1 = 0.01%).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct Confidence(u32);

impl Confidence {
    pub fn new(basis_points: u32) -> Result<Self, EnrichError> {
        if basis_points > MAX_CONFIDENCE_BASIS_POINTS {
            return Err(EnrichError::new(
                INVALID_CONFIDENCE,
                format!(
                    "confidence {basis_points} exceeds {MAX_CONFIDENCE_BASIS_POINTS} basis points"
                ),
            ));
        }
        Ok(Self(basis_points))
    }

    /// The confidence as basis points of 1.0.
    pub fn basis_points(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for Confidence {
    type Error = EnrichError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Confidence> for u32 {
    fn from(confidence: Confidence) -> Self {
        confidence.0
    }
}

/// A model-produced summary of one symbol, file or feature, stored with the
/// model and the prompt that produced it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityCard {
    pub id: Id,
    /// The symbol, file or feature the card summarizes.
    pub subject: Id,
    /// The summary text; it cites the source fact ids below.
    pub summary: String,
    /// The model that produced the summary.
    pub model: Id,
    /// The digest of the prompt that produced the summary.
    pub prompt_digest: String,
    /// The stored fact ids the summary cites.
    pub source_fact_ids: Vec<Id>,
}

/// A research concept ranked into an enhancement proposal by its value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnhancementCandidate {
    pub id: Id,
    /// The research concept the candidate is.
    pub concept: Id,
    /// The value the ranking orders candidates by.
    pub value: u32,
}

/// A spec distilled from ranked research concepts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecDraft {
    pub id: Id,
    pub title: String,
    pub body: String,
    /// The concepts the draft distills.
    pub source_concepts: Vec<Id>,
}

/// One document classified into the fixed subject taxonomy, linked to its
/// topic node with a confidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicAssignment {
    pub id: Id,
    /// The document the assignment classifies.
    pub document: Id,
    /// The topic node the document classifies into.
    pub topic: Id,
    /// How certain the classification is, in basis points.
    pub confidence: Confidence,
}

/// A declared rule projecting a node's own properties into the edges they
/// encode, under a declared taxonomy and reference rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionRule {
    pub id: Id,
    /// The taxonomy under which the projection is declared.
    pub taxonomy: Id,
    /// The node property the rule reads.
    pub property: Id,
    /// The edge relation the property encodes.
    pub relation: Id,
}

// ---------------------------------------------------------------------------
// Behaviour (MEDULLA-DECISION-ENGINE-R614.2): card storage, proposal ranking,
// taxonomy classification and property projection. Pure functions: the
// served `SemanticEnrich` method (R614.3) is their only caller.
// ---------------------------------------------------------------------------

/// Refusal: a card cites no source fact.
pub const CARD_UNGROUNDED: &str = "CARD_UNGROUNDED";
/// Refusal: a card cites a fact the tenant graph does not hold.
pub const CARD_FACT_UNKNOWN: &str = "CARD_FACT_UNKNOWN";
/// Refusal: the taxonomy has no topics.
pub const TAXONOMY_EMPTY: &str = "TAXONOMY_EMPTY";
/// Refusal: a projection rule names a taxonomy that is not declared.
pub const PROJECTION_RULE_UNKNOWN_TAXONOMY: &str = "PROJECTION_RULE_UNKNOWN_TAXONOMY";
/// Refusal: a knob value lies outside its declared range.
pub const KNOB_OUT_OF_BOUNDS: &str = "KNOB_OUT_OF_BOUNDS";
/// Refusal: a document vector and a centroid differ in dimension.
pub const VECTOR_DIMENSION_MISMATCH: &str = "VECTOR_DIMENSION_MISMATCH";

/// `enrichment.proposal_top_n`: default, range `[1, 200]`, proposals, Tunable.
pub const PROPOSAL_TOP_N_DEFAULT: u32 = 20;
pub const PROPOSAL_TOP_N_RANGE: (u32, u32) = (1, 200);
/// `enrichment.topic_min_confidence_bp`: default, range `[0, 10000]`, basis points, Tunable.
pub const TOPIC_MIN_CONFIDENCE_BP_DEFAULT: u32 = 3_000;
/// `enrichment.projection_batch`: default, range `[100, 100000]`, nodes, Tunable.
pub const PROJECTION_BATCH_DEFAULT: u32 = 10_000;
pub const PROJECTION_BATCH_RANGE: (u32, u32) = (100, 100_000);

/// The three tunable enrichment knobs, range-checked at construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnrichmentKnobs {
    pub proposal_top_n: u32,
    pub topic_min_confidence_bp: u32,
    pub projection_batch: u32,
}

impl Default for EnrichmentKnobs {
    fn default() -> Self {
        Self {
            proposal_top_n: PROPOSAL_TOP_N_DEFAULT,
            topic_min_confidence_bp: TOPIC_MIN_CONFIDENCE_BP_DEFAULT,
            projection_batch: PROJECTION_BATCH_DEFAULT,
        }
    }
}

impl EnrichmentKnobs {
    pub fn new(
        proposal_top_n: u32,
        topic_min_confidence_bp: u32,
        projection_batch: u32,
    ) -> Result<Self, EnrichError> {
        let check = |name: &str, value: u32, lo: u32, hi: u32| {
            if value < lo || value > hi {
                Err(EnrichError::new(
                    KNOB_OUT_OF_BOUNDS,
                    format!("knob enrichment.{name} = {value} is outside [{lo}, {hi}]"),
                ))
            } else {
                Ok(())
            }
        };
        check(
            "proposal_top_n",
            proposal_top_n,
            PROPOSAL_TOP_N_RANGE.0,
            PROPOSAL_TOP_N_RANGE.1,
        )?;
        check(
            "topic_min_confidence_bp",
            topic_min_confidence_bp,
            0,
            MAX_CONFIDENCE_BASIS_POINTS,
        )?;
        check(
            "projection_batch",
            projection_batch,
            PROJECTION_BATCH_RANGE.0,
            PROJECTION_BATCH_RANGE.1,
        )?;
        Ok(Self {
            proposal_top_n,
            topic_min_confidence_bp,
            projection_batch,
        })
    }
}

/// The stored node of one capability card. Its id derives from the subject
/// and the prompt digest, so a second card for the same pair carries the same
/// id and replaces the first when written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardNode {
    pub id: Id,
    pub subject: Id,
    /// The properties `summary`, `model`, `prompt_digest`, `source_fact_ids`.
    pub properties: std::collections::BTreeMap<String, String>,
}

/// Store a grounded capability card: it cites at least one source fact and
/// every cited fact exists in `known_fact_ids`.
pub fn store_card(
    card: &CapabilityCard,
    known_fact_ids: &std::collections::BTreeSet<Id>,
) -> Result<CardNode, EnrichError> {
    if card.source_fact_ids.is_empty() {
        return Err(EnrichError::new(
            CARD_UNGROUNDED,
            format!("card {} cites no source fact", card.id),
        ));
    }
    if let Some(missing) = card
        .source_fact_ids
        .iter()
        .find(|fact| !known_fact_ids.contains(fact))
    {
        return Err(EnrichError::new(
            CARD_FACT_UNKNOWN,
            format!("card {} cites unknown fact {missing}", card.id),
        ));
    }
    let digest = crate::sha256_hex(
        format!(
            "{}:{}\0{}:{}",
            card.subject.as_str().len(),
            card.subject,
            card.prompt_digest.len(),
            card.prompt_digest
        )
        .as_bytes(),
    );
    let id = Id::new(format!("card:{digest}"))?;
    let mut properties = std::collections::BTreeMap::new();
    properties.insert("summary".to_string(), card.summary.clone());
    properties.insert("model".to_string(), card.model.to_string());
    properties.insert("prompt_digest".to_string(), card.prompt_digest.clone());
    properties.insert(
        "source_fact_ids".to_string(),
        card.source_fact_ids
            .iter()
            .map(Id::as_str)
            .collect::<Vec<_>>()
            .join(","),
    );
    Ok(CardNode {
        id,
        subject: card.subject.clone(),
        properties,
    })
}

/// One research concept with its ranking inputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConceptInput {
    pub concept: Id,
    pub impact_bp: u32,
    pub confidence_bp: u32,
    pub cost_units: u32,
}

/// The integer value `impact_bp * confidence_bp / max(cost_units, 1)`,
/// clamped to `u32::MAX`.
pub fn candidate_value(impact_bp: u32, confidence_bp: u32, cost_units: u32) -> u32 {
    let value = u64::from(impact_bp) * u64::from(confidence_bp) / u64::from(cost_units.max(1));
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// Rank concepts into proposals with the default `enrichment.proposal_top_n`.
pub fn rank_candidates(inputs: &[ConceptInput]) -> Result<Vec<EnhancementCandidate>, EnrichError> {
    rank_candidates_top(inputs, PROPOSAL_TOP_N_DEFAULT)
}

/// Rank concepts into proposals: value descending, then concept id
/// ascending, at most `top_n`. A concept repeated in the input keeps its
/// highest value. The order does not depend on the input order.
pub fn rank_candidates_top(
    inputs: &[ConceptInput],
    top_n: u32,
) -> Result<Vec<EnhancementCandidate>, EnrichError> {
    let mut best: std::collections::BTreeMap<&Id, u32> = std::collections::BTreeMap::new();
    for input in inputs {
        let confidence = Confidence::new(input.confidence_bp)?;
        let value = candidate_value(input.impact_bp, confidence.basis_points(), input.cost_units);
        best.entry(&input.concept)
            .and_modify(|held| *held = (*held).max(value))
            .or_insert(value);
    }
    let mut ranked: Vec<(&Id, u32)> = best.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    ranked.truncate(top_n as usize);
    ranked
        .into_iter()
        .map(|(concept, value)| {
            Ok(EnhancementCandidate {
                id: Id::new(format!("cand:{concept}"))?,
                concept: concept.clone(),
                value,
            })
        })
        .collect()
}

/// One stored `Topic` node with its centroid vector.
#[derive(Clone, Debug, PartialEq)]
pub struct TopicCentroid {
    pub topic: Id,
    pub centroid: Vec<f32>,
    /// The confidence floor a fit stored with the topic (MEDULLA-DECISION-ENGINE-R614.2.6). It
    /// replaces the fixed cut of [`classify_with_floor`] when the topic wins; `None` for a
    /// topic no fit produced.
    pub floor_bp: Option<u32>,
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let (mut dot, mut na, mut nb) = (0f64, 0f64, 0f64);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (f64::from(*x), f64::from(*y));
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Classify a document with the default confidence floor.
pub fn classify(
    document: &Id,
    document_vector: &[f32],
    topics: &[TopicCentroid],
) -> Result<Option<TopicAssignment>, EnrichError> {
    classify_with_floor(
        document,
        document_vector,
        topics,
        TOPIC_MIN_CONFIDENCE_BP_DEFAULT,
    )
}

/// Classify a document into the topic with the highest cosine (ties to the
/// lowest topic id). The confidence is `max(0, cosine) * 10000` rounded down;
/// below `min_confidence_bp` the result is `None` and nothing is written.
pub fn classify_with_floor(
    document: &Id,
    document_vector: &[f32],
    topics: &[TopicCentroid],
    min_confidence_bp: u32,
) -> Result<Option<TopicAssignment>, EnrichError> {
    if topics.is_empty() {
        return Err(EnrichError::new(
            TAXONOMY_EMPTY,
            "the taxonomy has no topics",
        ));
    }
    let mut best: Option<(&TopicCentroid, f64)> = None;
    for topic in topics {
        if topic.centroid.len() != document_vector.len() {
            return Err(EnrichError::new(
                VECTOR_DIMENSION_MISMATCH,
                format!(
                    "topic {} has {} dimensions, the document {}",
                    topic.topic,
                    topic.centroid.len(),
                    document_vector.len()
                ),
            ));
        }
        let score = cosine(document_vector, &topic.centroid);
        let better = match best {
            None => true,
            Some((held, held_score)) => {
                score > held_score || (score == held_score && topic.topic < held.topic)
            }
        };
        if better {
            best = Some((topic, score));
        }
    }
    let Some((topic, score)) = best else {
        return Ok(None);
    };
    let basis_points =
        ((score.max(0.0) * 10_000.0).floor() as u32).min(MAX_CONFIDENCE_BASIS_POINTS);
    // A stored fit floor replaces the fixed cut for the winning topic.
    if basis_points < topic.floor_bp.unwrap_or(min_confidence_bp) {
        return Ok(None);
    }
    Ok(Some(TopicAssignment {
        id: Id::new(format!("assign:{document}:{}", topic.topic))?,
        document: document.clone(),
        topic: topic.topic.clone(),
        confidence: Confidence::new(basis_points)?,
    }))
}

/// The refusal of a topic with fewer than [`MIN_TOPIC_EXAMPLES`] examples.
pub const TOPIC_EXAMPLES_TOO_FEW: &str = "TOPIC_EXAMPLES_TOO_FEW";
/// The fewest examples one topic needs to be fitted.
pub const MIN_TOPIC_EXAMPLES: usize = 5;
/// The default share of each topic's examples set aside for the hold-out, in basis points.
pub const HOLDOUT_BP_DEFAULT: u16 = 2_000;
/// The largest hold-out share a fit accepts, in basis points.
pub const HOLDOUT_BP_MAX: u16 = 5_000;
/// The hold-out precision a floor must reach, in basis points.
pub const FLOOR_TARGET_PRECISION_BP: u64 = 9_000;
/// The seed of the split: examples order by the digest of this seed and their id.
const FIT_SPLIT_SEED: &str = "eg/fit-topics/v1";

/// One labelled example: the topic it belongs to, its vector and its id.
#[derive(Clone, Debug, PartialEq)]
pub struct LabelledVector {
    pub topic: Id,
    pub vector: Vec<f32>,
    pub example: Id,
}

/// One fitted topic (MEDULLA-DECISION-ENGINE-R614.2.5).
#[derive(Clone, Debug, PartialEq)]
pub struct FittedTopic {
    pub topic: Id,
    /// The mean of the topic's training vectors, scaled to unit length.
    pub centroid: Vec<f32>,
    /// The precision of the hold-out predictions that `classify` assigned to this topic, in basis
    /// points (0 when none was assigned).
    pub holdout_precision_bp: u32,
    /// The lowest `classify` confidence that reaches 90 percent precision on the hold-out, or
    /// 10000 when none does.
    pub floor_bp: u32,
    /// The examples the centroid was fitted from.
    pub train_examples: u32,
    /// The examples set aside, sorted.
    pub holdout_examples: Vec<Id>,
}

/// The fit of a whole taxonomy: its topics in topic id order.
#[derive(Clone, Debug, PartialEq)]
pub struct FittedTopics {
    pub holdout_bp: u16,
    pub topics: Vec<FittedTopic>,
}

fn split_rank(example: &Id) -> String {
    crate::sha256_hex(format!("{FIT_SPLIT_SEED}\0{example}").as_bytes())
}

fn unit_mean(vectors: &[&[f32]], dimensions: usize) -> Vec<f32> {
    let mut sum = vec![0f64; dimensions];
    for vector in vectors {
        for (acc, value) in sum.iter_mut().zip(*vector) {
            *acc += f64::from(*value);
        }
    }
    let norm = sum.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm == 0.0 {
        return vec![0.0; dimensions];
    }
    sum.iter().map(|v| (v / norm) as f32).collect()
}

/// Fit topic centroids from labelled examples (MEDULLA-DECISION-ENGINE-R614.2.5). Pure and
/// deterministic: the split depends only on the example ids, so the same examples in any order
/// give the same fit. A seeded split sets aside `holdout_bp` of each topic (at least one example
/// when it is positive, at least one is always left to train). The centroid of a topic is the mean
/// of its training vectors scaled to unit length. Each hold-out example is classified against all
/// the fitted centroids; the precision of a topic is the share of the hold-out examples assigned
/// to it that belong to it, and its floor is the lowest confidence at which the assigned examples
/// reach 90 percent precision (10000 when none does).
pub fn fit_topics(
    examples: &[LabelledVector],
    holdout_bp: u16,
) -> Result<FittedTopics, EnrichError> {
    if holdout_bp > HOLDOUT_BP_MAX {
        return Err(EnrichError::new(
            KNOB_OUT_OF_BOUNDS,
            format!("hold-out {holdout_bp} exceeds {HOLDOUT_BP_MAX} basis points"),
        ));
    }
    let Some(first) = examples.first() else {
        return Err(EnrichError::new(TAXONOMY_EMPTY, "no examples to fit"));
    };
    let dimensions = first.vector.len();
    let mut grouped: std::collections::BTreeMap<&Id, Vec<&LabelledVector>> =
        std::collections::BTreeMap::new();
    for example in examples {
        if example.vector.len() != dimensions || dimensions == 0 {
            return Err(EnrichError::new(
                VECTOR_DIMENSION_MISMATCH,
                format!(
                    "example {} has {} dimensions, the fit {dimensions}",
                    example.example,
                    example.vector.len()
                ),
            ));
        }
        grouped.entry(&example.topic).or_default().push(example);
    }
    for (topic, members) in &grouped {
        if members.len() < MIN_TOPIC_EXAMPLES {
            return Err(EnrichError::new(
                TOPIC_EXAMPLES_TOO_FEW,
                format!(
                    "topic {topic} has {} examples, at least {MIN_TOPIC_EXAMPLES} are needed",
                    members.len()
                ),
            ));
        }
    }
    // The split: order each topic's examples by their seeded rank, set the first aside.
    let mut centroids: Vec<TopicCentroid> = Vec::with_capacity(grouped.len());
    let mut held: Vec<(&Id, Vec<&LabelledVector>, u32)> = Vec::with_capacity(grouped.len());
    for (topic, mut members) in grouped {
        members.sort_by_cached_key(|member| (split_rank(&member.example), member.example.clone()));
        let want = (members.len() * usize::from(holdout_bp)).div_ceil(10_000);
        let holdout_count = want.min(members.len() - 1);
        let (aside, train) = members.split_at(holdout_count);
        let vectors: Vec<&[f32]> = train.iter().map(|m| m.vector.as_slice()).collect();
        centroids.push(TopicCentroid {
            topic: topic.clone(),
            centroid: unit_mean(&vectors, dimensions),
            floor_bp: None,
        });
        held.push((
            topic,
            aside.to_vec(),
            u32::try_from(train.len()).unwrap_or(u32::MAX),
        ));
    }
    // Classify every hold-out example against all centroids: (assigned topic, confidence, truth).
    let mut predictions: Vec<(Id, u32, bool)> = Vec::new();
    for (topic, aside, _) in &held {
        for member in aside {
            let assignment = classify_with_floor(&member.example, &member.vector, &centroids, 0)?
                .expect("a floor of zero assigns every document");
            let correct = assignment.topic == **topic;
            predictions.push((
                assignment.topic,
                assignment.confidence.basis_points(),
                correct,
            ));
        }
    }
    let mut topics = Vec::with_capacity(held.len());
    for ((topic, aside, train), centroid) in held.iter().zip(&centroids) {
        let mut mine: Vec<(u32, bool)> = predictions
            .iter()
            .filter(|(assigned, _, _)| assigned == *topic)
            .map(|(_, confidence, correct)| (*confidence, *correct))
            .collect();
        mine.sort_by_key(|(confidence, _)| *confidence);
        let total = mine.len() as u64;
        let correct_all = mine.iter().filter(|(_, ok)| *ok).count() as u64;
        let holdout_precision_bp = if total == 0 {
            0
        } else {
            u32::try_from(correct_all * 10_000 / total).unwrap_or(10_000)
        };
        // The lowest confidence whose suffix (every prediction at or above it) reaches the target.
        let mut floor_bp = MAX_CONFIDENCE_BASIS_POINTS;
        let mut distinct: Vec<u32> = mine.iter().map(|(confidence, _)| *confidence).collect();
        distinct.dedup();
        for candidate in distinct {
            let suffix: Vec<&(u32, bool)> = mine
                .iter()
                .filter(|(confidence, _)| *confidence >= candidate)
                .collect();
            let right = suffix.iter().filter(|(_, ok)| *ok).count() as u64;
            if right * 10_000 >= FLOOR_TARGET_PRECISION_BP * suffix.len() as u64 {
                floor_bp = candidate;
                break;
            }
        }
        let mut holdout_examples: Vec<Id> = aside.iter().map(|m| m.example.clone()).collect();
        holdout_examples.sort();
        topics.push(FittedTopic {
            topic: (*topic).clone(),
            centroid: centroid.centroid.clone(),
            holdout_precision_bp,
            floor_bp,
            train_examples: *train,
            holdout_examples,
        });
    }
    Ok(FittedTopics { holdout_bp, topics })
}

/// A node whose properties are projected into edges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionNode {
    pub id: Id,
    pub properties: std::collections::BTreeMap<String, String>,
}

/// One edge a projection emits; its id is the SHA-256 of node id, relation
/// and target id, so a replay writes the same edge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectedEdge {
    pub id: String,
    pub source: Id,
    pub relation: Id,
    pub target: Id,
}

/// The edges a projection emits and the count of property values that named
/// no node.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Projection {
    pub edges: Vec<ProjectedEdge>,
    pub unresolved: u32,
}

fn edge_id(source: &Id, relation: &Id, target: &Id) -> String {
    let mut preimage = String::new();
    for part in [source, relation, target] {
        preimage.push_str(&format!("{}:{}\0", part.as_str().len(), part));
    }
    crate::sha256_hex(preimage.as_bytes())
}

/// Project a node's properties into edges. For each rule, the rule property
/// of the node is read; when its value names an existing node (`resolve`),
/// one edge from the node to the named node is emitted. A value that names
/// no node emits nothing and counts as unresolved. A rule whose taxonomy is
/// not in `known_taxonomies` refuses the whole projection.
pub fn project_edges(
    node: &ProjectionNode,
    rules: &[ProjectionRule],
    known_taxonomies: &std::collections::BTreeSet<Id>,
    resolve: impl Fn(&Id) -> bool,
) -> Result<Projection, EnrichError> {
    if let Some(rule) = rules
        .iter()
        .find(|rule| !known_taxonomies.contains(&rule.taxonomy))
    {
        return Err(EnrichError::new(
            PROJECTION_RULE_UNKNOWN_TAXONOMY,
            format!("rule {} names unknown taxonomy {}", rule.id, rule.taxonomy),
        ));
    }
    let mut projection = Projection::default();
    let mut seen = std::collections::BTreeSet::new();
    for rule in rules {
        let Some(value) = node.properties.get(rule.property.as_str()) else {
            continue;
        };
        match Id::new(value.clone()) {
            Ok(target) if resolve(&target) => {
                let id = edge_id(&node.id, &rule.relation, &target);
                if seen.insert(id.clone()) {
                    projection.edges.push(ProjectedEdge {
                        id,
                        source: node.id.clone(),
                        relation: rule.relation.clone(),
                        target,
                    });
                }
            }
            _ => projection.unresolved += 1,
        }
    }
    Ok(projection)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> Id {
        Id::new(value).unwrap()
    }

    fn card() -> CapabilityCard {
        CapabilityCard {
            id: id("card:summarize::main"),
            subject: id("src/lib.rs::main"),
            summary: "main wires the parser to the store".into(),
            model: id("eg:head/text-7b"),
            prompt_digest: "ab".repeat(32),
            source_fact_ids: vec![id("fact:1"), id("fact:2")],
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R614.1
    #[test]
    fn semantic_types_round_trip() {
        let card = card();
        let candidate = EnhancementCandidate {
            id: id("cand:1"),
            concept: id("concept:store-indexing"),
            value: 42,
        };
        let draft = SpecDraft {
            id: id("draft:1"),
            title: "Store indexing".into(),
            body: "Index the store by fact id.".into(),
            source_concepts: vec![id("concept:store-indexing")],
        };
        let assignment = TopicAssignment {
            id: id("assign:1"),
            document: id("doc:1"),
            topic: id("topic:storage"),
            confidence: Confidence::new(8_500).unwrap(),
        };
        let rule = ProjectionRule {
            id: id("rule:1"),
            taxonomy: id("taxonomy:code"),
            property: id("path"),
            relation: id("eg:edge/contains"),
        };
        macro_rules! round_trip {
            ($value:expr, $ty:ty) => {{
                let json = serde_json::to_string(&$value).unwrap();
                assert_eq!(serde_json::from_str::<$ty>(&json).unwrap(), $value);
            }};
        }
        round_trip!(card, CapabilityCard);
        round_trip!(candidate, EnhancementCandidate);
        round_trip!(draft, SpecDraft);
        round_trip!(assignment, TopicAssignment);
        round_trip!(rule, ProjectionRule);
        assert_eq!(
            serde_json::to_string(&assignment).unwrap(),
            r#"{"id":"assign:1","document":"doc:1","topic":"topic:storage","confidence":8500}"#
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R614.1
    #[test]
    fn semantic_types_reject_invalid_identifiers() {
        for bad in ["", "has space", &"x".repeat(crate::MAX_ID_BYTES + 1)] {
            assert_eq!(Id::new(bad).unwrap_err().code, crate::INVALID_IDENTIFIER);
        }
        let bad_card = serde_json::to_value(card()).unwrap();
        let mut card = bad_card;
        card["id"] = serde_json::json!("card 1");
        assert!(serde_json::from_value::<CapabilityCard>(card).is_err());
        assert!(serde_json::from_str::<EnhancementCandidate>(
            r#"{"id":"a","concept":"b","value":1,"extra":2}"#
        )
        .is_err());
        assert!(serde_json::from_str::<TopicAssignment>(
            r#"{"id":"a","document":"b","topic":"c","confidence":10001}"#
        )
        .is_err());
        assert_eq!(
            Confidence::new(10_001).unwrap_err().code,
            INVALID_CONFIDENCE
        );
        assert_eq!(Confidence::new(10_000).unwrap().basis_points(), 10_000);
    }
}
