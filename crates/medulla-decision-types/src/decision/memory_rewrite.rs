//! Fidelity-gated memory rewrite evaluation: the model
//! (MEDULLA-DECISION-ENGINE-R141.100).
//!
//! A rewrite proposes a candidate revision of one memory record. The kernel in
//! `medulla-decision-stats` measures how much of the active revision's signal
//! the candidate keeps and returns an `Admit` or a `Discard`; a `Discard` leaves
//! the active revision in place. Nothing here reads a clock or a store.

use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;
use medulla_types::contract::BoundedVec;
use medulla_types::refusal_code::RefusalCode;

/// Current wire schema version of [`MemoryRewriteProposal`].
pub const MEMORY_REWRITE_SCHEMA_VERSION: u32 = 1;
/// Most history entries one proposal carries.
pub const MAX_REWRITE_HISTORY: usize = 50;
/// Fidelity and efficiency are stored in units of 1/10000.
pub const Q4_ONE: u64 = 10_000;
/// Domain of the history and receipt digests.
pub const MEMORY_REWRITE_DIGEST_DOMAIN: &str = "eg/memory-rewrite/v1";
/// Domain of the metric specification digest.
pub const MEMORY_METRIC_DIGEST_DOMAIN: &str = "eg/memory-fidelity-metric/v1";

/// Refusal: the metric id is not an entry of [`MEMORY_FIDELITY_METRICS`].
pub const MEMORY_METRIC_UNRESOLVED: RefusalCode = RefusalCode::new("MEMORY_METRIC_UNRESOLVED");
/// Refusal: the proposal names no policy digest.
pub const MEMORY_POLICY_MISSING: RefusalCode = RefusalCode::new("MEMORY_POLICY_MISSING");
/// Refusal: the proposal carries no history digest, or a history that does not
/// match it.
pub const MEMORY_HISTORY_MISSING: RefusalCode = RefusalCode::new("MEMORY_HISTORY_MISSING");
/// Refusal: the token count is zero.
pub const MEMORY_TOKEN_DENOMINATOR_ZERO: RefusalCode =
    RefusalCode::new("MEMORY_TOKEN_DENOMINATOR_ZERO");
/// Refusal: the active revision has no signals.
pub const MEMORY_ACTIVE_SIGNALS_EMPTY: RefusalCode =
    RefusalCode::new("MEMORY_ACTIVE_SIGNALS_EMPTY");
/// Refusal: the active revision differs from the stored head.
pub const MEMORY_REVISION_STALE: RefusalCode = RefusalCode::new("MEMORY_REVISION_STALE");

/// Every refusal code of the memory-rewrite evaluation (the `memory_change`
/// family table).
pub const REFUSALS: &[RefusalCode] = &[
    MEMORY_METRIC_UNRESOLVED,
    MEMORY_POLICY_MISSING,
    MEMORY_HISTORY_MISSING,
    MEMORY_TOKEN_DENOMINATOR_ZERO,
    MEMORY_ACTIVE_SIGNALS_EMPTY,
    MEMORY_REVISION_STALE,
];

/// One metric the engine can resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryFidelityMetric {
    pub id: &'static str,
    pub revision: u32,
    /// The specification text the metric's digest covers.
    pub specification: &'static str,
}

/// The one metric of the table.
pub const MEMORY_FIDELITY_SET_OVERLAP: MemoryFidelityMetric = MemoryFidelityMetric {
    id: "memory_fidelity_set_overlap",
    revision: 1,
    specification: "Each signal is normalized (NFC, lower case, whitespace collapsed to single \
spaces, trimmed) and hashed with SHA-256. fidelity = |S_active intersect S_candidate| / \
|S_active| over the hash sets, rounded half to even to four decimals. signal_per_token = \
distinct candidate signals / token_count, rounded the same way.",
};

/// Every metric the engine can resolve.
pub const MEMORY_FIDELITY_METRICS: &[MemoryFidelityMetric] = &[MEMORY_FIDELITY_SET_OVERLAP];

impl MemoryFidelityMetric {
    /// The metric a proposal's `metric_id` names, if the table holds it.
    pub fn resolve(metric_id: &str) -> Option<&'static MemoryFidelityMetric> {
        MEMORY_FIDELITY_METRICS.iter().find(|m| m.id == metric_id)
    }

    /// SHA-256 (64 lowercase hex characters) of the specification text.
    pub fn specification_digest(&self) -> String {
        super::digest::digest_text_hex(MEMORY_METRIC_DIGEST_DOMAIN, &self.specification)
    }
}

/// The `memory_rewrite.*` knobs, decision-policy knobs (DECISIONS
/// DE-R059.1-KNOBS), each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "memory_rewrite.min_fidelity",
        unit: "ratio",
        default: KnobValue::DecimalMicros(950_000),
        min: KnobValue::DecimalMicros(500_000),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "memory_rewrite.min_signal_per_token",
        unit: "signals per token",
        default: KnobValue::DecimalMicros(20_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "memory_rewrite.history_window",
        unit: "rewrites",
        default: KnobValue::Integer(5),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(50),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "memory_rewrite.max_degradations",
        unit: "rewrites",
        default: KnobValue::Integer(2),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(50),
        safety: KnobSafety::Tunable,
    },
];

/// The resolved `memory_rewrite.*` thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRewriteKnobs {
    /// Minimum fidelity, in millionths.
    pub min_fidelity_micros: u64,
    /// Minimum distinct candidate signals per token, in millionths.
    pub min_signal_per_token_micros: u64,
    /// How many of the latest history entries the degradation rule reads.
    pub history_window: usize,
    /// How many below-minimum entries in the window trigger the rule.
    pub max_degradations: usize,
}

impl MemoryRewriteKnobs {
    /// Read the thresholds through [`DecisionPolicy::knob`].
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let micros = |spec: &KnobSpec| match policy.knob(spec) {
            KnobValue::DecimalMicros(v) => v.max(0) as u64,
            _ => 0,
        };
        let count = |spec: &KnobSpec| match policy.knob(spec) {
            KnobValue::Integer(v) => v.max(0) as usize,
            _ => 0,
        };
        Self {
            min_fidelity_micros: micros(&KNOBS[0]),
            min_signal_per_token_micros: micros(&KNOBS[1]),
            history_window: count(&KNOBS[2]),
            max_degradations: count(&KNOBS[3]),
        }
    }

    /// The premise renderings `knob:<name>=<value>`, in table order.
    pub fn premises(&self) -> Vec<String> {
        let micros = |v: u64| KnobValue::DecimalMicros(v as i64).render();
        vec![
            format!(
                "knob:memory_rewrite.min_fidelity={}",
                micros(self.min_fidelity_micros)
            ),
            format!(
                "knob:memory_rewrite.min_signal_per_token={}",
                micros(self.min_signal_per_token_micros)
            ),
            format!("knob:memory_rewrite.history_window={}", self.history_window),
            format!(
                "knob:memory_rewrite.max_degradations={}",
                self.max_degradations
            ),
        ]
    }
}

/// One revision of a memory record: its number and content digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RevisionRef {
    pub revision: u64,
    /// SHA-256, 64 lowercase hex characters.
    pub digest: String,
}

/// One earlier rewrite's measured fidelity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HistoryEntry {
    pub revision: u64,
    pub fidelity_q4: u32,
}

/// The rewrite history of the record, oldest first, and the digest it carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RewriteHistory {
    pub entries: BoundedVec<HistoryEntry, MAX_REWRITE_HISTORY>,
    pub history_digest: String,
}

impl RewriteHistory {
    /// The history over `entries`, digest computed; more than
    /// [`MAX_REWRITE_HISTORY`] entries is refused.
    pub fn sealed(entries: Vec<HistoryEntry>) -> Result<Self, String> {
        let entries = BoundedVec::new(entries)?;
        let history_digest = Self::digest_of(entries.as_slice());
        Ok(Self {
            entries,
            history_digest,
        })
    }

    /// SHA-256 (64 lowercase hex characters) over the ordered entries.
    pub fn digest_of(entries: &[HistoryEntry]) -> String {
        super::digest::digest_text_hex(MEMORY_REWRITE_DIGEST_DOMAIN, &entries)
    }
}

/// One proposed rewrite of a memory record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MemoryRewriteProposal {
    pub schema_version: u32,
    pub tenant_id: String,
    pub idempotency_key: String,
    /// The graph whose snapshot store holds the record's head; read behind
    /// the caller's read access.
    pub graph: String,
    /// The id of the memory record the rewrite is for.
    pub record_id: String,
    pub active: RevisionRef,
    pub candidate: RevisionRef,
    pub active_signals: Vec<String>,
    pub candidate_signals: Vec<String>,
    pub token_count: u64,
    pub metric_id: String,
    /// SHA-256 of the decision policy the proposal was made under.
    pub policy: String,
    /// Where the policy comes from; the resolved policy's digest must equal
    /// `policy`, and its `memory_rewrite.*` knobs are the thresholds.
    pub policy_ref: super::DecisionPolicyRef,
    pub history: RewriteHistory,
}

/// The decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RewriteVerdict {
    /// The candidate may replace the active revision.
    Admit,
    /// The active revision stays.
    Discard,
}

/// Why a rewrite was discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RewriteReason {
    FidelityBelowMin,
    SignalEfficiencyBelowMin,
    DegradationRuleTriggered,
}

/// The evaluation of one proposal: every input, the measurements, the verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MemoryRewriteEvaluation {
    pub proposal: MemoryRewriteProposal,
    pub metric_revision: u32,
    pub metric_digest: String,
    pub fidelity_q4: u32,
    /// Distinct candidate signals.
    pub signal_count: u64,
    pub token_count: u64,
    pub signal_per_token_q4: u64,
    pub verdict: RewriteVerdict,
    pub reason_codes: Vec<RewriteReason>,
    /// SHA-256 over the evaluation with this field cleared.
    pub receipt_digest: String,
}

impl MemoryRewriteEvaluation {
    /// The evidence premises of the evaluation, knob premises included.
    pub fn premises(&self, knobs: &MemoryRewriteKnobs) -> Vec<String> {
        let mut premises = vec![
            format!("memory:record={}", self.proposal.record_id),
            format!("memory:active={}", self.proposal.active.digest),
            format!("memory:candidate={}", self.proposal.candidate.digest),
            format!(
                "memory:metric={}@{}",
                self.proposal.metric_id, self.metric_revision
            ),
            format!("memory:fidelity_q4={}", self.fidelity_q4),
            format!("memory:signal_per_token_q4={}", self.signal_per_token_q4),
            format!("memory:policy={}", self.proposal.policy),
        ];
        premises.extend(knobs.premises());
        premises
    }

    /// The digest the receipt carries: the evaluation with the digest cleared.
    pub fn compute_receipt_digest(&self) -> String {
        let mut subject = self.clone();
        subject.receipt_digest = String::new();
        super::digest::digest_text_hex(MEMORY_REWRITE_DIGEST_DOMAIN, &subject)
    }
}
