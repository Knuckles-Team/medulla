//! The clock basis of evidence and the bounded second pass (MEDULLA-FEDERATED-QUERY-R081.10).
//!
//! An evidence row may carry three typed clocks: `storage_time`, `event_time` and
//! `narrative_time`, each with its source and provenance. When the clocks disagree
//! [`choose_clock`] picks by the fixed precedence `event_time`, `narrative_time`, then
//! `storage_time`; no other heuristic picks a clock, and a missing clock is skipped.
//!
//! [`plan_second_pass`] decides whether the harness runs one more round. Sufficiency is the
//! fraction of requested slots filled with supported evidence; below the threshold it builds at
//! most `max_queries` queries from the unmet slots, and reports when the budget cut some off.
//! No I/O and no clock.

use medulla_result_types::retrieval_round::RetrievalPremise;
use serde::{Deserialize, Serialize};

/// The policy revision a clock choice reports.
pub const CLOCK_POLICY_REVISION: &str = "clock-precedence";

/// The three clocks an evidence row can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Clock {
    EventTime,
    NarrativeTime,
    StorageTime,
}

/// The fixed precedence of [`choose_clock`], highest first.
const CLOCK_PRECEDENCE: [Clock; 3] = [Clock::EventTime, Clock::NarrativeTime, Clock::StorageTime];

impl Clock {
    /// The field name of the clock.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EventTime => "event_time",
            Self::NarrativeTime => "narrative_time",
            Self::StorageTime => "storage_time",
        }
    }
}

/// One clock value with the source that supplied it and how it was derived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockValue {
    /// Unix milliseconds.
    pub value_ms: i64,
    pub source: String,
    pub provenance: String,
}

/// The three clocks of one evidence row; each may be absent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceClocks {
    #[serde(default)]
    pub storage_time: Option<ClockValue>,
    #[serde(default)]
    pub event_time: Option<ClockValue>,
    #[serde(default)]
    pub narrative_time: Option<ClockValue>,
}

impl EvidenceClocks {
    fn get(&self, clock: Clock) -> Option<&ClockValue> {
        match clock {
            Clock::EventTime => self.event_time.as_ref(),
            Clock::NarrativeTime => self.narrative_time.as_ref(),
            Clock::StorageTime => self.storage_time.as_ref(),
        }
    }
}

/// The chosen clock basis of a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockBasis {
    pub clock: Clock,
    pub value: ClockValue,
    /// Whether another present clock held a different value.
    pub disagreement: bool,
    pub policy_revision: String,
}

/// Choose the clock basis of a row, or `None` when it carries no clock.
pub fn choose_clock(clocks: &EvidenceClocks) -> Option<ClockBasis> {
    let (clock, value) = CLOCK_PRECEDENCE
        .iter()
        .find_map(|clock| clocks.get(*clock).map(|value| (*clock, value)))?;
    let disagreement = CLOCK_PRECEDENCE
        .iter()
        .filter_map(|other| clocks.get(*other))
        .any(|other| other.value_ms != value.value_ms);
    Some(ClockBasis {
        clock,
        value: value.clone(),
        disagreement,
        policy_revision: CLOCK_POLICY_REVISION.to_string(),
    })
}

/// The `retrieval.clock` premise of a chosen basis: the clock, its value, source and
/// provenance, whether the clocks disagreed, and the policy revision.
pub fn clock_premise(basis: &ClockBasis) -> RetrievalPremise {
    RetrievalPremise {
        step: "clock".into(),
        name: "retrieval.clock".into(),
        value: serde_json::json!({
            "clock": basis.clock.as_str(),
            "value_ms": basis.value.value_ms,
            "source": basis.value.source,
            "provenance": basis.value.provenance,
            "disagreement": basis.disagreement,
            "policy_revision": basis.policy_revision,
        }),
    }
}

/// One requested slot of a round and the query that would fill it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedSlot {
    pub name: String,
    /// Whether supported evidence fills the slot.
    pub filled: bool,
    pub query: String,
}

/// What the second-pass step decided.
#[derive(Debug, Clone, PartialEq)]
pub struct SecondPass {
    /// Filled slots over requested slots; `1.0` when nothing was requested.
    pub sufficiency: f64,
    /// The queries of the one second round, empty at or above the threshold.
    pub queries: Vec<String>,
    /// More slots were unmet than the query budget allows: the result is partial and the
    /// caller reports `RETRIEVAL_BUDGET_EXCEEDED`.
    pub budget_exceeded: bool,
}

/// Plan the bounded second pass over `slots`. At or above `threshold` there is no second pass.
pub fn plan_second_pass(slots: &[RequestedSlot], threshold: f64, max_queries: usize) -> SecondPass {
    let filled = slots.iter().filter(|slot| slot.filled).count();
    let sufficiency = if slots.is_empty() {
        1.0
    } else {
        filled as f64 / slots.len() as f64
    };
    if sufficiency >= threshold {
        return SecondPass {
            sufficiency,
            queries: Vec::new(),
            budget_exceeded: false,
        };
    }
    let unmet: Vec<&RequestedSlot> = slots.iter().filter(|slot| !slot.filled).collect();
    let budget_exceeded = unmet.len() > max_queries;
    let queries = unmet
        .into_iter()
        .take(max_queries)
        .map(|slot| slot.query.clone())
        .collect();
    SecondPass {
        sufficiency,
        queries,
        budget_exceeded,
    }
}
