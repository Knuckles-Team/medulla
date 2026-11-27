//! The statistical decision layer's engine-side computation (DL-2/DL-4/DL-6/DL-8).
//!
//! Everything a `Decide`, `DecisionFit` or `DecisionEval` answer depends on is
//! computed here, over the wire types in `medulla_decision_types::decision`, on top of the
//! pinned deterministic kernels in [`crate::detkernel`]. Living inside this
//! crate is deliberate: the crate's `clippy.toml` bans every platform float
//! transcendental, so the ban covers the decision path by construction rather
//! than by a second policy file that could drift.
//!
//! Module DAG (acyclic, depth <= 3): `quant`/`refusal` are leaves; `bm25`,
//! `candidate`, `exploration` and `admission` sit on them; `features` and
//! `head_eval` read those; `ladder`, `fit`, `evaluate` and `nl` are the tops.
//! `targets` is a leaf; the resident scorer (`scorer`) sits under
//! `head_eval` and `fit`, and `promotion` under `evaluate`. `legal_option_filter`
//! is a standalone leaf: the typed caller-candidate-filter contract ahead of
//! the real SHACL/SPARQL-backed derivation (MEDULLA-DECISION-ENGINE-R085.2).
//! `writeback` is a leaf: the typed write-back proposal, queue, inventory
//! and ticket records (MEDULLA-DECISION-ENGINE-R615.1) the approval behaviour
//! (R615.2) builds on.
//!
//! Nothing here reads a clock, a hash map's iteration order, or a thread pool:
//! reductions are serial over sorted keys, ties break by option order (the
//! caller sorts options by id), and every value that reaches a record is
//! quantised first.

pub mod adapter;
pub mod admission;
pub mod analyst_synthesis;
pub mod autonomy;
pub mod bm25;
pub mod branch_outcomes;
pub mod candidate;
pub mod changeset_review;
pub mod cluster_knobs;
pub mod comparative_analysis;
pub mod context_cursor;
pub mod context_partition;
pub mod context_search;
pub mod correction;
pub mod curriculum;
pub mod delegation;
pub mod distilled;
pub mod dmn_eval;
pub mod entity_resolution;
pub mod evolution_round;
pub mod feature_match;
pub mod grounded_eval;
pub mod importance_penalty;
pub mod insight;
pub mod instruction_disclosure;
pub mod knowledge_curation;
pub mod knowledge_flag;
pub mod legal_option_filter;
pub mod memory;
pub mod memory_benchmark;
pub mod memory_rewrite;
pub mod nl;
pub mod participant_ask;
pub mod participant_card;
pub mod participant_derive;
pub mod playbook;
pub mod population;
pub mod promotion_tier;
pub mod ranking_eval;
pub mod recommend;
pub mod refusal;
pub mod research_voi;
pub mod retrieval;
pub mod reward;
pub mod route;
pub mod run_credit;
pub mod safety;
pub mod schema_mapping;
pub mod session_note;
pub mod skill_eval;

pub use refusal::{Refusal, RefusalResult};
