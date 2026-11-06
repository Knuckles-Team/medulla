//! Shard-level shift and vocabulary monitors (MEDULLA-DECISION-ENGINE-R174.8.2).
//!
//! A [`DriftMonitor`] keeps, per shard, a running mean and diagonal variance of
//! the vectors written since the generation was activated (Welford updates)
//! and counts the written tokens a Bloom filter built at activation has never
//! seen. Shift is the symmetric divergence of two diagonal Gaussians (the
//! activation reference and the current window), averaged over dimensions;
//! vocabulary growth is the share of written tokens missing from the filter,
//! in percentage points. A signal fires once its value exceeds its threshold
//! on two consecutive evaluations. Only `+ - * /` touch floats, so the crate's
//! ban on transcendental functions holds, and nothing here reads a clock.

use medulla_decision_types::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use medulla_decision_types::decision::statistical::retrieval_drift::DriftKind;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_decision_types::decision::DecisionPolicy;

use super::refusal::{Refusal, RefusalResult};

/// Bytes of the activation vocabulary filter.
pub const BLOOM_BYTES: usize = 8 * 1024;
/// Probes per token.
const BLOOM_PROBES: u64 = 4;
/// Consecutive exceeding evaluations that fire a signal.
pub const FIRING_EVALUATIONS: u8 = 2;
/// A variance floor: a constant dimension never divides by zero.
const VARIANCE_FLOOR: f64 = 1e-9;

/// The knobs of the monitors, one entry per line.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "drift.shift_threshold",
        unit: "divergence",
        default: KnobValue::DecimalMicros(200_000),
        min: KnobValue::DecimalMicros(10_000),
        max: KnobValue::DecimalMicros(5_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "drift.vocabulary_growth_points",
        unit: "points",
        default: KnobValue::Integer(5),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(50),
        safety: KnobSafety::Tunable,
    },
];

/// The thresholds one evaluation compares against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriftThresholds {
    /// Symmetric divergence above which the distribution shifted.
    pub shift: f64,
    /// Percentage points of unseen tokens above which the vocabulary grew.
    pub vocabulary_points: u64,
}

impl DriftThresholds {
    /// The thresholds a pinned policy carries, or the knob defaults.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let shift = match policy.knob(&KNOBS[0]) {
            KnobValue::DecimalMicros(micros) => micros as f64 / 1_000_000.0,
            _ => 0.2,
        };
        let vocabulary_points = match policy.knob(&KNOBS[1]) {
            KnobValue::Integer(points) => points.max(0) as u64,
            _ => 5,
        };
        Self {
            shift,
            vocabulary_points,
        }
    }
}

/// Running mean and diagonal variance of written vectors.
#[derive(Debug, Clone, PartialEq)]
pub struct Moments {
    count: u64,
    mean: Vec<f64>,
    m2: Vec<f64>,
}

impl Moments {
    /// Empty moments over `dim` dimensions.
    pub fn new(dim: usize) -> Self {
        Self {
            count: 0,
            mean: vec![0.0; dim],
            m2: vec![0.0; dim],
        }
    }

    /// Fold one vector in with the incremental (Welford) update.
    pub fn push(&mut self, vector: &[f64]) -> RefusalResult<()> {
        if vector.len() != self.mean.len() {
            return Err(Refusal::new(
                StatisticalErrorCode::ParameterInvalid,
                format!(
                    "written vector has {} dimensions, the monitor has {}",
                    vector.len(),
                    self.mean.len()
                ),
            ));
        }
        self.count += 1;
        let n = self.count as f64;
        for ((mean, m2), &x) in self.mean.iter_mut().zip(self.m2.iter_mut()).zip(vector) {
            let delta = x - *mean;
            *mean += delta / n;
            *m2 += delta * (x - *mean);
        }
        Ok(())
    }

    /// Vectors folded in so far.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Heap bytes of the two running vectors.
    pub fn memory_bytes(&self) -> usize {
        (self.mean.len() + self.m2.len()) * std::mem::size_of::<f64>()
    }

    fn variance(&self, i: usize) -> f64 {
        (self.m2[i] / self.count as f64).max(VARIANCE_FLOOR)
    }
}

/// The mean over dimensions of the symmetric divergence of two diagonal
/// Gaussians; `0.0` while either side has fewer than two vectors.
pub fn symmetric_divergence(a: &Moments, b: &Moments) -> f64 {
    if a.count < 2 || b.count < 2 || a.mean.len() != b.mean.len() || a.mean.is_empty() {
        return 0.0;
    }
    let total: f64 = (0..a.mean.len())
        .map(|i| {
            let (va, vb) = (a.variance(i), b.variance(i));
            let gap = a.mean[i] - b.mean[i];
            0.5 * (va / vb + vb / va - 2.0 + gap * gap * (1.0 / va + 1.0 / vb))
        })
        .sum();
    total / a.mean.len() as f64
}

/// The tokens known at generation activation, in [`BLOOM_BYTES`] bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VocabBloom {
    bits: Vec<u8>,
}

impl Default for VocabBloom {
    fn default() -> Self {
        Self {
            bits: vec![0; BLOOM_BYTES],
        }
    }
}

impl VocabBloom {
    /// The bit positions of `token`: FNV-1a halves combined by double hashing.
    fn probes(token: &str) -> impl Iterator<Item = usize> {
        let hash = token.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, byte| {
            (h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
        let (h1, h2) = (hash & 0xffff_ffff, (hash >> 32) | 1);
        (0..BLOOM_PROBES)
            .map(move |i| (h1.wrapping_add(i * h2) % (BLOOM_BYTES as u64 * 8)) as usize)
    }

    /// Add `token` to the filter.
    pub fn insert(&mut self, token: &str) {
        for bit in Self::probes(token) {
            self.bits[bit / 8] |= 1 << (bit % 8);
        }
    }

    /// Whether `token` may have been inserted; `false` is exact.
    pub fn contains(&self, token: &str) -> bool {
        Self::probes(token).all(|bit| self.bits[bit / 8] & (1 << (bit % 8)) != 0)
    }
}

/// Consecutive exceeding evaluations of each signal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Streaks {
    shift: u8,
    vocabulary: u8,
}

/// One more evaluation: the streak grows when `exceeded`, else resets.
fn step(streak: &mut u8, exceeded: bool) {
    *streak = if exceeded {
        streak.saturating_add(1)
    } else {
        0
    };
}

/// The per-shard monitor, built when a generation is activated.
#[derive(Debug, Clone, PartialEq)]
pub struct DriftMonitor {
    reference: Moments,
    vocabulary: VocabBloom,
    current: Moments,
    tokens: u64,
    unseen: u64,
    streaks: Streaks,
}

impl DriftMonitor {
    /// A monitor against the activation `reference` and `vocabulary`.
    pub fn activate(reference: Moments, vocabulary: VocabBloom) -> Self {
        let current = Moments::new(reference.mean.len());
        Self {
            reference,
            vocabulary,
            current,
            tokens: 0,
            unseen: 0,
            streaks: Streaks::default(),
        }
    }

    /// Record one write: its vector and the tokens it carries.
    pub fn observe(&mut self, vector: &[f64], tokens: &[&str]) -> RefusalResult<()> {
        self.current.push(vector)?;
        self.tokens += tokens.len() as u64;
        self.unseen += tokens
            .iter()
            .filter(|t| !self.vocabulary.contains(t))
            .count() as u64;
        Ok(())
    }

    /// The current shift against the activation reference.
    pub fn shift(&self) -> f64 {
        symmetric_divergence(&self.reference, &self.current)
    }

    /// Written tokens the activation filter never saw, in percentage points.
    pub fn vocabulary_points(&self) -> u64 {
        (self.unseen * 100).checked_div(self.tokens).unwrap_or(0)
    }

    /// Evaluate both signals once and return the kinds now firing.
    pub fn evaluate(&mut self, thresholds: &DriftThresholds) -> Vec<DriftKind> {
        let shifted = self.shift() > thresholds.shift;
        step(&mut self.streaks.shift, shifted);
        let grown = self.vocabulary_points() > thresholds.vocabulary_points;
        step(&mut self.streaks.vocabulary, grown);
        self.firing()
    }

    /// The kinds whose last two evaluations both exceeded their threshold.
    pub fn firing(&self) -> Vec<DriftKind> {
        let fired = |streak: u8| streak >= FIRING_EVALUATIONS;
        [
            (DriftKind::DistributionShift, self.streaks.shift),
            (DriftKind::VocabularyGrowth, self.streaks.vocabulary),
        ]
        .into_iter()
        .filter(|&(_, streak)| fired(streak))
        .map(|(kind, _)| kind)
        .collect()
    }

    /// Heap bytes of the running vectors, excluding the filter.
    pub fn memory_bytes(&self) -> usize {
        self.reference.memory_bytes() + self.current.memory_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIM: usize = 4;

    fn thresholds() -> DriftThresholds {
        DriftThresholds {
            shift: 0.2,
            vocabulary_points: 5,
        }
    }

    /// A deterministic vector around `centre`.
    fn vector(seed: usize, centre: f64) -> Vec<f64> {
        (0..DIM)
            .map(|d| centre + ((seed * 7 + d * 3) % 11) as f64 / 10.0)
            .collect()
    }

    fn reference() -> Moments {
        let mut moments = Moments::new(DIM);
        for seed in 0..40 {
            moments.push(&vector(seed, 0.0)).unwrap();
        }
        moments
    }

    fn bloom(words: &[&str]) -> VocabBloom {
        let mut bloom = VocabBloom::default();
        words.iter().for_each(|w| bloom.insert(w));
        bloom
    }

    fn feed(monitor: &mut DriftMonitor, centre: f64, tokens: &[&str]) {
        for seed in 0..40 {
            monitor.observe(&vector(seed, centre), tokens).unwrap();
        }
    }

    /// Verifies MEDULLA-DECISION-ENGINE-R174.8.2: Behavior: shift and vocabulary monitors.
    // spec: MEDULLA-DECISION-ENGINE-R174.8.2
    #[test]
    fn shift_signal_fires_after_two_evaluations() {
        let mut stable = DriftMonitor::activate(reference(), bloom(&["a"]));
        feed(&mut stable, 0.0, &["a"]);
        assert!(stable.shift() < 0.2);
        assert!(stable.evaluate(&thresholds()).is_empty());

        let mut shifted = DriftMonitor::activate(reference(), bloom(&["a"]));
        feed(&mut shifted, 3.0, &["a"]);
        assert!(shifted.shift() > 0.2);
        assert!(shifted.evaluate(&thresholds()).is_empty(), "one is not two");
        assert_eq!(
            shifted.evaluate(&thresholds()),
            vec![DriftKind::DistributionShift]
        );
        // Returning inside the threshold resets the hysteresis.
        let mut back = DriftMonitor::activate(reference(), bloom(&["a"]));
        feed(&mut back, 3.0, &["a"]);
        back.evaluate(&thresholds());
        back.current = Moments::new(DIM);
        feed(&mut back, 0.0, &["a"]);
        assert!(back.evaluate(&thresholds()).is_empty());
        assert!(back.firing().is_empty());
    }

    /// Verifies MEDULLA-DECISION-ENGINE-R174.8.2: Behavior: shift and vocabulary monitors.
    // spec: MEDULLA-DECISION-ENGINE-R174.8.2
    #[test]
    fn vocabulary_growth_signal_fires() {
        let mut monitor = DriftMonitor::activate(reference(), bloom(&["alpha", "beta"]));
        feed(&mut monitor, 0.0, &["alpha", "beta"]);
        assert_eq!(monitor.vocabulary_points(), 0);
        assert!(monitor.evaluate(&thresholds()).is_empty());
        // 80 old and 40 new tokens: 33 points, above the 5-point threshold.
        feed(&mut monitor, 0.0, &["gamma-new-term"]);
        assert!(monitor.vocabulary_points() >= 30);
        assert!(monitor.evaluate(&thresholds()).is_empty());
        assert_eq!(
            monitor.evaluate(&thresholds()),
            vec![DriftKind::VocabularyGrowth]
        );
        assert!(bloom(&["x"]).contains("x") && !VocabBloom::default().contains("x"));
    }

    #[test]
    fn memory_stays_inside_the_per_shard_budget() {
        let monitor = DriftMonitor::activate(Moments::new(1024), VocabBloom::default());
        assert!(monitor.memory_bytes() <= 64 * 1024);
        assert_eq!(VocabBloom::default().bits.len(), BLOOM_BYTES);
        let mut moments = Moments::new(2);
        assert!(moments.push(&[1.0]).is_err());
    }

    #[test]
    fn knob_defaults_lie_inside_bounds() {
        for spec in KNOBS {
            assert!(spec.admits(&spec.default), "{}", spec.name);
        }
        let policy = DecisionPolicy::engine_default();
        assert_eq!(DriftThresholds::from_policy(&policy), thresholds());
    }
}
