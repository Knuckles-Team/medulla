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
//! [`ShardMonitor::apply`] is the only writer of the state.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use medulla_decision_stats::decision::drift::{DriftMonitor, DriftThresholds, Moments, VocabBloom};
use medulla_decision_types::decision::knobs::{KnobSpec, KnobValue};
use medulla_decision_types::decision::statistical::retrieval_drift::{DriftKind, RegionRef};
use medulla_decision_types::decision::DecisionPolicy;
use medulla_retrieval::discover_signals::{
    set_discover_signals, DiscoverSignals, SignalsReading, WrittenVector,
};
use parking_lot::Mutex;

/// The closed states of a shard monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShardMonitorState {
    /// Collecting the reference window.
    Warming,
    /// Comparing writes against the sealed reference.
    Monitoring,
}

/// The closed events of a shard monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShardMonitorEvent {
    /// A written vector reached the shard.
    Write,
    /// The reference window is complete.
    SealReference,
}

/// An event the machine refuses in its current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardMonitorError {
    pub machine: &'static str,
    pub state: ShardMonitorState,
    pub event: ShardMonitorEvent,
    pub detail: String,
}

impl std::fmt::Display for ShardMonitorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: event {:?} refused in state {:?}: {}",
            self.machine, self.event, self.state, self.detail
        )
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

    fn refuse(&self, event: ShardMonitorEvent, detail: impl Into<String>) -> ShardMonitorError {
        ShardMonitorError {
            machine: "drift.shard_monitor",
            state: self.state,
            event,
            detail: detail.into(),
        }
    }

    /// The one transition function: the only writer of `state`.
    pub fn apply(
        &mut self,
        event: ShardMonitorEvent,
        write: Option<(&[f64], &[&str])>,
        reference_vectors: u64,
    ) -> Result<ShardMonitorState, ShardMonitorError> {
        match (self.state, event) {
            (ShardMonitorState::Warming, ShardMonitorEvent::Write) => {
                let (vector, tokens) = write.ok_or_else(|| self.refuse(event, "no vector"))?;
                let Body::Warming {
                    reference,
                    vocabulary,
                } = &mut self.body
                else {
                    return Err(self.refuse(event, "warming state without a warming body"));
                };
                reference
                    .push(vector)
                    .map_err(|r| self.refuse(event, format!("{r:?}")))?;
                for token in tokens {
                    vocabulary.insert(token);
                }
                Ok(self.state)
            }
            (ShardMonitorState::Monitoring, ShardMonitorEvent::Write) => {
                let (vector, tokens) = write.ok_or_else(|| self.refuse(event, "no vector"))?;
                let Body::Monitoring(monitor) = &mut self.body else {
                    return Err(self.refuse(event, "monitoring state without a monitor"));
                };
                monitor
                    .observe(vector, tokens)
                    .map_err(|r| self.refuse(event, format!("{r:?}")))?;
                Ok(self.state)
            }
            (ShardMonitorState::Warming, ShardMonitorEvent::SealReference) => {
                let Body::Warming {
                    reference,
                    vocabulary,
                } = &self.body
                else {
                    return Err(self.refuse(event, "warming state without a warming body"));
                };
                if reference.count() < reference_vectors {
                    return Err(self.refuse(
                        event,
                        format!(
                            "{} of {reference_vectors} reference vectors",
                            reference.count()
                        ),
                    ));
                }
                self.body = Body::Monitoring(Box::new(DriftMonitor::activate(
                    reference.clone(),
                    vocabulary.clone(),
                )));
                self.generation += 1;
                self.state = ShardMonitorState::Monitoring;
                Ok(self.state)
            }
            (ShardMonitorState::Monitoring, ShardMonitorEvent::SealReference) => {
                Err(self.refuse(event, "the reference is already sealed"))
            }
        }
    }

    fn monitor(&self) -> Option<&DriftMonitor> {
        match &self.body {
            Body::Monitoring(monitor) => Some(monitor),
            Body::Warming { .. } => None,
        }
    }
}

type Key = (String, String, u32);

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
    pub fn observe(&self, write: &WrittenVector<'_>) -> Result<(), ShardMonitorError> {
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
        if entry.dimension != vector.len() {
            return Err(entry.refuse(
                ShardMonitorEvent::Write,
                format!(
                    "vector has {} dimensions, the monitor has {}",
                    vector.len(),
                    entry.dimension
                ),
            ));
        }
        entry.apply(ShardMonitorEvent::Write, Some((&vector, &tokens)), window)?;
        entry.node_types.insert(write.node_type.to_string());
        entry.last_version = write.version;
        if entry.state == ShardMonitorState::Warming {
            let sealed = matches!(&entry.body, Body::Warming { reference, .. } if reference.count() >= window);
            if sealed {
                entry.apply(ShardMonitorEvent::SealReference, None, window)?;
                entry.first_version = write.version;
            }
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
    ) -> (u32, Vec<(u32, Vec<DriftKind>, Vec<RegionRef>)>) {
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
            let kinds = monitor.evaluate(thresholds);
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
                firing.push((*shard, kinds, regions));
            }
        }
        (monitored, firing)
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

    const DIM: usize = 2;

    fn warm(window: u64) -> ShardMonitor {
        let mut shard = ShardMonitor::new(DIM, 0);
        for seed in 0..window {
            let v = [seed as f64, 1.0];
            shard
                .apply(ShardMonitorEvent::Write, Some((&v, &["a"])), window)
                .unwrap();
        }
        shard
    }

    // spec: MEDULLA-DECISION-ENGINE-R174.8.3
    #[test]
    fn shard_monitor_legal_transitions() {
        let mut shard = warm(4);
        assert_eq!(shard.state(), ShardMonitorState::Warming);
        let sealed = shard.apply(ShardMonitorEvent::SealReference, None, 4);
        assert_eq!(sealed, Ok(ShardMonitorState::Monitoring));
        let v = [1.0, 1.0];
        let after = shard.apply(ShardMonitorEvent::Write, Some((&v, &["a"])), 4);
        assert_eq!(after, Ok(ShardMonitorState::Monitoring));
        assert_eq!(shard.generation, 1);
    }

    // spec: MEDULLA-DECISION-ENGINE-R174.8.3
    #[test]
    fn shard_monitor_illegal_transitions_are_typed() {
        let mut shard = warm(2);
        let early = shard
            .apply(ShardMonitorEvent::SealReference, None, 3)
            .unwrap_err();
        assert_eq!(early.machine, "drift.shard_monitor");
        assert_eq!(early.state, ShardMonitorState::Warming);
        assert_eq!(early.event, ShardMonitorEvent::SealReference);
        shard
            .apply(ShardMonitorEvent::SealReference, None, 2)
            .unwrap();
        let twice = shard
            .apply(ShardMonitorEvent::SealReference, None, 2)
            .unwrap_err();
        assert_eq!(twice.state, ShardMonitorState::Monitoring);
        let wrong_dim = shard
            .apply(ShardMonitorEvent::Write, Some((&[1.0], &["a"])), 2)
            .unwrap_err();
        assert_eq!(wrong_dim.event, ShardMonitorEvent::Write);
    }
}
