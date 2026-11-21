//! Evidence forest contract: the candidate graph, its two separate scores and the
//! selected forest (MEDULLA-FEDERATED-QUERY-R736.1).
//!
//! These are pure data with validating constructors. `medulla_plan::evidence_forest`
//! expands and selects; the retrieval round returns the selection as its evidence rows.

use serde::{Deserialize, Serialize};
use thiserror::Error;

medulla_types::closed_error_codes! {
    /// Every typed refusal of the evidence forest; the engine's error registry declares
    /// these so a refusal keeps its code on the wire.
    pub enum ForestErrorCode {
        /// The round's context budget is zero.
        BudgetZero => "FOREST_BUDGET_ZERO",
        /// The candidate walk passed its cap.
        CandidateCap => "FOREST_CANDIDATE_CAP",
        /// The round has no seed to expand from.
        NoSeeds => "FOREST_NO_SEEDS",
    }
}

/// A typed forest refusal naming its registered code and the condition it names.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{}: {detail}", code.as_str())]
pub struct ForestRefusal {
    /// The registered refusal code.
    pub code: ForestErrorCode,
    /// The field or condition the refusal names.
    pub detail: String,
}

impl ForestRefusal {
    /// A refusal with its code and the condition it names.
    pub fn new(code: ForestErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// One candidate memory. `support` and `verify_cost` are separate values in `0..=1`;
/// neither is ever folded into the other on the candidate itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceCandidate {
    /// The candidate memory.
    pub memory_id: String,
    /// The seed id or candidate id whose relation gave the candidate its kept support.
    pub parent: String,
    /// Direct support: the seed-to-candidate score, in `0..=1`.
    pub support: f32,
    /// The cost of checking the relation that links the candidate to its parent, in `0..=1`.
    pub verify_cost: f32,
    /// The tokens the candidate spends when chosen.
    pub tokens: u32,
}

impl EvidenceCandidate {
    /// A candidate whose two scores are finite and inside `0..=1` and whose id and parent
    /// are not empty.
    pub fn new(
        memory_id: impl Into<String>,
        parent: impl Into<String>,
        support: f32,
        verify_cost: f32,
        tokens: u32,
    ) -> Result<Self, String> {
        let memory_id = memory_id.into();
        if memory_id.is_empty() {
            return Err("INVALID_ARGUMENT: candidate memory_id is empty".to_string());
        }
        let parent = parent.into();
        if parent.is_empty() {
            return Err("INVALID_ARGUMENT: candidate parent is empty".to_string());
        }
        for (field, value) in [("support", support), ("verify_cost", verify_cost)] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(format!(
                    "INVALID_ARGUMENT: candidate {field} must be a finite value in 0..=1"
                ));
            }
        }
        Ok(Self {
            memory_id,
            parent,
            support,
            verify_cost,
            tokens,
        })
    }
}

/// The selected forest: roots, the parent edges that attach every other chosen candidate to
/// one chosen parent, the tokens used and the candidates the budget dropped.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForestSelection {
    /// Chosen candidates that attach to a seed rather than to another chosen candidate.
    pub roots: Vec<String>,
    /// `(parent, child)` pairs between chosen candidates.
    pub edges: Vec<(String, String)>,
    /// The tokens the chosen candidates spend, never above the budget.
    pub tokens_used: u32,
    /// Candidates left out, in the order they were considered.
    pub dropped: Vec<String>,
}

/// Refuse a round whose context budget is zero.
pub fn check_budget(budget_tokens: u32) -> Result<(), ForestRefusal> {
    if budget_tokens == 0 {
        return Err(ForestRefusal::new(
            ForestErrorCode::BudgetZero,
            "context budget is zero",
        ));
    }
    Ok(())
}

/// Refuse a round that has no seed to expand from.
pub fn check_seeds(seed_count: usize) -> Result<(), ForestRefusal> {
    if seed_count == 0 {
        return Err(ForestRefusal::new(
            ForestErrorCode::NoSeeds,
            "the round has no seeds",
        ));
    }
    Ok(())
}

/// Refuse a candidate walk that passed its cap (`cap` is the knob `retrieval.forest.candidate_cap`).
pub fn check_candidate_cap(candidate_count: usize, cap: usize) -> Result<(), ForestRefusal> {
    if candidate_count > cap {
        return Err(ForestRefusal::new(
            ForestErrorCode::CandidateCap,
            format!("{candidate_count} candidates exceed the cap of {cap}"),
        ));
    }
    Ok(())
}

impl ForestSelection {
    /// A selection under `budget_tokens`. A zero budget refuses with `FOREST_BUDGET_ZERO`;
    /// a forest with no root, an edge whose parent is not chosen, a child that is also a
    /// root, or more tokens than the budget is malformed (`INVALID_ARGUMENT`).
    pub fn new(
        roots: Vec<String>,
        edges: Vec<(String, String)>,
        tokens_used: u32,
        dropped: Vec<String>,
        budget_tokens: u32,
    ) -> Result<Self, String> {
        check_budget(budget_tokens).map_err(|refusal| refusal.to_string())?;
        if roots.is_empty() {
            return Err("INVALID_ARGUMENT: a forest needs at least one root".to_string());
        }
        if tokens_used > budget_tokens {
            return Err("INVALID_ARGUMENT: tokens used exceed the context budget".to_string());
        }
        for (parent, child) in &edges {
            let parent_chosen = roots.contains(parent) || edges.iter().any(|(_, c)| c == parent);
            if !parent_chosen || roots.contains(child) {
                return Err(format!(
                    "INVALID_ARGUMENT: edge {parent} -> {child} does not attach to a chosen parent"
                ));
            }
        }
        Ok(Self {
            roots,
            edges,
            tokens_used,
            dropped,
        })
    }
}
