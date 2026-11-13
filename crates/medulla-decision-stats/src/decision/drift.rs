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
use std::collections::VecDeque;

use medulla_decision_types::decision::statistical::retrieval_drift::{
    DriftKind, RegionRef, MAX_REFRESH_REGIONS,
};
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
    KnobSpec {
        name: "drift.quality_decay_pct",
        unit: "percent",
        default: KnobValue::Integer(10),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(90),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "drift.quality_window",
        unit: "outcomes",
        default: KnobValue::Integer(256),
        min: KnobValue::Integer(8),
        max: KnobValue::Integer(65_536),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "discover.max_version_lag",
        unit: "versions",
        default: KnobValue::Integer(1000),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(1_000_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "drift.reference_vectors",
        unit: "vectors",
        default: KnobValue::Integer(64),
        min: KnobValue::Integer(2),
        max: KnobValue::Integer(1_000_000),
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
    /// Percent the rolling MRR may fall below the activation baseline.
    pub quality_decay_pct: u64,
    /// Rolling window of cited outcomes the MRR is taken over.
    pub quality_window: usize,
    /// Versions the index may trail the graph before it is stale.
    pub max_version_lag: u64,
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
        let integer = |index: usize, fallback: i64| match policy.knob(&KNOBS[index]) {
            KnobValue::Integer(value) => value.max(0),
            _ => fallback,
        };
        Self {
            shift,
            vocabulary_points,
            quality_decay_pct: integer(2, 10) as u64,
            quality_window: integer(3, 256) as usize,
            max_version_lag: integer(4, 1000) as u64,
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
    quality: u8,
    staleness: u8,
}

/// One more evaluation: the streak grows when `exceeded`, else resets.
fn step(streak: &mut u8, exceeded: bool) {
    *streak = if exceeded {
        streak.saturating_add(1)
    } else {
        0
    };
}

/// A compressed bitmap of the revisions written since the generation
/// watermark: sorted, disjoint, non-adjacent inclusive runs, so memory is
/// proportional to the number of contiguous changed ranges.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangedRevisions {
    runs: Vec<(u64, u64)>,
}

impl ChangedRevisions {
    /// Mark `revision` changed, merging with neighbouring runs.
    pub fn mark(&mut self, revision: u64) {
        let at = self
            .runs
            .partition_point(|&(_, to)| to.saturating_add(1) < revision);
        match self.runs.get_mut(at) {
            Some((from, to)) if *from <= revision.saturating_add(1) => {
                *from = (*from).min(revision);
                *to = (*to).max(revision);
                let merged_to = *to;
                while self
                    .runs
                    .get(at + 1)
                    .is_some_and(|&(next, _)| next <= merged_to.saturating_add(1))
                {
                    let (_, next_to) = self.runs.remove(at + 1);
                    self.runs[at].1 = self.runs[at].1.max(next_to);
                }
            }
            _ => self.runs.insert(at, (revision, revision)),
        }
    }

    /// Whether `revision` is marked.
    pub fn contains(&self, revision: u64) -> bool {
        let at = self.runs.partition_point(|&(_, to)| to < revision);
        self.runs.get(at).is_some_and(|&(from, _)| from <= revision)
    }

    /// The changed runs, ascending.
    pub fn runs(&self) -> &[(u64, u64)] {
        &self.runs
    }

    /// The runs coalesced to at most `cap` by joining the smallest gaps first.
    fn coalesced(&self, cap: usize) -> Vec<(u64, u64)> {
        let mut runs = self.runs.clone();
        if runs.len() <= cap.max(1) {
            return runs;
        }
        let excess = runs.len() - cap.max(1);
        let mut gaps: Vec<(u64, usize)> = runs
            .windows(2)
            .enumerate()
            .map(|(i, w)| (w[1].0 - w[0].1, i))
            .collect();
        gaps.sort_unstable();
        let mut join: Vec<usize> = gaps.into_iter().take(excess).map(|(_, i)| i).collect();
        join.sort_unstable_by(|a, b| b.cmp(a));
        for i in join {
            let (_, next_to) = runs.remove(i + 1);
            runs[i].1 = next_to;
        }
        runs
    }
}

/// Mean reciprocal rank of the last `window` cited outcomes.
#[derive(Debug, Clone, Default, PartialEq)]
struct RollingMrr {
    ranks: VecDeque<f64>,
    sum: f64,
}

impl RollingMrr {
    fn push(&mut self, reciprocal_rank: f64, window: usize) {
        self.ranks.push_back(reciprocal_rank);
        self.sum += reciprocal_rank;
        while self.ranks.len() > window.max(1) {
            if let Some(old) = self.ranks.pop_front() {
                self.sum -= old;
            }
        }
    }

    fn mean(&self) -> Option<f64> {
        (!self.ranks.is_empty()).then(|| self.sum / self.ranks.len() as f64)
    }
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
    /// Activation MRR of cited outcomes; quality decay needs one.
    baseline_mrr: Option<f64>,
    rolling: RollingMrr,
    graph_version: u64,
    embedded_through: u64,
    changed: ChangedRevisions,
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
            baseline_mrr: None,
            rolling: RollingMrr::default(),
            graph_version: 0,
            embedded_through: 0,
            changed: ChangedRevisions::default(),
        }
    }

    /// Set the activation MRR of cited outcomes the rolling MRR is held to.
    pub fn with_quality_baseline(mut self, mrr: f64) -> Self {
        self.baseline_mrr = Some(mrr);
        self
    }

    /// Record one cited outcome as its reciprocal rank (0 when not returned).
    pub fn observe_outcome(&mut self, reciprocal_rank: f64, thresholds: &DriftThresholds) {
        self.rolling
            .push(reciprocal_rank, thresholds.quality_window);
    }

    /// Record the graph version and the version the index is embedded through.
    pub fn observe_versions(&mut self, graph_version: u64, embedded_through: u64) {
        self.graph_version = graph_version;
        self.embedded_through = embedded_through;
    }

    /// Record a node change at `revision`, after the generation watermark.
    pub fn record_change(&mut self, revision: u64) {
        self.changed.mark(revision);
    }

    /// The revisions changed since the generation watermark.
    pub fn changed(&self) -> &ChangedRevisions {
        &self.changed
    }

    /// Percent the rolling MRR fell below the activation baseline, if both exist.
    pub fn quality_decay_pct(&self) -> Option<f64> {
        let baseline = self.baseline_mrr.filter(|b| *b > 0.0)?;
        let rolling = self.rolling.mean()?;
        Some((baseline - rolling) / baseline * 100.0)
    }

    /// Versions the index trails the graph by (the `stale_index` test).
    pub fn version_lag(&self) -> u64 {
        self.graph_version.saturating_sub(self.embedded_through)
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
        let decayed = self
            .quality_decay_pct()
            .is_some_and(|pct| pct > thresholds.quality_decay_pct as f64);
        step(&mut self.streaks.quality, decayed);
        let stale = self.version_lag() > thresholds.max_version_lag;
        step(&mut self.streaks.staleness, stale);
        self.firing()
    }

    /// The kinds whose last two evaluations both exceeded their threshold.
    pub fn firing(&self) -> Vec<DriftKind> {
        let fired = |streak: u8| streak >= FIRING_EVALUATIONS;
        [
            (DriftKind::DistributionShift, self.streaks.shift),
            (DriftKind::VocabularyGrowth, self.streaks.vocabulary),
            (DriftKind::QualityDecay, self.streaks.quality),
            (DriftKind::Staleness, self.streaks.staleness),
        ]
        .into_iter()
        .filter(|&(_, streak)| fired(streak))
        .map(|(kind, _)| kind)
        .collect()
    }

    /// Heap bytes of the running vectors, excluding the filter.
    pub fn memory_bytes(&self) -> usize {
        self.reference.memory_bytes()
            + self.current.memory_bytes()
            + self.rolling.ranks.capacity() * std::mem::size_of::<f64>()
    }
}

/// The regions a refresh covers: for every shard with a firing signal, one
/// region per changed revision run of that shard (at most
/// [`MAX_REFRESH_REGIONS`] in all, smallest gaps joined first). A shard with
/// no firing signal contributes nothing, however much changed in it.
pub fn select_refresh_regions(shards: &[(u32, &str, &DriftMonitor)]) -> Vec<RegionRef> {
    let firing: Vec<_> = shards
        .iter()
        .filter(|(_, _, monitor)| !monitor.firing().is_empty())
        .collect();
    let per_shard_cap = MAX_REFRESH_REGIONS / firing.len().max(1);
    firing
        .into_iter()
        .flat_map(|&(shard_id, node_type, monitor)| {
            monitor.changed.coalesced(per_shard_cap).into_iter().map(
                move |(revision_from, revision_to)| RegionRef {
                    shard_id,
                    node_type: node_type.to_string(),
                    revision_from,
                    revision_to,
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIM: usize = 4;

    fn thresholds() -> DriftThresholds {
        DriftThresholds {
            shift: 0.2,
            vocabulary_points: 5,
            quality_decay_pct: 10,
            quality_window: 256,
            max_version_lag: 1000,
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

    // spec: MEDULLA-DECISION-ENGINE-R174.8.2.1
    #[test]
    fn quality_decay_and_staleness_fire() {
        let t = thresholds();
        let mut monitor =
            DriftMonitor::activate(reference(), VocabBloom::default()).with_quality_baseline(0.8);
        // Rolling MRR 0.74 is a 7.5% decay: below the 10% threshold.
        for _ in 0..10 {
            monitor.observe_outcome(0.74, &t);
        }
        monitor.observe_versions(1500, 1000);
        assert!(monitor.evaluate(&t).is_empty());
        // The window now holds only 0.5: a 37.5% decay, firing on the 2nd evaluation.
        for _ in 0..256 {
            monitor.observe_outcome(0.5, &t);
        }
        assert!((monitor.quality_decay_pct().unwrap() - 37.5).abs() < 1e-9);
        assert!(monitor.evaluate(&t).is_empty());
        assert_eq!(monitor.evaluate(&t), vec![DriftKind::QualityDecay]);
        // A lag of exactly the knob does not fire; one more version does, twice.
        monitor.observe_versions(2000, 1000);
        assert_eq!(monitor.version_lag(), 1000);
        monitor.evaluate(&t);
        assert!(!monitor.firing().contains(&DriftKind::Staleness));
        monitor.observe_versions(2001, 1000);
        monitor.evaluate(&t);
        assert_eq!(
            monitor.evaluate(&t),
            vec![DriftKind::QualityDecay, DriftKind::Staleness]
        );
        // Recovery resets the streak.
        monitor.observe_versions(2001, 2001);
        assert_eq!(monitor.evaluate(&t), vec![DriftKind::QualityDecay]);
        // No baseline or no outcomes: no quality signal.
        let none = DriftMonitor::activate(reference(), VocabBloom::default());
        assert_eq!(none.quality_decay_pct(), None);
    }

    // spec: MEDULLA-DECISION-ENGINE-R174.8.2.1
    #[test]
    fn plan_selects_only_changed_regions() {
        let t = thresholds();
        let fire = |changes: &[u64], unseen: bool| {
            let mut monitor = DriftMonitor::activate(reference(), bloom(&["known"]));
            for seed in 0..40 {
                let token = if unseen { "novel" } else { "known" };
                monitor.observe(&vector(seed, 0.0), &[token]).unwrap();
            }
            changes.iter().for_each(|&r| monitor.record_change(r));
            monitor.evaluate(&t);
            monitor.evaluate(&t);
            monitor
        };
        let hot = fire(&[5, 6, 7, 20, 4, 21], true);
        let quiet = fire(&[1, 2, 3], false);
        assert_eq!(hot.changed().runs(), &[(4, 7), (20, 21)]);
        assert!(hot.changed().contains(6) && !hot.changed().contains(8));
        let regions = select_refresh_regions(&[(0, "Document", &quiet), (1, "Document", &hot)]);
        let spans: Vec<_> = regions
            .iter()
            .map(|r| (r.shard_id, r.revision_from, r.revision_to))
            .collect();
        assert_eq!(spans, vec![(1, 4, 7), (1, 20, 21)]);
        assert!(select_refresh_regions(&[(0, "Document", &quiet)]).is_empty());
        // A shard with many runs coalesces to the cap, joining the smallest gaps.
        let mut many = DriftMonitor::activate(reference(), bloom(&["known"]));
        (0..10_000_u64).for_each(|r| many.record_change(r * 3));
        assert_eq!(many.changed().runs().len(), 10_000);
        for _ in 0..2 {
            many.observe_versions(5000, 0);
            many.evaluate(&t);
        }
        let capped = select_refresh_regions(&[(9, "Document", &many)]);
        assert_eq!(capped.len(), MAX_REFRESH_REGIONS);
        assert_eq!(capped.first().unwrap().revision_from, 0);
        assert_eq!(capped.last().unwrap().revision_to, 29_997);
        // Adjacent marks merge and out-of-order marks keep runs sorted.
        let mut bitmap = ChangedRevisions::default();
        for r in [9, 1, 3, 2, 8, 10] {
            bitmap.mark(r);
        }
        assert_eq!(bitmap.runs(), &[(1, 3), (8, 10)]);
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
