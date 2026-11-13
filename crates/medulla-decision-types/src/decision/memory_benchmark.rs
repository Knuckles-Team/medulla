//! Memory-change benchmark types (MEDULLA-DECISION-ENGINE-R141.101).
//!
//! A change to a memory record is benchmarked against a dataset of queries
//! with expected evidence ids and reference answer digests, once for the
//! baseline snapshot and once for the candidate. The request carries both
//! runs; the kernel in `medulla-decision-stats` computes the metrics exactly.

use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;
use super::record::outcome::RefusalCode;

/// Domain of the benchmark receipt digest.
pub const MEMORY_BENCHMARK_DIGEST_DOMAIN: &str = "eg/memory-change-benchmark/v1";

/// Format identity of a benchmark request.
pub const MEMORY_BENCHMARK_SCHEMA_VERSION: u16 = 1;

/// The `memory_change.*` knobs. The three ratios are decimal micros.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "memory_change.min_recall",
        unit: "ratio",
        default: KnobValue::DecimalMicros(800_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "memory_change.min_answer_accuracy",
        unit: "ratio",
        default: KnobValue::DecimalMicros(800_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "memory_change.max_regression",
        unit: "ratio",
        default: KnobValue::DecimalMicros(0),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(100_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "memory_change.max_k",
        unit: "items",
        default: KnobValue::Integer(100),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(1000),
        safety: KnobSafety::Tunable,
    },
];

/// Refusal code: the dataset has no queries.
pub const MEMORY_DATASET_EMPTY: RefusalCode = RefusalCode::new("MEMORY_DATASET_EMPTY");
/// Refusal code: a query has no expected evidence ids.
pub const MEMORY_EXPECTED_MISSING: RefusalCode = RefusalCode::new("MEMORY_EXPECTED_MISSING");
/// Refusal code: a run has no result for a dataset query.
pub const MEMORY_COVERAGE_INCOMPLETE: RefusalCode = RefusalCode::new("MEMORY_COVERAGE_INCOMPLETE");
/// Refusal code: a run was measured on another snapshot than the request names.
pub const MEMORY_SNAPSHOT_MISMATCH: RefusalCode = RefusalCode::new("MEMORY_SNAPSHOT_MISMATCH");
/// Refusal code: `k` is zero or above `memory_change.max_k`, or a knob is not a ratio.
pub const MEMORY_METRIC_UNRESOLVED: RefusalCode = RefusalCode::new("MEMORY_METRIC_UNRESOLVED");
/// Refusal code: the candidate revision is not newer than the baseline revision.
pub const MEMORY_REVISION_STALE: RefusalCode = RefusalCode::new("MEMORY_REVISION_STALE");

/// Reason code: the candidate recall is below `memory_change.min_recall`.
pub const RECALL_BELOW_MIN: RefusalCode = RefusalCode::new("RECALL_BELOW_MIN");
/// Reason code: the candidate accuracy is below `memory_change.min_answer_accuracy`.
pub const ACCURACY_BELOW_MIN: RefusalCode = RefusalCode::new("ACCURACY_BELOW_MIN");
/// Reason code: a metric fell below the baseline by more than the knob allows.
pub const REGRESSION_ABOVE_MAX: RefusalCode = RefusalCode::new("REGRESSION_ABOVE_MAX");

/// The refusal codes the `memory_change` family declares, with the three
/// discard reasons.
pub const REFUSALS: &[RefusalCode] = &[
    MEMORY_DATASET_EMPTY,
    MEMORY_EXPECTED_MISSING,
    MEMORY_COVERAGE_INCOMPLETE,
    MEMORY_SNAPSHOT_MISMATCH,
    MEMORY_METRIC_UNRESOLVED,
    MEMORY_REVISION_STALE,
    RECALL_BELOW_MIN,
    ACCURACY_BELOW_MIN,
    REGRESSION_ABOVE_MAX,
];

/// What kind of change a memory mutation makes (MEDULLA-DECISION-ENGINE-R141.104).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ChangeKind {
    Add,
    Edit,
    Delete,
    Rewrite,
    Consolidate,
}

/// One revision of a memory snapshot, named by revision number and digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RevisionRef {
    pub revision: u64,
    pub digest: String,
}

/// The benchmark dataset: an id, a revision, a digest and its queries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BenchmarkDataset {
    pub id: String,
    pub revision: u64,
    pub digest: String,
    pub queries: Vec<BenchmarkQuery>,
}

/// One query with the evidence it must retrieve and its reference answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BenchmarkQuery {
    pub query_id: String,
    pub expected_evidence_ids: Vec<String>,
    pub reference_answer_digest: String,
}

/// What one snapshot returned for one query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct QueryResult {
    pub query_id: String,
    pub top_ids: Vec<String>,
    pub answer_text: String,
}

/// The results of one snapshot over the dataset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RunResults {
    /// The snapshot these results were measured on.
    pub revision: RevisionRef,
    pub results: Vec<QueryResult>,
}

/// A benchmark of one memory change: baseline and candidate over a dataset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MemoryChangeBenchmarkRequest {
    pub schema_version: u16,
    pub tenant_id: String,
    pub idempotency_key: String,
    pub record_id: String,
    pub change_kind: ChangeKind,
    pub baseline: RevisionRef,
    pub candidate: RevisionRef,
    pub dataset: BenchmarkDataset,
    pub k: u32,
    pub evaluator_revision: String,
    pub policy: super::DecisionPolicyRef,
    pub baseline_results: RunResults,
    pub candidate_results: RunResults,
}

/// The resolved `memory_change.*` thresholds, in parts per million.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BenchmarkKnobs {
    pub min_recall_ppm: u64,
    pub min_answer_accuracy_ppm: u64,
    pub max_regression_ppm: u64,
    pub max_k: u32,
}

impl BenchmarkKnobs {
    /// Read the thresholds through [`DecisionPolicy::knob`]; a knob that does
    /// not resolve to its declared type is `None` (`MEMORY_METRIC_UNRESOLVED`).
    pub fn from_policy(policy: &DecisionPolicy) -> Option<Self> {
        let micros = |spec: &KnobSpec| match policy.knob(spec) {
            KnobValue::DecimalMicros(v) => u64::try_from(v).ok(),
            _ => None,
        };
        let max_k = match policy.knob(&KNOBS[3]) {
            KnobValue::Integer(v) => u32::try_from(v).ok()?,
            _ => return None,
        };
        Some(Self {
            min_recall_ppm: micros(&KNOBS[0])?,
            min_answer_accuracy_ppm: micros(&KNOBS[1])?,
            max_regression_ppm: micros(&KNOBS[2])?,
            max_k,
        })
    }
}

/// The exact metrics of one run, as the receipt carries them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RunMetricsReceipt {
    /// Sum of the per-query recall, in parts per million.
    pub recall_ppm_sum: u64,
    pub correct: u64,
    pub queries: u64,
    pub recall_at_k_ppm: u64,
    pub answer_accuracy_ppm: u64,
}

/// The verdict of a benchmark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum BenchmarkVerdictKind {
    Admit,
    Discard,
}

/// The receipt of one scored benchmark: every pinned input by digest, both
/// runs' metrics, the verdict with every failing reason, and the premises.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MemoryBenchmarkReceipt {
    pub schema_version: u16,
    pub tenant_id: String,
    pub record_id: String,
    pub change_kind: ChangeKind,
    pub baseline: RevisionRef,
    pub candidate: RevisionRef,
    pub dataset_id: String,
    pub dataset_revision: u64,
    pub dataset_digest: String,
    pub k: u32,
    pub evaluator_revision: String,
    /// SHA-256 (64 hex characters) of the resolved decision policy.
    pub policy_digest: String,
    pub baseline_metrics: RunMetricsReceipt,
    pub candidate_metrics: RunMetricsReceipt,
    pub verdict: BenchmarkVerdictKind,
    /// The failing reasons in the order recall, accuracy, regression.
    pub reason_codes: Vec<String>,
    pub premises: Vec<String>,
    /// SHA-256 over the receipt with this field cleared.
    pub receipt_digest: String,
}

impl MemoryBenchmarkReceipt {
    /// The digest the receipt carries: the receipt with the digest cleared.
    pub fn compute_receipt_digest(&self) -> String {
        let mut subject = self.clone();
        subject.receipt_digest = String::new();
        super::digest::digest_text_hex(MEMORY_BENCHMARK_DIGEST_DOMAIN, &subject)
    }
}
