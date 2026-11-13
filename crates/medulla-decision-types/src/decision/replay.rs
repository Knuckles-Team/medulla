//! Walk-forward policy replay (EH-528, ANALYTICS-HARVEST AH-08).
//!
//! `DecisionEval` evaluates a candidate head off-policy by default. A replay
//! evaluation instead RUNS the head forward through time: the admitted items
//! are ordered by `recorded_at_ms`, cut into walk-forward folds, and at each
//! test step the head (refitted on the fold's trailing training window when
//! asked) allocates a shared budget over the step's options. Each option's
//! utility is its label, which must not depend on what the policy chose: the
//! request declares that assumption, and bandit-labelled data (whose outcome
//! exists only for the executed option) is refused in favour of OPE.
//!
//! The result is sealed as a content-addressed [`EvaluationRun`] that may
//! name the run it supersedes.

use serde::{Deserialize, Serialize};

use super::jobs::{EvalCandidate, OptimiserSpec};
use super::numeric::QuantisedValue;
use super::request::DecisionPolicyRef;
use super::statistical::head::HeadKind;
use medulla_agent_types::agent_component::ComponentDependency;
use medulla_types::contract::BoundedVec;

/// Most folds one replay may cut.
pub const MAX_REPLAY_FOLDS: usize = 256;
/// Most options one run reports a contribution for.
pub const MAX_REPLAY_CONTRIBUTIONS: usize = 256;
/// Most replayed steps one run carries on its utility path.
pub const MAX_REPLAY_STEPS: usize = 4_096;
/// Most events one event replay applies.
pub const MAX_REPLAY_EVENTS: usize = 4_096;
/// Most outcome references one event carries.
pub const MAX_EVENT_OUTCOME_REFS: usize = 64;

/// One recorded decision event an event replay applies in sequence
/// (MEDULLA-DECISION-ENGINE-R088.2.1): the pinned snapshot the decision read,
/// the action taken, and the outcomes it later joined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReplayEvent {
    pub event_id: String,
    /// Position in the stream; sequence numbers increase by one.
    pub seq: u64,
    pub event_time_ms: u64,
    /// The component revision the decision read.
    pub snapshot: ComponentDependency,
    pub action_id: String,
    pub outcome_refs: BoundedVec<String, MAX_EVENT_OUTCOME_REFS>,
    pub source_revision: String,
}

/// What one event replay answers: the digest of every applied event in order,
/// the cursor a later replay resumes from (`<seq>:<event digest>` of the last
/// applied event), the policy it ran under and the digest of its result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReplayReceipt {
    pub event_digests: BoundedVec<String, MAX_REPLAY_EVENTS>,
    pub cursor: String,
    pub policy: DecisionPolicyRef,
    pub result_digest: String,
}

/// Walk-forward folds, in admitted items. Fold `k` tests the `test` items
/// starting at `train + purge + k * step`; it trains on the `train` items that
/// end `purge` items before its test window, minus the `embargo` items after
/// every earlier test window (their outcomes may still be maturing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WalkForward {
    pub train: u32,
    pub test: u32,
    pub step: u32,
    pub purge: u32,
    pub embargo: u32,
}

/// How requests are scaled when they exceed the shared cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum AllocationRule {
    /// Every request is scaled by the same factor `cap / sum |request|`.
    Proportional,
}

/// One budget shared by every option of a step (capital, GPU-hours, tokens,
/// lane slots): the applied amounts never sum past `cap` in absolute value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SharedCap {
    pub cap: QuantisedValue,
    pub rule: AllocationRule,
}

/// What the caller asserts about the environment the policy is replayed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ReplayEnvironment {
    /// An option's outcome does not depend on which option the policy chose.
    /// The only environment replay evaluates.
    PolicyIndependent,
    /// The policy's choice changes what is observed. Replay refuses; the
    /// off-policy estimators (within the logging support) are the answer.
    PolicyDependent,
}

/// Refit the head on each fold's training window with `DecisionFit`'s kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RefitSpec {
    pub head_kind: HeadKind,
    pub optimiser: OptimiserSpec,
}

/// How many configurations the search behind this candidate tried. The
/// deflated Sharpe ratio deflates by `declared`; a declaration below the
/// search log's own count is refused (`n_trials = 1` after a search is the
/// defect this guards).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TrialLog {
    pub declared: u32,
    pub searched: u32,
}

fn no_events(events: &BoundedVec<ReplayEvent, MAX_REPLAY_EVENTS>) -> bool {
    events.as_slice().is_empty()
}

/// A replay evaluation's parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReplaySpec {
    pub folds: WalkForward,
    pub budget: SharedCap,
    pub env: ReplayEnvironment,
    /// `None` replays the candidate as submitted in every fold.
    #[serde(default)]
    pub refit: Option<RefitSpec>,
    pub trials: TrialLog,
    /// The policy the candidate is compared against; `None` is the uniform
    /// allocation over each step's options.
    #[serde(default)]
    pub incumbent: Option<EvalCandidate>,
    /// The digest of the run this one revises.
    #[serde(default)]
    pub supersedes: Option<String>,
    /// The recorded events the replay applies, in sequence order
    /// (MEDULLA-DECISION-ENGINE-R088.2.2). The replayed steps are the admitted
    /// items the events' `outcome_refs` name. Empty replays every admitted item.
    #[serde(default, skip_serializing_if = "no_events")]
    pub events: BoundedVec<ReplayEvent, MAX_REPLAY_EVENTS>,
    /// The cursor of an earlier run over the same stream's prefix: the events
    /// up to it are not re-checked against their snapshots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_cursor: Option<String>,
}

/// How a `DecisionEval` job evaluates its candidate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum EvalMode {
    /// Off-policy and calibration evaluation into a promotion receipt.
    #[default]
    OffPolicy,
    /// Walk-forward replay into a sealed [`EvaluationRun`].
    Replay { spec: Box<ReplaySpec> },
}

/// One fold's outcome. Utilities are sums over the fold's test steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReplayFoldView {
    pub train_items: u32,
    pub test_items: u32,
    pub first_test_ms: u64,
    pub last_test_ms: u64,
    /// The head the fold replayed: the candidate's digest, or its refit's.
    pub head_digest: String,
    pub utility: QuantisedValue,
    pub incumbent_utility: QuantisedValue,
    pub max_drawdown: QuantisedValue,
    /// Steps where the head read a feature outside its fitted range and
    /// allocated nothing.
    pub abstained: u32,
}

/// One option's share of the candidate's utility: the applied budget and the
/// utility it earned, summed over every step it was offered in. The utility is
/// linear in the applied amounts, so these contributions sum to the total.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OptionContribution {
    pub option_id: String,
    pub applied: QuantisedValue,
    pub utility: QuantisedValue,
}

/// The run's overfitting and comparison statistics, from the engine's own
/// validation kernels (never the caller).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReplayValidation {
    /// Per-step Sharpe-like ratio of the candidate's utility path.
    pub observed_sharpe: QuantisedValue,
    pub n_trials: u32,
    pub deflated_sharpe: QuantisedValue,
    /// Over the folds, candidate vs incumbent, train window vs test window.
    pub probability_backtest_overfit: QuantisedValue,
    /// Diebold-Mariano on the per-step losses (negated utilities), candidate
    /// minus incumbent: negative favours the candidate.
    pub diebold_mariano: QuantisedValue,
    pub diebold_mariano_p: QuantisedValue,
}

/// A sealed replay evaluation: content-addressed, immutable; a revision is a
/// new run that names this one in `supersedes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EvaluationRun {
    pub run_digest: String,
    pub head_digest: String,
    pub policy_digest: String,
    pub dataset_digest: String,
    pub spec: ReplaySpec,
    pub folds: BoundedVec<ReplayFoldView, MAX_REPLAY_FOLDS>,
    /// The candidate's utility at every replayed step, in time order.
    pub path: BoundedVec<QuantisedValue, MAX_REPLAY_STEPS>,
    pub contributions: BoundedVec<OptionContribution, MAX_REPLAY_CONTRIBUTIONS>,
    pub validation: ReplayValidation,
    pub synthetic: bool,
    /// Binds the applied events, cursor, policy and result of an event replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_receipt: Option<ReplayReceipt>,
}

/// Most lags one feature-power decay curve reports (`knob eval.ic_max_lag`).
pub const MAX_IC_DECAY_LAGS: usize = 64;
/// Most features one evaluation reports feature power for.
pub const MAX_FEATURE_POWER_REPORTS: usize = 32;

/// A bootstrap confidence interval on a feature's rank information coefficient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IcInterval {
    /// The coverage of the interval (`knob eval.ci_level`).
    pub level: QuantisedValue,
    pub lower: QuantisedValue,
    pub upper: QuantisedValue,
}

/// How much predictive power one feature carries, measured over the items
/// an evaluation admitted, in time order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FeaturePowerReport {
    pub feature: String,
    /// Rank information coefficient of the feature against the same item's outcome.
    pub rank_ic: QuantisedValue,
    /// Rank information coefficient of the feature against the outcome `k`
    /// items later, for `k` = 1 to `knob eval.ic_max_lag`, in lag order.
    pub ic_decay: BoundedVec<QuantisedValue, MAX_IC_DECAY_LAGS>,
    /// `rank_ic * sqrt(effective_sample_size)`.
    pub information_ratio: QuantisedValue,
    /// Items discounted by the feature's lag-1 autocorrelation.
    pub effective_sample_size: QuantisedValue,
    /// Seeded by the dataset digest, so the same dataset gives the same interval.
    pub ic_interval: IcInterval,
}
