//! Context window budget and recursive parallel-map limits
//! (MEDULLA-DECISION-ENGINE-R137.1 and R137.2).
//!
//! A prompt that passes the over-window fraction of the model window is split
//! in halves, depth by depth, until every chunk fits under that fraction. Each
//! leaf chunk is one model call and each pairwise merge is one more, so a plan
//! of `c` chunks costs `2c - 1` model calls and reads `c * chunk_tokens` tokens.
//! The engine plans only; it runs no model.

use serde::{Deserialize, Serialize};

use medulla_decision_types::decision::numeric::UnitRationalWire;

/// Named, replaceable defaults (MEDULLA-DECISION-ENGINE-R137.2).
pub const DEFAULT_OVER_WINDOW_NUMERATOR: u64 = 4;
pub const DEFAULT_OVER_WINDOW_DENOMINATOR: u64 = 5;
pub const DEFAULT_MAX_ITEMS: u32 = 1000;
pub const DEFAULT_MAX_DEPTH: u8 = 3;
pub const DEFAULT_MAX_MODEL_CALLS: u32 = 1000;

/// Bounds on recursive parallel work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ParallelMapLimits {
    pub max_depth: u8,
    pub max_items: u32,
    /// Ceiling on the tokens all leaf calls read together.
    pub max_cost: u64,
    pub max_model_calls: u32,
}

impl Default for ParallelMapLimits {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_items: DEFAULT_MAX_ITEMS,
            max_cost: u64::MAX,
            max_model_calls: DEFAULT_MAX_MODEL_CALLS,
        }
    }
}

/// The model window and the share of it one prompt may fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WindowBudget {
    pub window_tokens: u64,
    pub over_window_fraction: UnitRationalWire,
}

impl WindowBudget {
    /// A window with the default over-window fraction of 0.8.
    pub fn with_default_fraction(window_tokens: u64) -> Self {
        Self {
            window_tokens,
            over_window_fraction: UnitRationalWire::new(
                DEFAULT_OVER_WINDOW_NUMERATOR,
                DEFAULT_OVER_WINDOW_DENOMINATOR,
            )
            .expect("the default fraction is a unit rational"),
        }
    }

    /// The largest prompt that does not exceed the window share.
    pub fn threshold_tokens(&self) -> u64 {
        let fraction = self.over_window_fraction;
        let scaled = u128::from(self.window_tokens) * u128::from(fraction.numerator())
            / u128::from(fraction.denominator());
        u64::try_from(scaled).expect("a unit fraction never grows the window")
    }
}

/// The derived plan. `depth` 0 and one chunk means the prompt fits as is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OverWindowPlan {
    pub exceeds: bool,
    pub chunks: u32,
    pub depth: u8,
    pub chunk_tokens: u64,
    pub model_calls: u32,
}

/// Which limit stopped the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ContextBudgetLimit {
    MaxDepth,
    MaxCost,
    MaxModelCalls,
    WindowTokens,
}

/// A typed refusal; it carries no plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ContextBudgetRefusal {
    BudgetExhausted {
        limit: ContextBudgetLimit,
        observed: u64,
    },
    TooManyItems {
        max_items: u32,
        observed: u64,
    },
}

impl medulla_decision_types::decision::record::outcome::DecisionRefusal for ContextBudgetRefusal {
    fn code(&self) -> medulla_decision_types::decision::record::outcome::RefusalCode {
        match self {
            Self::BudgetExhausted { .. } => {
                medulla_decision_types::decision::codes::CONTEXT_BUDGET_EXHAUSTED
            }
            Self::TooManyItems { .. } => {
                medulla_decision_types::decision::codes::CONTEXT_BUDGET_TOO_MANY_ITEMS
            }
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::BudgetExhausted { limit, observed } => {
                format!("{limit:?} exceeded: observed {observed}")
            }
            Self::TooManyItems {
                max_items,
                observed,
            } => {
                format!("{observed} items exceed the bound of {max_items}")
            }
        }
    }
}

/// One context source a question may place in the window
/// (MEDULLA-DECISION-ENGINE-R029.2.1): its relevance (the head score, Q32), the
/// tokens it costs, the subject it speaks about and its authority rank
/// (`proof` 3, `observation` 2, `claim` 1, anything else 0).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AvailableContextSource {
    pub source_id: String,
    pub relevance_q32: i64,
    pub tokens: u64,
    pub subject: String,
    pub authority_rank: u8,
}

/// Why a source is not in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum OmissionReason {
    /// Relevance below `knob context_budget.relevance_min`.
    Irrelevant,
    /// Selecting it would pass the token budget.
    OverBudget,
    /// A selected source of higher authority speaks about the same subject.
    AuthorityDisplaced,
    /// The caller may not read it; reported with the caller's own id only.
    Unauthorized,
}

impl OmissionReason {
    /// The word a `context_source:<id>=<reason>` premise carries.
    pub fn word(self) -> &'static str {
        match self {
            Self::Irrelevant => "irrelevant",
            Self::OverBudget => "over_budget",
            Self::AuthorityDisplaced => "authority_displaced",
            Self::Unauthorized => "unauthorized",
        }
    }
}

/// One omitted source and the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OmittedContextSource {
    pub source_id: String,
    pub reason: OmissionReason,
}

/// The partition of the available sources: every source is selected or
/// omitted exactly once, both lists in rank order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContextPartition {
    pub selected: Vec<String>,
    pub omitted: Vec<OmittedContextSource>,
    pub total_tokens: u64,
    pub budget_tokens: u64,
}

/// A partition sealed with the digests of what it was decided under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContextSourceSelection {
    pub partition: ContextPartition,
    pub policy_digest: String,
    /// Digest of the source snapshot (ids, relevance, tokens) that was ranked.
    pub source_digest: String,
}

fn leaf(prompt_tokens: u64, depth: u8) -> (u64, u64) {
    let chunks = 1u64 << depth;
    (chunks, prompt_tokens.div_ceil(chunks))
}

fn check_limits(
    chunks: u64,
    chunk_tokens: u64,
    limits: &ParallelMapLimits,
) -> Result<(), ContextBudgetRefusal> {
    if chunks > u64::from(limits.max_items) {
        return Err(ContextBudgetRefusal::TooManyItems {
            max_items: limits.max_items,
            observed: chunks,
        });
    }
    let calls = 2 * chunks - 1;
    if calls > u64::from(limits.max_model_calls) {
        return Err(ContextBudgetRefusal::BudgetExhausted {
            limit: ContextBudgetLimit::MaxModelCalls,
            observed: calls,
        });
    }
    let cost = chunks.saturating_mul(chunk_tokens);
    if cost > limits.max_cost {
        return Err(ContextBudgetRefusal::BudgetExhausted {
            limit: ContextBudgetLimit::MaxCost,
            observed: cost,
        });
    }
    Ok(())
}

/// Decide whether a prompt exceeds its window share and, if so, the shallowest
/// halving split whose chunks each fit within every limit.
pub fn decide_over_window(
    prompt_tokens: u64,
    budget: &WindowBudget,
    limits: &ParallelMapLimits,
) -> Result<OverWindowPlan, ContextBudgetRefusal> {
    let threshold = budget.threshold_tokens();
    if threshold == 0 {
        return Err(ContextBudgetRefusal::BudgetExhausted {
            limit: ContextBudgetLimit::WindowTokens,
            observed: budget.window_tokens,
        });
    }
    if prompt_tokens <= threshold {
        return Ok(OverWindowPlan {
            exceeds: false,
            chunks: 1,
            depth: 0,
            chunk_tokens: prompt_tokens,
            model_calls: 1,
        });
    }
    // Past depth 31 the chunk count no longer fits the plan's u32 field.
    for depth in 1..=limits.max_depth.min(31) {
        let (chunks, chunk_tokens) = leaf(prompt_tokens, depth);
        check_limits(chunks, chunk_tokens, limits)?;
        if chunk_tokens <= threshold {
            return Ok(OverWindowPlan {
                exceeds: true,
                chunks: u32::try_from(chunks).expect("depth is at most 31"),
                depth,
                chunk_tokens,
                model_calls: u32::try_from(2 * chunks - 1).expect("bounded by max_model_calls"),
            });
        }
    }
    Err(ContextBudgetRefusal::BudgetExhausted {
        limit: ContextBudgetLimit::MaxDepth,
        observed: u64::from(limits.max_depth),
    })
}

#[cfg(test)]
mod tests;
