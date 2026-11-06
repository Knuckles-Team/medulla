//! Evidence for the KV-cache tier policy (MEDULLA-DURABLE-KERNEL-R800.5).
//!
//! The counter `medulla_kvcache_policy_total{tier,event}` counts every decision of
//! every [`medulla_kvcache::PolicyTier`] in the process: the served answer cache
//! (`tier="answer"`) and the shared block store (`tier="block"`). The newest
//! [`TRACE_CAPACITY`] decisions are kept in a fixed-size ring so `GET /kv/stats` can
//! replay them in O(1) per decision and O(1) overall. An entry holds a key digest,
//! a tenant digest, the event, the worth and the refusal reason; it never holds a
//! key, a value or a tenant name.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use medulla_kvcache::{content_hash, PolicyDecision, PolicyEvent, PolicyObserver};

/// How many decisions the replayable trace keeps.
pub(super) const TRACE_CAPACITY: usize = 1_024;

/// Hex digits of a digest kept in the trace.
const DIGEST_HEX: usize = 16;

struct TraceEntry {
    seq: u64,
    tier: String,
    event: PolicyEvent,
    key_digest: String,
    tenant_digest: String,
    worth: f64,
    reason: Option<&'static str>,
}

#[derive(Default)]
struct State {
    counts: BTreeMap<(String, &'static str), u64>,
    ring: VecDeque<TraceEntry>,
    next_seq: u64,
}

/// The decision counters and the newest-decision ring.
pub(super) struct PolicyMetrics {
    state: Mutex<State>,
}

/// A short, stable digest of `text`: what the trace keeps instead of the text.
fn digest(text: &str) -> String {
    content_hash(text.as_bytes())[..DIGEST_HEX].to_string()
}

impl PolicyMetrics {
    fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // A poisoned lock only means another thread panicked mid-record; the counters
        // are still valid integers, so keep serving them.
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Total decisions of `event` in `tier`.
    #[cfg(test)]
    pub(super) fn total(&self, tier: &str, event: PolicyEvent) -> u64 {
        self.lock()
            .counts
            .get(&(tier.to_string(), event.as_str()))
            .copied()
            .unwrap_or(0)
    }

    /// Hits over hits plus misses across every tier; `0.0` before any lookup.
    pub(super) fn hit_ratio(&self) -> f64 {
        let state = self.lock();
        let sum = |event: PolicyEvent| -> u64 {
            state
                .counts
                .iter()
                .filter(|((_, name), _)| *name == event.as_str())
                .map(|(_, total)| *total)
                .sum()
        };
        let (hits, misses) = (sum(PolicyEvent::Hit), sum(PolicyEvent::Miss));
        if hits + misses == 0 {
            0.0
        } else {
            hits as f64 / (hits + misses) as f64
        }
    }

    /// The counters, the hit ratio and the replayable trace, oldest decision first.
    pub(super) fn snapshot_json(&self) -> serde_json::Value {
        let hit_ratio = self.hit_ratio();
        let state = self.lock();
        let counters: Vec<_> = state
            .counts
            .iter()
            .map(|((tier, event), total)| {
                serde_json::json!({ "tier": tier, "event": event, "value": total })
            })
            .collect();
        let trace: Vec<_> = state
            .ring
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "seq": entry.seq,
                    "tier": entry.tier,
                    "event": entry.event.as_str(),
                    "key_digest": entry.key_digest,
                    "tenant_digest": entry.tenant_digest,
                    "worth": entry.worth,
                    "reason": entry.reason,
                })
            })
            .collect();
        serde_json::json!({
            "counter": "medulla_kvcache_policy_total",
            "counters": counters,
            "hit_ratio": hit_ratio,
            "trace_capacity": TRACE_CAPACITY,
            "trace": trace,
        })
    }

    /// The counter in Prometheus text exposition.
    #[cfg(test)]
    pub(super) fn render_prometheus(&self) -> String {
        let state = self.lock();
        state
            .counts
            .iter()
            .map(|((tier, event), total)| {
                format!(
                    "medulla_kvcache_policy_total{{tier=\"{tier}\",event=\"{event}\"}} {total}\n"
                )
            })
            .collect()
    }
}

impl PolicyObserver for PolicyMetrics {
    fn record(&self, decision: &PolicyDecision<'_>) {
        let entry = TraceEntry {
            seq: 0,
            tier: decision.tier.to_string(),
            event: decision.event,
            key_digest: digest(decision.key_digest),
            tenant_digest: digest(decision.tenant),
            worth: decision.worth,
            reason: decision.reason,
        };
        let mut state = self.lock();
        *state
            .counts
            .entry((entry.tier.clone(), decision.event.as_str()))
            .or_insert(0) += 1;
        let seq = state.next_seq;
        state.next_seq += 1;
        if state.ring.len() == TRACE_CAPACITY {
            state.ring.pop_front();
        }
        state.ring.push_back(TraceEntry { seq, ..entry });
    }
}

/// Forwards to the process-wide metrics, which the tier observer slot owns.
struct Forward(&'static PolicyMetrics);

impl PolicyObserver for Forward {
    fn record(&self, decision: &PolicyDecision<'_>) {
        self.0.record(decision);
    }
}

static GLOBAL: OnceLock<PolicyMetrics> = OnceLock::new();

/// The process-wide metrics, registered as the tier observer on first use.
pub(super) fn global() -> &'static PolicyMetrics {
    let metrics = GLOBAL.get_or_init(PolicyMetrics::new);
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    REGISTERED.get_or_init(|| medulla_kvcache::install_observer(Box::new(Forward(metrics))));
    metrics
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision<'a>(event: PolicyEvent, key: &'a str) -> PolicyDecision<'a> {
        PolicyDecision {
            tier: "block",
            event,
            tenant: "tenant-a",
            key_digest: key,
            worth: 0.5,
            reason: None,
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R800.5
    #[test]
    fn ring_keeps_the_newest_decisions_and_no_names() {
        let metrics = PolicyMetrics::new();
        for index in 0..(TRACE_CAPACITY + 10) {
            metrics.record(&decision(PolicyEvent::Admitted, &format!("key-{index}")));
        }
        metrics.record(&decision(PolicyEvent::Hit, "key-0"));
        metrics.record(&decision(PolicyEvent::Miss, "key-1"));

        let snapshot = metrics.snapshot_json();
        let trace = snapshot["trace"].as_array().unwrap();
        assert_eq!(trace.len(), TRACE_CAPACITY);
        assert_eq!(trace.last().unwrap()["event"], "miss");
        assert_eq!(
            metrics.total("block", PolicyEvent::Admitted),
            (TRACE_CAPACITY + 10) as u64
        );
        assert_eq!(metrics.hit_ratio(), 0.5);
        let text = snapshot.to_string();
        assert!(!text.contains("tenant-a"), "no tenant name in the trace");
        assert!(!text.contains("key-0\""), "no key in the trace");
        assert!(metrics
            .render_prometheus()
            .contains("medulla_kvcache_policy_total{tier=\"block\",event=\"hit\"} 1"));
    }
}
