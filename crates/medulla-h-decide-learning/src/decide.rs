#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod correction;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod family;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod knowledge_flag;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_adapter;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_belief;
#[cfg(feature = "decide")]
mod stat_delegation;
#[cfg(feature = "decide")]
mod stat_gold_guarantee;
#[cfg(feature = "decide")]
mod stat_grounded_eval;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_jobs;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_log_corrections;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_memory_change;
#[cfg(feature = "decide")]
mod stat_penalty;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_promotion_hi;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_skill_eval;
// MEDULLA-DECISION-ENGINE-R142.101: the served local-skill certification.
#[cfg(feature = "decide")]
mod stat_certification;
// MEDULLA-DECISION-ENGINE-R038.6: the engine records a run output proposal's critique.
#[cfg(feature = "decide")]
mod stat_held_item_critique;
#[cfg(feature = "decide")]
mod stat_ranking_eval;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_replay;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_rule_promotion;
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_task_plan_hi;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_verdict_dataset;
// EH-528: walk-forward replay evaluation (its validation kernels are finance's).
#[cfg(all(feature = "decide", feature = "finance"))]
mod stat_walk_forward;
// EH-394..398: the retrieval-learning relations of the decision views.
#[cfg(all(feature = "decide", feature = "query"))]
mod stat_retrieval_views;
// EH-525: the learned reputation relation of the decision views.
#[cfg(all(feature = "decide", feature = "query"))]
pub(super) mod stat_reputation_hi;
// EH-066 — the decision record views (SQL catalog relations + the UQL `DECISIONS` source).
#[cfg(all(feature = "decide", feature = "query"))]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod stat_view;
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod windows_read;
