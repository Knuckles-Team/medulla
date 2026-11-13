//! Bounded, fixed-dimension candidates and their Pareto front.

use serde::{Deserialize, Serialize};

mod score_array {
    use serde::de::{Error, IgnoredAny, SeqAccess, Visitor};
    use serde::ser::SerializeSeq;
    use serde::{Deserializer, Serializer};
    use std::fmt;

    pub fn serialize<S, const K: usize>(scores: &[f32; K], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(K))?;
        for score in scores {
            sequence.serialize_element(score)?;
        }
        sequence.end()
    }

    pub fn deserialize<'de, D, const K: usize>(deserializer: D) -> Result<[f32; K], D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ScoreArrayVisitor<const K: usize>;

        impl<'de, const K: usize> Visitor<'de> for ScoreArrayVisitor<K> {
            type Value = [f32; K];

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "a score array containing exactly {K} values")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut scores = [0.0; K];
                for (index, score) in scores.iter_mut().enumerate() {
                    *score = sequence
                        .next_element()?
                        .ok_or_else(|| A::Error::invalid_length(index, &self))?;
                }
                if sequence.next_element::<IgnoredAny>()?.is_some() {
                    return Err(A::Error::invalid_length(K + 1, &self));
                }
                Ok(scores)
            }
        }

        deserializer.deserialize_seq(ScoreArrayVisitor::<K>)
    }
}

/// A policy evaluated on one fixed-size set of evaluation metrics.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyCandidate<const K: usize> {
    pub id: String,
    pub parent: Option<String>,
    /// Lowercase SHA-256 digest of the candidate's policy genes.
    pub genes_digest: String,
    /// Scores in `[0, 1]`; the array fixes the evaluation-set dimension at compile time.
    #[serde(with = "score_array")]
    pub scores: [f32; K],
    /// Measured cost in integer micro-USD.
    pub cost_micro_usd: u64,
}

impl<'de, const K: usize> Deserialize<'de> for PolicyCandidate<K> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct CandidateData<const K: usize> {
            id: String,
            parent: Option<String>,
            genes_digest: String,
            #[serde(with = "score_array")]
            scores: [f32; K],
            cost_micro_usd: u64,
        }

        let data = CandidateData::<K>::deserialize(deserializer)?;
        let candidate = Self {
            id: data.id,
            parent: data.parent,
            genes_digest: data.genes_digest,
            scores: data.scores,
            cost_micro_usd: data.cost_micro_usd,
        };
        candidate.validate().map_err(serde::de::Error::custom)?;
        Ok(candidate)
    }
}

impl<const K: usize> PolicyCandidate<K> {
    fn validate(&self) -> Result<(), FrontError> {
        if self.id.trim().is_empty() {
            return Err(FrontError::EmptyId);
        }
        if let Some(parent) = &self.parent {
            if parent.trim().is_empty() {
                return Err(FrontError::EmptyParent);
            }
        }
        if self.genes_digest.len() != 64
            || !self
                .genes_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(FrontError::InvalidGenesDigest);
        }
        if self
            .scores
            .iter()
            .any(|score| !score.is_finite() || !(0.0..=1.0).contains(score))
        {
            return Err(FrontError::ScoreOutOfBounds);
        }
        Ok(())
    }

    fn dominates(&self, other: &Self) -> bool {
        let no_worse = self.cost_micro_usd <= other.cost_micro_usd
            && self
                .scores
                .iter()
                .zip(other.scores.iter())
                .all(|(left, right)| left >= right);
        let strictly_better = self.cost_micro_usd < other.cost_micro_usd
            || self
                .scores
                .iter()
                .zip(other.scores.iter())
                .any(|(left, right)| left > right);
        no_worse && strictly_better
    }
}

/// A typed refusal for malformed candidate identity or score data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrontError {
    EmptyId,
    EmptyParent,
    InvalidGenesDigest,
    ScoreOutOfBounds,
}

impl std::fmt::Display for FrontError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for FrontError {}

/// Non-dominated candidates sharing one compile-time score dimension `K`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParetoFront<const K: usize> {
    candidates: Vec<PolicyCandidate<K>>,
}

impl<'de, const K: usize> Deserialize<'de> for ParetoFront<K> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FrontData<const K: usize> {
            candidates: Vec<PolicyCandidate<K>>,
        }

        let data = FrontData::<K>::deserialize(deserializer)?;
        let expected_len = data.candidates.len();
        let mut front = Self::default();
        for candidate in data.candidates {
            if !front.insert(candidate).map_err(serde::de::Error::custom)? {
                return Err(serde::de::Error::custom(
                    "serialized Pareto front contains a dominated candidate",
                ));
            }
        }
        if front.candidates.len() != expected_len {
            return Err(serde::de::Error::custom(
                "serialized Pareto front contains a candidate dominated by a later entry",
            ));
        }
        Ok(front)
    }
}

impl<const K: usize> Default for ParetoFront<K> {
    fn default() -> Self {
        Self {
            candidates: Vec::new(),
        }
    }
}

impl<const K: usize> ParetoFront<K> {
    pub fn candidates(&self) -> &[PolicyCandidate<K>] {
        &self.candidates
    }

    /// Crowding distance of every candidate, in candidate order. Every score
    /// and the cost count as one objective each, normalised by the front's
    /// range of that objective; boundary candidates are infinitely far.
    pub fn crowding_distances(&self) -> Vec<f64> {
        let count = self.candidates.len();
        let mut distance = vec![0.0f64; count];
        let objective = |candidate: &PolicyCandidate<K>, index: usize| -> f64 {
            if index < K {
                f64::from(candidate.scores[index])
            } else {
                candidate.cost_micro_usd as f64
            }
        };
        for index in 0..=K {
            let mut order: Vec<usize> = (0..count).collect();
            order.sort_by(|&left, &right| {
                objective(&self.candidates[left], index)
                    .total_cmp(&objective(&self.candidates[right], index))
                    .then_with(|| self.candidates[left].id.cmp(&self.candidates[right].id))
            });
            let (Some(&first), Some(&last)) = (order.first(), order.last()) else {
                continue;
            };
            distance[first] = f64::INFINITY;
            distance[last] = f64::INFINITY;
            let low = objective(&self.candidates[first], index);
            let range = objective(&self.candidates[last], index) - low;
            if range <= 0.0 {
                continue;
            }
            for window in order.windows(3) {
                let gap = objective(&self.candidates[window[2]], index)
                    - objective(&self.candidates[window[0]], index);
                distance[window[1]] += gap / range;
            }
        }
        distance
    }

    /// Keep at most `max_size` candidates: the smallest crowding distance
    /// leaves first, ties dropping the greater id.
    pub fn bound(&mut self, max_size: usize) {
        while self.candidates.len() > max_size.max(1) {
            let distance = self.crowding_distances();
            let Some(worst) = (0..self.candidates.len()).min_by(|&left, &right| {
                distance[left]
                    .total_cmp(&distance[right])
                    .then_with(|| self.candidates[right].id.cmp(&self.candidates[left].id))
            }) else {
                return;
            };
            self.candidates.remove(worst);
        }
    }

    /// The next parent by binary tournament: two draws of a SplitMix64
    /// stream seeded by `seed`; the larger crowding distance wins and a tie
    /// picks the lower id. `None` on an empty front.
    pub fn next_parent(&self, seed: u64) -> Option<&PolicyCandidate<K>> {
        let count = self.candidates.len() as u64;
        if count == 0 {
            return None;
        }
        let distance = self.crowding_distances();
        let mut state = seed;
        let first = (splitmix64(&mut state) % count) as usize;
        let second = (splitmix64(&mut state) % count) as usize;
        let winner = match distance[first].total_cmp(&distance[second]) {
            std::cmp::Ordering::Greater => first,
            std::cmp::Ordering::Less => second,
            std::cmp::Ordering::Equal => {
                if self.candidates[first].id <= self.candidates[second].id {
                    first
                } else {
                    second
                }
            }
        };
        Some(&self.candidates[winner])
    }

    /// Candidates ranked by `scores[0]` per `max(cost_micro_usd, 1)`,
    /// descending; ties by id.
    pub fn scoreboard(&self) -> Vec<&PolicyCandidate<K>> {
        let ratio = |candidate: &PolicyCandidate<K>| {
            if K == 0 {
                0.0
            } else {
                f64::from(candidate.scores[0]) / candidate.cost_micro_usd.max(1) as f64
            }
        };
        let mut ranked: Vec<&PolicyCandidate<K>> = self.candidates.iter().collect();
        ranked.sort_by(|left, right| {
            ratio(right)
                .total_cmp(&ratio(left))
                .then_with(|| left.id.cmp(&right.id))
        });
        ranked
    }

    /// SHA-256 over the canonical JSON of the candidates sorted by id, as 64
    /// lowercase hex characters.
    pub fn digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut sorted: Vec<&PolicyCandidate<K>> = self.candidates.iter().collect();
        sorted.sort_by(|left, right| left.id.cmp(&right.id));
        let canonical = serde_json::to_vec(&sorted).unwrap_or_default();
        hex::encode(Sha256::digest(canonical))
    }

    /// Insert a valid candidate unless an existing candidate dominates it.
    /// Returns `true` when the front changes.
    pub fn insert(&mut self, candidate: PolicyCandidate<K>) -> Result<bool, FrontError> {
        candidate.validate()?;
        if self
            .candidates
            .iter()
            .any(|existing| existing.dominates(&candidate))
        {
            return Ok(false);
        }
        self.candidates
            .retain(|existing| !candidate.dominates(existing));
        self.candidates.push(candidate);
        Ok(true)
    }
}

/// One SplitMix64 step.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
