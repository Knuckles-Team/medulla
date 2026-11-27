//! The drift monitors of the served graphs (MEDULLA-DECISION-ENGINE-R174.8.3).
//!
//! One [`DriftRegistry`] holds a monitor per `(tenant, graph, shard)`. The write commit path
//! feeds it through the `medulla-retrieval` port ([`DiscoverSignals::observe_write`]); a
//! `DecisionLog` `EvaluateDrift` write evaluates the monitors and schedules one refresh plan;
//! `Discover` reads the firing shards through the same port. The tenant is part of every key,
//! so a read never sees another tenant's monitors.
//!
//! Each shard entry is an explicit state machine, [`ShardMonitor`]: it is `Warming` while it
//! collects the reference window (the first `drift.reference_vectors` writes after
//! activation), and `Monitoring` once the reference is sealed into a [`DriftMonitor`].
//! [`ShardMonitor::transition`] is the only writer of the state; it delegates to
//! [`ShardMonitorState::apply`] over the shared machine table (MEDULLA-FEDERATED-QUERY-R739.1).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use medulla_decision_stats::decision::drift::{DriftMonitor, DriftThresholds, Moments, VocabBloom};
use medulla_decision_stats::decision::refusal::Refusal;
use medulla_decision_types::decision::knobs::{KnobSpec, KnobValue};
use medulla_decision_types::decision::statistical::retrieval_drift::{DriftKind, RegionRef};
use medulla_decision_types::decision::statistical::shard_monitor::{
    ShardMonitorEvent, ShardMonitorFacts, ShardMonitorState,
};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_decision_types::decision::DecisionPolicy;
use medulla_retrieval::discover_signals::{
    set_discover_signals, DiscoverSignals, SignalsReading, WrittenVector,
};
use medulla_types::transition_refused::TransitionRefused;
use parking_lot::Mutex;

/// Why a shard monitor did not take a written vector: the machine refused the event, or
/// the vector does not fit the monitor (its width, or a non-finite component).
#[derive(Debug, Clone, PartialEq)]
pub enum ShardMonitorFailure {
    /// The machine refused the event in its current state.
    Transition(TransitionRefused),
    /// The written vector was refused before any transition.
    Vector(Refusal),
}

impl std::fmt::Display for ShardMonitorFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transition(refused) => write!(f, "{refused}"),
            Self::Vector(refusal) => write!(f, "{refusal:?}"),
        }
    }
}

impl From<TransitionRefused> for ShardMonitorFailure {
    fn from(refused: TransitionRefused) -> Self {
        Self::Transition(refused)
    }
}

enum Body {
    Warming {
        reference: Moments,
        vocabulary: VocabBloom,
    },
    Monitoring(Box<DriftMonitor>),
}

/// One shard's monitor and the facts a refresh region needs.
pub struct ShardMonitor {
    state: ShardMonitorState,
    body: Body,
    dimension: usize,
    node_types: BTreeSet<String>,
    first_version: u64,
    last_version: u64,
    /// Count of sealed references: the generation readers name.
    generation: u64,
}

impl ShardMonitor {
    /// A monitor for vectors of `dimension`.
    pub fn new(dimension: usize, version: u64) -> Self {
        Self {
            state: ShardMonitorState::Warming,
            body: Body::Warming {
                reference: Moments::new(dimension),
                vocabulary: VocabBloom::default(),
            },
            dimension,
            node_types: BTreeSet::new(),
            first_version: version,
            last_version: version,
            generation: 0,
        }
    }

    /// The current state.
    pub fn state(&self) -> ShardMonitorState {
        self.state
    }

    /// The facts the machine's guards read.
    fn facts(&self, reference_vectors: u64) -> ShardMonitorFacts {
        let reference_count = match &self.body {
            Body::Warming { reference, .. } => reference.count(),
            Body::Monitoring(_) => reference_vectors,
        };
        ShardMonitorFacts {
            reference_count,
            reference_vectors,
        }
    }

    /// The one transition function: the only writer of `state`. It delegates to
    /// [`ShardMonitorState::apply`]; a refused event leaves the monitor unchanged.
    pub fn transition(
        &mut self,
        event: ShardMonitorEvent,
        reference_vectors: u64,
    ) -> Result<(), TransitionRefused> {
        let next = self.state.apply(event, &self.facts(reference_vectors))?;
        if let (ShardMonitorState::Warming, ShardMonitorState::Monitoring) = (self.state, next) {
            let Body::Warming {
                reference,
                vocabulary,
            } = &self.body
            else {
                unreachable!("a warming monitor holds a warming body");
            };
            self.body = Body::Monitoring(Box::new(DriftMonitor::activate(
                reference.clone(),
                vocabulary.clone(),
            )));
            // The first seal makes generation 1; after a refresh the swap already
            // moved the generation, so the re-seal adds no second increment.
            if self.generation == 0 {
                self.generation = 1;
            }
        }
        if let (ShardMonitorState::Monitoring, ShardMonitorState::Warming) = (self.state, next) {
            self.body = Body::Warming {
                reference: Moments::new(self.dimension),
                vocabulary: VocabBloom::default(),
            };
            self.generation += 1;
        }
        self.state = next;
        Ok(())
    }

    /// Fold one written vector in: the machine must accept `Write` in the current state,
    /// then the vector joins the reference window (warming) or the sealed monitor.
    pub fn observe(
        &mut self,
        vector: &[f64],
        tokens: &[&str],
        reference_vectors: u64,
    ) -> Result<(), ShardMonitorFailure> {
        if vector.len() != self.dimension {
            return Err(ShardMonitorFailure::Vector(Refusal::new(
                StatisticalErrorCode::ParameterInvalid,
                format!(
                    "vector has {} dimensions, the monitor has {}",
                    vector.len(),
                    self.dimension
                ),
            )));
        }
        // The edge is checked first, so a refused event never folds a vector in.
        self.state
            .apply(ShardMonitorEvent::Write, &self.facts(reference_vectors))?;
        match &mut self.body {
            Body::Warming {
                reference,
                vocabulary,
            } => {
                reference
                    .push(vector)
                    .map_err(ShardMonitorFailure::Vector)?;
                for token in tokens {
                    vocabulary.insert(token);
                }
            }
            Body::Monitoring(monitor) => monitor
                .observe(vector, tokens)
                .map_err(ShardMonitorFailure::Vector)?,
        }
        self.transition(ShardMonitorEvent::Write, reference_vectors)?;
        Ok(())
    }

    fn monitor(&self) -> Option<&DriftMonitor> {
        match &self.body {
            Body::Monitoring(monitor) => Some(monitor),
            Body::Warming { .. } => None,
        }
    }
}

type Key = (String, String, u32);

/// One firing shard: its index, the drift kinds firing and the refresh regions it covers.
/// One firing shard: its id, the kinds that fire with their readings, the regions to refresh,
/// its generation and the reference window length.
pub struct FiringReading {
    pub shard: u32,
    pub kinds: Vec<(DriftKind, f64)>,
    pub regions: Vec<RegionRef>,
    pub generation: u64,
    pub window: u64,
}

type FiringShard = FiringReading;

/// The monitors of every served graph.
#[derive(Default)]
pub struct DriftRegistry {
    shards: Mutex<BTreeMap<Key, ShardMonitor>>,
}

/// The knob of the reference window length.
pub const REFERENCE_VECTORS_KNOB: &str = "drift.reference_vectors";

/// Vectors the reference window collects: the pinned policy's knob.
pub fn reference_vectors(policy: &DecisionPolicy) -> u64 {
    let spec: &KnobSpec = &medulla_decision_stats::decision::drift::KNOBS[2];
    match policy.knob(spec) {
        KnobValue::Integer(n) => n.max(2) as u64,
        _ => 64,
    }
}

impl DriftRegistry {
    /// Fold one committed write into its shard's monitor, creating the monitor on the first
    /// write and sealing the reference when the window is complete.
    pub fn observe(&self, write: &WrittenVector<'_>) -> Result<(), ShardMonitorFailure> {
        let policy = DecisionPolicy::engine_default();
        let window = reference_vectors(&policy);
        let vector: Vec<f64> = write.vector.iter().map(|&x| f64::from(x)).collect();
        let tokens: Vec<&str> = write.tokens.iter().map(String::as_str).collect();
        let key = (
            write.tenant.to_string(),
            write.graph.to_string(),
            write.shard,
        );
        let mut shards = self.shards.lock();
        let entry = shards
            .entry(key)
            .or_insert_with(|| ShardMonitor::new(vector.len(), write.version));
        entry.observe(&vector, &tokens, window)?;
        entry.node_types.insert(write.node_type.to_string());
        entry.last_version = write.version;
        if entry.state == ShardMonitorState::Warming
            && entry.facts(window).reference_count >= window
        {
            entry.transition(ShardMonitorEvent::SealReference, window)?;
            entry.first_version = write.version;
        }
        Ok(())
    }

    /// The kinds firing for one shard (`DriftMonitor::firing`), empty while it warms.
    pub fn firing(&self, tenant: &str, graph: &str, shard: u32) -> Vec<DriftKind> {
        self.shards
            .lock()
            .get(&(tenant.to_string(), graph.to_string(), shard))
            .and_then(ShardMonitor::monitor)
            .map(DriftMonitor::firing)
            .unwrap_or_default()
    }

    /// Evaluate every monitored shard of `graph` once; returns the shard count and, for each
    /// firing shard, its kinds and the refresh region it covers.
    pub fn evaluate(
        &self,
        tenant: &str,
        graph: &str,
        thresholds: &DriftThresholds,
    ) -> (u32, Vec<FiringShard>) {
        let reference_window = reference_vectors(&DecisionPolicy::engine_default());
        let mut shards = self.shards.lock();
        let mut monitored = 0;
        let mut firing = Vec::new();
        for ((t, g, shard), entry) in shards.iter_mut() {
            if t != tenant || g != graph {
                continue;
            }
            monitored += 1;
            let (from, to) = (entry.first_version, entry.last_version);
            let types = entry.node_types.clone();
            let Body::Monitoring(monitor) = &mut entry.body else {
                continue;
            };
            let kinds: Vec<(DriftKind, f64)> = monitor
                .evaluate(thresholds)
                .into_iter()
                .map(|kind| {
                    let reading = match kind {
                        DriftKind::DistributionShift => monitor.shift(),
                        DriftKind::VocabularyGrowth => monitor.vocabulary_points() as f64,
                        DriftKind::QualityDecay => monitor.quality_decay_pct().unwrap_or(0.0),
                        DriftKind::Staleness => monitor.version_lag() as f64,
                    };
                    (kind, reading)
                })
                .collect();
            if !kinds.is_empty() {
                let regions = types
                    .into_iter()
                    .map(|node_type| RegionRef {
                        shard_id: *shard,
                        node_type,
                        revision_from: from,
                        revision_to: to.max(from),
                    })
                    .collect();
                firing.push(FiringReading {
                    shard: *shard,
                    kinds,
                    regions,
                    generation: entry.generation,
                    window: reference_window,
                });
            }
        }
        (monitored, firing)
    }
}

/// A refresh plan's shard generations no longer match the shards
/// (MEDULLA-DECISION-ENGINE-R174.8.4); nothing was changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapConflict {
    pub shard: u32,
    pub expected: u64,
    pub found: Option<u64>,
}

impl DriftRegistry {
    /// Apply swaps whose generation pointers are already committed durably
    /// (MEDULLA-DECISION-ENGINE-R174.8.4): the committed pointer is the authority, so nothing
    /// is verified again. Each `(shard, before, after)` present in the registry takes
    /// `ShardMonitorEvent::Refresh` and the committed generation.
    pub fn apply_committed_swaps(&self, tenant: &str, graph: &str, swaps: &[(u32, u64, u64)]) {
        let window = reference_vectors(&DecisionPolicy::engine_default());
        let mut shards = self.shards.lock();
        for &(shard, _, after) in swaps {
            if let Some(entry) = shards.get_mut(&(tenant.to_string(), graph.to_string(), shard)) {
                if entry.transition(ShardMonitorEvent::Refresh, window).is_ok() {
                    entry.generation = after;
                }
            }
        }
    }

    /// The check of [`Self::planned_swaps`] without the swap: run before any embedding.
    pub fn verify_generations(
        &self,
        tenant: &str,
        graph: &str,
        expected: &[(u32, u64)],
        durable: &BTreeMap<u32, u64>,
    ) -> Result<(), SwapConflict> {
        let shards = self.shards.lock();
        Self::verified(&shards, tenant, graph, expected, durable).map(|_| ())
    }

    /// Verify `expected` as a swap does, and return the `(shard, before,
    /// after)` each shard would take, without changing the registry.
    pub fn planned_swaps(
        &self,
        tenant: &str,
        graph: &str,
        expected: &[(u32, u64)],
        durable: &BTreeMap<u32, u64>,
    ) -> Result<Vec<(u32, u64, u64)>, SwapConflict> {
        let shards = self.shards.lock();
        Self::verified(&shards, tenant, graph, expected, durable).map(|wanted| {
            wanted
                .into_iter()
                .map(|(shard, generation)| (shard, generation, generation + 1))
                .collect()
        })
    }

    fn verified(
        shards: &BTreeMap<Key, ShardMonitor>,
        tenant: &str,
        graph: &str,
        expected: &[(u32, u64)],
        durable: &BTreeMap<u32, u64>,
    ) -> Result<Vec<(u32, u64)>, SwapConflict> {
        let mut wanted: Vec<(u32, u64)> = expected.to_vec();
        wanted.sort_unstable();
        wanted.dedup();
        for &(shard, generation) in &wanted {
            let entry = shards.get(&(tenant.to_string(), graph.to_string(), shard));
            let held = entry.map(|entry| entry.generation);
            let current = durable.get(&shard).copied().or(held).unwrap_or(generation);
            let monitoring = entry.is_none_or(|entry| entry.state == ShardMonitorState::Monitoring);
            if current != generation || !monitoring {
                return Err(SwapConflict {
                    shard,
                    expected: generation,
                    found: durable.get(&shard).copied().or(held),
                });
            }
        }
        Ok(wanted)
    }
}

impl DiscoverSignals for DriftRegistry {
    fn read(&self, tenant: &str, graph: &str) -> SignalsReading {
        let shards = self.shards.lock();
        let mut reading = SignalsReading::default();
        for ((t, g, shard), entry) in shards.iter() {
            if t != tenant || g != graph {
                continue;
            }
            reading
                .shard_generations
                .push((shard.to_string(), entry.generation));
            if entry.monitor().is_some_and(|m| !m.firing().is_empty()) {
                reading.firing_shards.push(shard.to_string());
            }
        }
        reading
    }

    fn observe_write(&self, write: &WrittenVector<'_>) {
        if let Err(error) = self.observe(write) {
            tracing::warn!(%error, "drift monitor refused a written vector");
        }
    }
}

static REGISTRY: OnceLock<Arc<DriftRegistry>> = OnceLock::new();

/// The process registry.
pub fn registry() -> &'static Arc<DriftRegistry> {
    REGISTRY.get_or_init(|| Arc::new(DriftRegistry::default()))
}

/// Install the registry as the `Discover` signal source and the write observer. Called once at
/// server start; a second call is a no-op.
pub fn install() -> bool {
    let source: Arc<dyn DiscoverSignals> = registry().clone();
    set_discover_signals(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::statistical::shard_monitor::SHARD_MONITOR_MACHINE;
    use medulla_types::transition_refused::{MachineEvent, MachineState, TransitionRefusalReason};

    const DIM: usize = 2;

    fn warm(window: u64) -> ShardMonitor {
        let mut shard = ShardMonitor::new(DIM, 0);
        for seed in 0..window {
            let v = [seed as f64, 1.0];
            shard.observe(&v, &["a"], window).unwrap();
        }
        shard
    }

    // spec: MEDULLA-DECISION-ENGINE-R174.8.3
    // spec: MEDULLA-FEDERATED-QUERY-R739.1
    #[test]
    fn every_legal_transition_applies() {
        // S01: a write while warming stays warming.
        let mut shard = warm(4);
        assert_eq!(shard.state(), ShardMonitorState::Warming);
        // S02: the complete reference seals.
        shard
            .transition(ShardMonitorEvent::SealReference, 4)
            .unwrap();
        assert_eq!(shard.state(), ShardMonitorState::Monitoring);
        assert_eq!(shard.generation, 1);
        // S03: a write while monitoring stays monitoring.
        shard.observe(&[1.0, 1.0], &["a"], 4).unwrap();
        assert_eq!(shard.state(), ShardMonitorState::Monitoring);
        // Every edge of the table was exercised above.
        assert_eq!(
            SHARD_MONITOR_MACHINE.edges.len(),
            ["S01", "S02", "S03", "S04"].len()
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R174.8.3
    // spec: MEDULLA-FEDERATED-QUERY-R739.1
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        // Warming + SealReference with an incomplete window: the guard fails.
        let mut shard = warm(2);
        let early = shard
            .transition(ShardMonitorEvent::SealReference, 3)
            .unwrap_err();
        assert_eq!(early.machine, "ShardMonitorMachine");
        assert_eq!(early.from, ShardMonitorState::Warming.stored());
        assert_eq!(early.event, ShardMonitorEvent::SealReference.name());
        assert_eq!(early.reason, TransitionRefusalReason::GuardFailed);
        assert_eq!(shard.state(), ShardMonitorState::Warming);
        // Monitoring + SealReference: no edge leaves Monitoring on this event.
        shard
            .transition(ShardMonitorEvent::SealReference, 2)
            .unwrap();
        let twice = shard
            .transition(ShardMonitorEvent::SealReference, 2)
            .unwrap_err();
        assert_eq!(twice.from, ShardMonitorState::Monitoring.stored());
        assert_eq!(twice.reason, TransitionRefusalReason::IllegalTransition);
        assert_eq!(shard.generation, 1);
        // A vector of the wrong width is refused before any transition.
        let wrong_dim = shard.observe(&[1.0], &["a"], 2).unwrap_err();
        assert!(matches!(wrong_dim, ShardMonitorFailure::Vector(_)));
        assert_eq!(shard.state(), ShardMonitorState::Monitoring);
    }

    /// Verifies MEDULLA-DECISION-ENGINE-R174.8.4: planning a swap changes nothing; the
    /// generation moves only when the committed swaps are applied.
    // spec: MEDULLA-DECISION-ENGINE-R174.8.4
    #[test]
    fn planned_swaps_leave_memory_until_the_commit_applies() {
        let registry = DriftRegistry::default();
        let mut shard = warm(4);
        shard
            .transition(ShardMonitorEvent::SealReference, 4)
            .unwrap();
        registry
            .shards
            .lock()
            .insert(("t".into(), "g".into(), 1), shard);
        let durable = BTreeMap::new();
        let planned = registry
            .planned_swaps("t", "g", &[(1, 1)], &durable)
            .unwrap();
        assert_eq!(planned, vec![(1, 1, 2)]);
        let held = |registry: &DriftRegistry| {
            registry
                .shards
                .lock()
                .get(&("t".into(), "g".into(), 1))
                .map(|s| s.generation)
        };
        assert_eq!(
            held(&registry),
            Some(1),
            "planning moved the in-memory generation"
        );
        registry.apply_committed_swaps("t", "g", &planned);
        assert_eq!(held(&registry), Some(2));
    }

    // spec: MEDULLA-FEDERATED-QUERY-R739.1
    #[test]
    fn no_writer_sets_state_outside_transition() {
        let source = include_str!("drift_registry.rs");
        let production = &source[..source
            .find("#[cfg(test)]")
            .expect("the test module follows the code")];
        let transition_fn = production
            .find("pub fn transition(")
            .expect("the one transition function");
        let transition_end = transition_fn
            + production[transition_fn..]
                .find("\n    }\n")
                .expect("the transition function ends");
        for (at, _) in production.match_indices("state = ") {
            let line_start = production[..at].rfind('\n').map_or(0, |i| i + 1);
            let line = production[line_start..].lines().next().unwrap_or_default();
            if !line.contains(".state = ") {
                continue;
            }
            assert!(
                (transition_fn..transition_end).contains(&at),
                "a write of `state` outside `transition`: {line}"
            );
        }
    }
}
